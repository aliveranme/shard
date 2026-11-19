use crate::git_storage::{is_object_id, BlobRead, RepoHandle, TreeEntry};
use crate::native_git::{GitServiceRepo, SnapshotRead};
use serde::{Deserialize, Deserializer};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;
use worker::Result;

const MAX_DEPTH: u8 = 12;
const MAX_TREE_READS: u8 = 80;
const MAX_ENTRIES: u32 = 4_096;
const MAX_CHANGES: usize = 256;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_BLOB_BYTES: usize = 96 * 1024;
const MAX_LINES: usize = 512;
const CONTEXT: usize = 3;

pub enum Comparison {
    Complete(Vec<ChangedFile>),
    MissingSnapshot,
    Limited,
}

#[derive(Deserialize)]
pub struct ChangedFile {
    pub path: String,
    #[serde(skip)]
    pub url_path: String,
    before: Option<FileObject>,
    after: Option<FileObject>,
}

#[derive(Deserialize)]
struct FileObject {
    hash: String,
    mode: FileMode,
}

#[derive(Clone, Copy)]
struct FileMode(&'static str);

impl<'de> Deserialize<'de> for FileMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let mode = match <&str>::deserialize(deserializer)? {
            "100644" => "100644",
            "100755" => "100755",
            "120000" => "120000",
            "160000" => "160000",
            _ => return Err(serde::de::Error::custom("Invalid Git diff mode")),
        };
        Ok(Self(mode))
    }
}

impl ChangedFile {
    pub fn status(&self) -> &'static str {
        match (&self.before, &self.after) {
            (None, _) => "added",
            (_, None) => "deleted",
            _ => "modified",
        }
    }

    pub fn modes(&self) -> Option<(&'static str, &'static str)> {
        let before = self.before.as_ref()?.mode.0;
        let after = self.after.as_ref()?.mode.0;
        (before != after).then_some((before, after))
    }

    pub async fn lines(&self, target: &RepoHandle, source: &RepoHandle) -> Result<FileDiff> {
        if let (Some(before), Some(after)) = (&self.before, &self.after) {
            if before.hash == after.hash {
                return Ok(FileDiff::Unchanged);
            }
        }
        let mut text = [String::new(), String::new()];
        for ((entry, repo), output) in [(&self.before, target), (&self.after, source)].into_iter().zip(&mut text) {
            let Some(entry) = entry else { continue };
            if entry.mode.0 == "160000" {
                return Ok(FileDiff::Submodule);
            }
            let bytes = match repo.read_blob_bounded(&entry.hash, MAX_BLOB_BYTES).await? {
                BlobRead::Bytes(bytes) => bytes,
                BlobRead::TooLarge => return Ok(FileDiff::Limited),
            };
            if bytes.contains(&0) {
                return Ok(FileDiff::Binary);
            }
            *output = match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => return Ok(FileDiff::Binary),
            };
        }
        Ok(LineDiff::build(text))
    }
}

pub async fn compare(target: &RepoHandle, source: &RepoHandle, base: &str, head: &str) -> Result<Comparison> {
    if !is_object_id(base) || !is_object_id(head) {
        return Ok(Comparison::MissingSnapshot);
    }
    if let Some((target, source)) = GitServiceRepo::comparison_pair(target, source) {
        return match target.file_changes(source, base, head).await? {
            SnapshotRead::Found(files) => native_files(files),
            SnapshotRead::Missing => Ok(Comparison::MissingSnapshot),
            SnapshotRead::Limited => Ok(Comparison::Limited),
        };
    }
    let Some(base_tree) = commit_tree(target, base).await? else {
        return Ok(Comparison::MissingSnapshot);
    };
    let same = std::ptr::eq(target, source);
    let head_tree = if same && base == head {
        None
    } else {
        let Some(tree) = commit_tree(source, head).await? else {
            return Ok(Comparison::MissingSnapshot);
        };
        Some(tree)
    };
    let head_tree = head_tree.as_deref().unwrap_or(&base_tree);
    if base_tree == head_tree {
        target.read_tree_bounded(&base_tree, 0).await?;
        if !same {
            source.read_tree_bounded(head_tree, 0).await?;
        }
        return Ok(Comparison::Complete(Vec::new()));
    }
    let mut walk = Walk {
        target,
        source,
        reads_left: MAX_TREE_READS,
        entries_left: MAX_ENTRIES,
        changed: Vec::new(),
        path: String::new(),
    };
    if walk.pair(Some(&base_tree), Some(head_tree), 0).await? {
        walk.changed.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        Ok(Comparison::Complete(walk.changed))
    } else {
        Ok(Comparison::Limited)
    }
}

fn native_files(mut files: Vec<ChangedFile>) -> Result<Comparison> {
    if files.len() > MAX_CHANGES {
        return Ok(Comparison::Limited);
    }
    for file in &mut files {
        let mut parts = file.path.split('/');
        if file.path.len() > MAX_PATH_BYTES || parts.clone().nth(usize::from(MAX_DEPTH) + 1).is_some() {
            return Ok(Comparison::Limited);
        }
        if parts.any(|part| matches!(part, "" | "." | "..") || part.contains('\0'))
            || (file.before.is_none() && file.after.is_none())
            || [&file.before, &file.after].into_iter().flatten().any(|object| {
                !is_object_id(&object.hash) || object.hash.bytes().all(|byte| byte == b'0')
            })
        {
            return Err(worker::Error::RustError("Native Git origin returned an invalid file change".into()));
        }
        file.url_path = crate::util::url_path(&file.path);
    }
    files.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    if files.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(worker::Error::RustError("Native Git origin returned duplicate file changes".into()));
    }
    Ok(Comparison::Complete(files))
}

async fn commit_tree(repo: &RepoHandle, id: &str) -> Result<Option<String>> {
    Ok(repo.read_commit(id).await?
        .and_then(|commit| commit.tree)
        .filter(|tree| is_object_id(tree)))
}

struct Walk<'a> {
    target: &'a RepoHandle,
    source: &'a RepoHandle,
    reads_left: u8,
    entries_left: u32,
    changed: Vec<ChangedFile>,
    path: String,
}

impl Walk<'_> {
    async fn entries(&mut self, repo: &RepoHandle, hash: Option<&str>) -> Result<Option<Vec<TreeEntry>>> {
        let Some(hash) = hash else { return Ok(Some(Vec::new())) };
        if self.reads_left == 0 {
            return Ok(None);
        }
        self.reads_left -= 1;
        let Some(mut entries) = repo.read_tree_bounded(hash, self.entries_left).await? else {
            return Ok(None);
        };
        self.entries_left -= entries.len() as u32;
        entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(Some(entries))
    }

    async fn pair(&mut self, base: Option<&str>, head: Option<&str>, depth: u8) -> Result<bool> {
        if base == head {
            return Ok(true);
        }
        if depth > MAX_DEPTH {
            return Ok(false);
        }
        let Some(base) = self.entries(self.target, base).await? else { return Ok(false) };
        let Some(head) = self.entries(self.source, head).await? else { return Ok(false) };
        let mut base = base.into_iter().peekable();
        let mut head = head.into_iter().peekable();
        loop {
            let (before, after) = match (base.peek(), head.peek()) {
                (Some(a), Some(b)) => match a.name.cmp(&b.name) {
                    Ordering::Less => (base.next(), None),
                    Ordering::Greater => (None, head.next()),
                    Ordering::Equal => (base.next(), head.next()),
                },
                (Some(_), None) => (base.next(), None),
                (None, Some(_)) => (None, head.next()),
                (None, None) => return Ok(true),
            };
            if let (Some(a), Some(b)) = (&before, &after) {
                if a.hash == b.hash && a.mode == b.mode {
                    continue;
                }
            }
            let prefix_len = self.path.len();
            if prefix_len != 0 {
                self.path.push('/');
            }
            self.path.push_str(&before.as_ref().or(after.as_ref()).unwrap().name);
            if self.path.len() > MAX_PATH_BYTES {
                return Ok(false);
            }
            let (before_tree, before_file) = split_entry(before);
            let (after_tree, after_file) = split_entry(after);
            if before_file.is_some() || after_file.is_some() {
                if self.changed.len() == MAX_CHANGES {
                    return Ok(false);
                }
                self.changed.push(ChangedFile {
                    url_path: crate::util::url_path(&self.path),
                    path: self.path.clone(),
                    before: before_file,
                    after: after_file,
                });
            }
            if !Box::pin(self.pair(before_tree.as_deref(), after_tree.as_deref(), depth + 1)).await? {
                return Ok(false);
            }
            self.path.truncate(prefix_len);
        }
    }
}

fn split_entry(entry: Option<TreeEntry>) -> (Option<String>, Option<FileObject>) {
    match entry {
        Some(entry) if entry.is_dir => (Some(entry.hash), None),
        Some(TreeEntry { hash, mode, .. }) => (None, Some(FileObject { hash, mode: FileMode(mode) })),
        None => (None, None),
    }
}

pub enum FileDiff {
    Text(LineDiff),
    Unchanged,
    Binary,
    Submodule,
    Limited,
    Unavailable,
}

pub struct LineDiff {
    text: [String; 2],
    rows: Vec<Line>,
}

struct Line {
    old: usize,
    new: usize,
    bytes: Range<usize>,
    gap: bool,
}

pub struct DisplayLine<'a> {
    pub old: usize,
    pub new: usize,
    pub text: &'a str,
    pub class: &'static str,
    pub marker: &'static str,
    pub gap: bool,
    pub no_newline: bool,
}

impl LineDiff {
    fn build(text: [String; 2]) -> FileDiff {
        let old: Vec<_> = text[0].split_inclusive('\n').take(MAX_LINES + 1).collect();
        let new: Vec<_> = text[1].split_inclusive('\n').take(MAX_LINES + 1).collect();
        if old.len() > MAX_LINES || new.len() > MAX_LINES {
            return FileDiff::Limited;
        }
        if old == new {
            return FileDiff::Unchanged;
        }

        // Intern complete lines so the matrix compares IDs, even for long lines.
        let mut ids = HashMap::with_capacity(old.len() + new.len());
        let mut intern = |line| {
            let id = ids.len() as u16;
            *ids.entry(line).or_insert(id)
        };
        let old_ids: Vec<_> = old.iter().map(|&line| intern(line)).collect();
        let new_ids: Vec<_> = new.iter().map(|&line| intern(line)).collect();

        let cols = new.len() + 1;
        let mut lcs = vec![0_u16; (old.len() + 1) * cols];
        for (i, line) in old_ids.iter().enumerate().rev() {
            for (j, candidate) in new_ids.iter().enumerate().rev() {
                lcs[i * cols + j] = if line == candidate {
                    1 + lcs[(i + 1) * cols + j + 1]
                } else {
                    lcs[(i + 1) * cols + j].max(lcs[i * cols + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        let (mut old_at, mut new_at) = (0, 0);
        let mut rows = Vec::with_capacity(old.len() + new.len());
        while i < old.len() || j < new.len() {
            let (old_number, new_number, bytes) = if i < old.len() && j < new.len() && old_ids[i] == new_ids[j] {
                let start = old_at;
                old_at += old[i].len();
                new_at += new[j].len();
                i += 1;
                j += 1;
                (i, j, start..old_at)
            } else if i < old.len() && (j == new.len() || lcs[(i + 1) * cols + j] >= lcs[i * cols + j + 1]) {
                let start = old_at;
                old_at += old[i].len();
                i += 1;
                (i, 0, start..old_at)
            } else {
                let start = new_at;
                new_at += new[j].len();
                j += 1;
                (0, j, start..new_at)
            };
            rows.push(Line { old: old_number, new: new_number, bytes, gap: false });
        }

        let mut keep = vec![false; rows.len()];
        for (i, _) in rows.iter().enumerate().filter(|(_, row)| row.old == 0 || row.new == 0) {
            let end = (i + CONTEXT + 1).min(keep.len());
            keep[i.saturating_sub(CONTEXT)..end].fill(true);
        }
        let mut gap = false;
        let mut keep = keep.into_iter();
        rows.retain_mut(|row| {
            if keep.next() == Some(true) {
                row.gap = std::mem::take(&mut gap);
                true
            } else {
                gap = true;
                false
            }
        });
        FileDiff::Text(Self { text, rows })
    }

    pub fn rows(&self) -> impl Iterator<Item = DisplayLine<'_>> {
        self.rows.iter().map(|row| {
            let (side, class, marker) = match (row.old, row.new) {
                (0, _) => (1, "added", "+"),
                (_, 0) => (0, "deleted", "−"),
                _ => (0, "context", " "),
            };
            let source = &self.text[side][row.bytes.clone()];
            DisplayLine {
                old: row.old,
                new: row.new,
                text: source.strip_suffix('\n').unwrap_or(source),
                class,
                marker,
                gap: row.gap,
                no_newline: !source.ends_with('\n'),
            }
        })
    }
}
