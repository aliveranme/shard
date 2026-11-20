use crate::git_storage::{is_object_id, BlobRead, Commit, RepoHandle};
use crate::ctx::AppState;
use crate::handlers::common::{can_admin, not_found, page_title, visible_repo};
use crate::i18n::{CommonText, RepoObjectText, RepoText};
use crate::models::{Repo, Ts};
use crate::templates::{Avatar, Chrome, error_response, render};
use crate::util;
use askama::Template;
use worker::*;

#[derive(Template)]
#[template(path = "commit.html")]
struct CommitPage {
    chrome: Chrome,
    title: String,
    repo: Repo,
    tab: &'static str,
    is_owner: bool,
    branch_url: String,
    commit: Commit,
    author_avatar: Avatar,
    date: Option<Ts>,
    parents: Vec<String>,
    checks: crate::handlers::checks::CommitChecks,
    signature: crate::gpg::CommitSignature,
}

enum ObjectError {
    RepositoryMissing,
    CommitMissing,
    Storage(Error),
    ArchiveLimit,
    ArchiveUnsupported,
}

impl From<Error> for ObjectError {
    fn from(error: Error) -> Self {
        Self::Storage(error)
    }
}

impl ObjectError {
    fn response(self, state: &AppState) -> Result<Response> {
        let locale = state.locale;
        let (status, message) = match self {
            Self::RepositoryMissing => return not_found(state, locale.t(CommonText::RepositoryNotFound)),
            Self::CommitMissing => return not_found(state, locale.t(RepoText::CommitNotFound)),
            Self::Storage(error) => {
                console_log!("Repository object read failed: {error}");
                (502, locale.t(RepoText::StorageUnavailable))
            }
            Self::ArchiveLimit => (413, locale.t(RepoObjectText::ArchiveLimit)),
            Self::ArchiveUnsupported => (422, locale.t(RepoObjectText::ArchiveUnsupported)),
        };
        error_response(&Chrome::new(state), status, message)
    }
}

async fn load_commit(ctx: &RouteContext<AppState>) -> std::result::Result<(Repo, RepoHandle, Commit), ObjectError> {
    let owner = ctx.param("owner").map_or("", String::as_str);
    let name = ctx.param("repo").map_or("", String::as_str);
    let repo = visible_repo(&ctx.data, owner, name).await?.ok_or(ObjectError::RepositoryMissing)?;
    let hash = ctx.param("hash").map_or("", String::as_str);
    if !is_object_id(hash) {
        return Err(ObjectError::CommitMissing);
    }
    let handle = RepoHandle::open(&ctx.data.env, &repo).await?;
    let commit = handle.read_commit(hash).await?.ok_or(ObjectError::CommitMissing)?;
    Ok((repo, handle, commit))
}

pub async fn commit(req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let (repo, handle, commit) = match load_commit(&ctx).await {
        Ok(loaded) => loaded,
        Err(error) => return error.response(state),
    };
    let parents = match handle.commit_parents(&commit.hash).await {
        Ok(Some(parents)) => parents,
        Ok(None) => return ObjectError::CommitMissing.response(state),
        Err(error) => return ObjectError::Storage(error).response(state),
    };
    let branch = util::query_param(&req, "branch");
    let checks = crate::handlers::checks::load(state, repo.id, &commit.hash).await?;
    let avatars = crate::avatar::git_author_ids(state, commit.author_email.iter().map(String::as_str)).await?;
    let author_avatar = Avatar::new(&commit.author, commit.author_email.as_ref().and_then(|email| avatars.get(email)).copied());
    let signature = crate::gpg::verify_commit(state, &handle, &commit.hash).await;
    let page = CommitPage {
        chrome: Chrome::new(state),
        title: page_title(&[&repo.full_name(), state.locale.t(RepoObjectText::Commit), &commit.hash[..12]]),
        tab: "commits",
        is_owner: can_admin(state, &repo),
        branch_url: util::url_path(branch.as_deref().unwrap_or(&repo.default_branch)),
        date: commit.timestamp.map(Ts),
        repo,
        commit,
        author_avatar,
        parents,
        checks,
        signature,
    };
    let mut response = render(&page)?;
    response.headers_mut().set("Cache-Control", "private, no-store")?;
    Ok(response)
}

pub async fn archive(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let (repo, handle, commit) = match load_commit(&ctx).await {
        Ok(loaded) => loaded,
        Err(error) => return error.response(state),
    };
    let headers = archive_headers(&repo.name, &commit.hash)?;
    let stored = match handle.source_archive(&commit.hash, &repo.name).await {
        Ok(stored) => stored,
        Err(error) => return ObjectError::Storage(error).response(state),
    };
    if let Some(response) = stored {
        return Ok(response);
    }
    let Some(tree) = commit.tree.as_deref().filter(|hash| is_object_id(hash)) else {
        return ObjectError::Storage(Error::RustError("Git commit tree is missing or invalid".into())).response(state);
    };
    let mut path = format!("source-{}", commit.hash);
    let mut tar = SourceTar { bytes: Vec::new(), remaining_entries: MAX_ENTRIES };
    let packed = async {
        tar.append(("", &path), 0o755, b'5', &[])?;
        tar.add_tree(&handle, tree, &mut path, 0).await
    }.await;
    if let Err(error) = packed {
        return error.response(state);
    }
    tar.bytes.resize(tar.bytes.len() + END_SIZE, 0);
    Ok(Response::from_bytes(tar.bytes)?.with_headers(headers))
}

fn archive_headers(repo: &str, hash: &str) -> Result<Headers> {
    let filename = format!("{repo}-{hash}.tar");
    let headers = Headers::new();
    headers.set("Content-Type", "application/x-tar")?;
    headers.set("Content-Disposition", &format!("attachment; filename*=UTF-8''{}", urlencoding::encode(&filename)))?;
    headers.set("Cache-Control", "private, no-store")?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    Ok(headers)
}

const BLOCK_SIZE: usize = 512;
const END_SIZE: usize = 2 * BLOCK_SIZE;
const MAX_ARCHIVE_SIZE: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: u32 = 2_000;
const MAX_COMPONENTS: usize = 16;
const USTAR_PREFIX_SIZE: usize = 155;
const USTAR_NAME_SIZE: usize = 100;

struct SourceTar {
    bytes: Vec<u8>,
    remaining_entries: u32,
}

type ArchiveResult<T> = std::result::Result<T, ObjectError>;

impl SourceTar {
    async fn add_tree(
        &mut self,
        handle: &RepoHandle,
        tree: &str,
        path: &mut String,
        depth: usize,
    ) -> ArchiveResult<()> {
        let mut entries = handle
            .read_tree_bounded(tree, self.remaining_entries)
            .await?
            .ok_or(ObjectError::ArchiveLimit)?;
        self.remaining_entries -= entries.len() as u32;
        if depth >= MAX_COMPONENTS && !entries.is_empty() {
            return Err(ObjectError::ArchiveLimit);
        }
        entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        if entries.windows(2).any(|pair| pair[0].name == pair[1].name) {
            return Err(ObjectError::ArchiveUnsupported);
        }
        let parent_len = path.len();
        for entry in entries {
            if entry.name.len() > USTAR_NAME_SIZE || !safe_component(&entry.name) {
                return Err(ObjectError::ArchiveUnsupported);
            }
            path.push('/');
            path.push_str(&entry.name);
            let tar_path = ustar_path(path).ok_or(ObjectError::ArchiveUnsupported)?;
            match entry.mode {
                "040000" => {
                    self.append(tar_path, 0o755, b'5', &[])?;
                    Box::pin(self.add_tree(handle, &entry.hash, path, depth + 1)).await?;
                }
                "100644" | "100755" => {
                    let space = self.file_capacity()?;
                    let BlobRead::Bytes(blob) = handle.read_blob_bounded(&entry.hash, space).await? else {
                        return Err(ObjectError::ArchiveLimit);
                    };
                    let mode = if entry.mode == "100755" { 0o755 } else { 0o644 };
                    self.append(tar_path, mode, b'0', &blob)?;
                }
                _ => return Err(ObjectError::ArchiveUnsupported),
            }
            path.truncate(parent_len);
        }
        Ok(())
    }

    fn file_capacity(&self) -> ArchiveResult<usize> {
        (MAX_ARCHIVE_SIZE - END_SIZE - BLOCK_SIZE)
            .checked_sub(self.bytes.len())
            .ok_or(ObjectError::ArchiveLimit)
    }

    fn append(&mut self, (prefix, name): (&str, &str), mode: usize, kind: u8, contents: &[u8]) -> ArchiveResult<()> {
        if contents.len() > self.file_capacity()? {
            return Err(ObjectError::ArchiveLimit);
        }
        let mut header = [0; BLOCK_SIZE];
        header[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut header[100..108], mode);
        octal(&mut header[108..116], 0); // uid
        octal(&mut header[116..124], 0); // gid
        octal(&mut header[124..136], contents.len());
        octal(&mut header[136..148], 0); // mtime
        header[148..156].fill(b' ');
        header[156] = kind;
        header[257..265].copy_from_slice(b"ustar\x0000");
        header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
        let checksum = header.iter().map(|&byte| usize::from(byte)).sum();
        octal(&mut header[148..155], checksum);
        header[155] = b' ';
        self.bytes.extend_from_slice(&header);
        self.bytes.extend_from_slice(contents);
        self.bytes.resize(self.bytes.len().next_multiple_of(BLOCK_SIZE), 0);
        Ok(())
    }
}

fn octal(field: &mut [u8], mut value: usize) {
    let (terminator, digits) = field.split_last_mut().expect("nonempty tar field");
    *terminator = 0;
    for digit in digits.iter_mut().rev() {
        *digit = b'0' + (value & 7) as u8;
        value >>= 3;
    }
    debug_assert_eq!(value, 0);
}

// USTAR allows a 155-byte prefix and a 100-byte name, split at a slash.
fn ustar_path(path: &str) -> Option<(&str, &str)> {
    if path.len() <= USTAR_NAME_SIZE {
        return Some(("", path));
    }
    path.rmatch_indices('/').find_map(|(at, _)| {
        let name = &path[at + 1..];
        (at <= USTAR_PREFIX_SIZE && name.len() <= USTAR_NAME_SIZE).then_some((&path[..at], name))
    })
}

fn safe_component(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name.eq_ignore_ascii_case(".git")
        || name.chars().any(|ch| ch.is_control() || ch == '\u{fffd}')
        || name.bytes().any(|b| matches!(b, b'/' | b'\\' | b':' | b'<' | b'>' | b'"' | b'|' | b'?' | b'*'))
    {
        return false;
    }
    // DOS device names also apply inside directories and with extensions.
    let stem = name.split('.').next().unwrap_or_default().trim_end_matches(' ');
    !["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter().any(|device| stem.eq_ignore_ascii_case(device))
        && !stem.get(..3).is_some_and(|prefix| {
            (prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
                && matches!(&stem[3..], "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        })
}
