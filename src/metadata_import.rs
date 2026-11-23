use crate::ctx::AppState;
use crate::git_storage::{is_object_id, RepoHandle};
use crate::handlers::common::{load_repo_by_id, page_title};
use crate::i18n::LifecycleText as Text;
use crate::models::Repo;
use crate::repo_lifecycle::{login, owned_repo, public_https, repo_path, repository_not_found};
use crate::templates::{render, Chrome};
use crate::util::{form_value, js_int, js_text, now};
use crate::{db, refs};
use askama::Template;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use worker::d1::D1Database;
use worker::*;

const MAX_BUNDLE_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 100;
const MAX_OBJECTS: usize = 32;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_THREAD_NUMBER: i64 = 1_000_000_000;
const MAX_TIMESTAMP: i64 = 253_402_300_799;

type Failure = (Text, u16);
const INVALID: Failure = (Text::MetadataInvalid, 422);
const TOO_LARGE: Failure = (Text::MetadataTooLarge, 413);
const PRIVATE: Failure = (Text::MetadataPrivate, 409);
const SOURCE_MISMATCH: Failure = (Text::MetadataSourceMismatch, 409);
const CONFLICT: Failure = (Text::MetadataConflict, 409);
const TAG_MISMATCH: Failure = (Text::MetadataTagMismatch, 422);
const UNAVAILABLE: Failure = (Text::MetadataUnavailable, 503);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    version: u8,
    source: Source,
    #[serde(default)]
    threads: Vec<Thread>,
    #[serde(default)]
    releases: Vec<Release>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Source {
    url: String,
    private: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Thread {
    number: i64,
    title: String,
    #[serde(default)]
    body: String,
    state: ThreadState,
    created_at: i64,
    updated_at: i64,
    closed_at: Option<i64>,
    pull: Option<Pull>,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ThreadState {
    Open,
    Closed,
    Merged,
}

impl Thread {
    fn has_timestamp(&self, time: i64) -> bool {
        (self.created_at..=self.updated_at).contains(&time)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pull {
    base_branch: String,
    head_branch: String,
    head_source_url: Option<String>,
    snapshot: Option<Snapshot>,
    merged_at: Option<i64>,
    merge_commit: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    base_commit: String,
    head_commit: String,
    merge_base: Option<String>,
}

impl Snapshot {
    fn object_ids(&self) -> impl Iterator<Item = &str> {
        [self.base_commit.as_str(), self.head_commit.as_str()]
            .into_iter()
            .chain(self.merge_base.as_deref())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Release {
    tag: String,
    commit: String,
    title: String,
    #[serde(default)]
    notes: String,
    created_at: i64,
    published_at: Option<i64>,
}

impl Bundle {
    fn parse(raw: &str) -> std::result::Result<Self, Failure> {
        if raw.len() > MAX_BUNDLE_BYTES {
            return Err(TOO_LARGE);
        }
        let mut bundle: Self = serde_json::from_str(raw).map_err(|_| INVALID)?;
        let count = bundle.threads.len() + bundle.releases.len();
        if count > MAX_RECORDS {
            return Err(TOO_LARGE);
        }
        if bundle.version != 1 || count == 0 {
            return Err(INVALID);
        }
        bundle.source.url = public_https(&bundle.source.url)
            .ok_or(INVALID)?
            .as_str()
            .to_owned();
        let mut numbers = HashSet::new();
        for thread in &mut bundle.threads {
            if !(1..=MAX_THREAD_NUMBER).contains(&thread.number)
                || !numbers.insert(thread.number)
                || !valid_title(&thread.title)
                || !valid_text(&thread.body)
                || !valid_timestamp(thread.created_at)
                || !valid_timestamp(thread.updated_at)
                || thread.updated_at < thread.created_at
                || thread.closed_at.is_some_and(|time| {
                    thread.state == ThreadState::Open || !thread.has_timestamp(time)
                })
            {
                return Err(INVALID);
            }
            let state = thread.state;
            let created_at = thread.created_at;
            let updated_at = thread.updated_at;
            match &mut thread.pull {
                Some(pull) => {
                    if !valid_ref_name(&pull.base_branch)
                        || !valid_ref_name(&pull.head_branch)
                        || pull.merged_at.is_some_and(|time| {
                            state != ThreadState::Merged
                                || !(created_at..=updated_at).contains(&time)
                        })
                        || pull.merge_commit.is_some() && state != ThreadState::Merged
                    {
                        return Err(INVALID);
                    }
                    if let Some(source) = pull.head_source_url.take() {
                        let source = public_https(&source).ok_or(INVALID)?;
                        if source.as_str() != bundle.source.url {
                            // A foreign head can use a saved comparison, but must never resolve
                            // to a same-named branch in the destination when refreshing it.
                            pull.head_branch = format!("{source}:{}", pull.head_branch);
                        }
                    }
                }
                None if state == ThreadState::Merged => return Err(INVALID),
                None => {}
            }
        }
        let mut tags = HashSet::new();
        for release in &bundle.releases {
            if !valid_ref_name(&release.tag)
                || !tags.insert(release.tag.as_str())
                || !valid_title(&release.title)
                || !valid_text(&release.notes)
                || !valid_timestamp(release.created_at)
                || release
                    .published_at
                    .is_some_and(|time| !valid_timestamp(time) || time < release.created_at)
            {
                return Err(INVALID);
            }
        }
        Ok(bundle)
    }

    fn object_ids(&self) -> std::result::Result<HashSet<&str>, Failure> {
        let pulls = self
            .threads
            .iter()
            .filter_map(|thread| thread.pull.as_ref());
        let captures = pulls.flat_map(|pull| {
            pull.snapshot
                .iter()
                .flat_map(Snapshot::object_ids)
                .chain(pull.merge_commit.as_deref())
        });
        let objects: HashSet<_> = captures
            .chain(self.releases.iter().map(|release| release.commit.as_str()))
            .collect();
        if objects.len() > MAX_OBJECTS {
            return Err(TOO_LARGE);
        }
        if objects.iter().any(|id| !is_object_id(id)) {
            return Err(INVALID);
        }
        Ok(objects)
    }
}

fn valid_timestamp(time: i64) -> bool {
    (0..=MAX_TIMESTAMP).contains(&time)
}

fn valid_title(title: &str) -> bool {
    !title.trim().is_empty() && title.chars().count() <= 200 && !title.chars().any(char::is_control)
}

fn valid_text(text: &str) -> bool {
    text.len() <= MAX_BODY_BYTES && !text.contains('\0')
}

fn valid_ref_name(name: &str) -> bool {
    crate::git_wire::valid_ref(name)
}

#[derive(Template)]
#[template(path = "metadata_import.html")]
struct MetadataPage<'a> {
    chrome: Chrome,
    title: String,
    repo: &'a Repo,
    source: Option<&'a str>,
    success: Option<&'a str>,
    error: Option<&'static str>,
}

#[derive(Deserialize)]
struct RegisteredSource {
    source_url: String,
}

#[derive(Deserialize)]
struct Receipt {
    id: i64,
}

pub(crate) fn is_form_path(path: &str) -> bool {
    let mut parts = path.trim_end_matches('/').split('/');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(""), Some(owner), Some(repo), Some("settings"), Some("metadata-import"), None)
            if !owner.is_empty() && !repo.is_empty()
    )
}

pub(crate) async fn form(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    if state.user.is_none() {
        let (owner, repo) = repo_path(&ctx);
        return login(&format!("/{owner}/{repo}/settings/metadata-import"));
    }
    let Some(repo) = owned_repo(&ctx).await? else {
        return repository_not_found(state);
    };
    let source = registered_source(&state.db()?, repo.id).await?;
    render_page(state, &repo, source.as_deref(), None, None)
}

pub(crate) async fn submit(_req: Request, ctx: RouteContext<AppState>) -> Result<Response> {
    let state = &ctx.data;
    let Some(owner_id) = state.user_id() else {
        let (owner, repo) = repo_path(&ctx);
        return login(&format!("/{owner}/{repo}/settings/metadata-import"));
    };
    let Some(repo) = owned_repo(&ctx).await? else {
        return repository_not_found(state);
    };
    let database = state.db()?;
    let source = registered_source(&database, repo.id).await?;
    let fail = |error| render_page(state, &repo, source.as_deref(), None, Some(error));
    let Some(raw) = form_value(state.form()?, "bundle") else {
        return fail(INVALID);
    };
    let mut bundle = match Bundle::parse(&raw) {
        Ok(bundle) => bundle,
        Err(error) => return fail(error),
    };
    drop(raw);
    if source
        .as_deref()
        .is_some_and(|source| source != bundle.source.url)
    {
        return fail(SOURCE_MISMATCH);
    }
    if bundle.source.private && !repo.is_private() {
        return fail(PRIVATE);
    }
    let without_comparison = match validate_git(&state.env, &repo, &mut bundle).await {
        Ok(count) => count,
        Err(error) => return fail(error),
    };
    match write_bundle(&database, &repo, &bundle).await {
        Ok(true) => {
            let pulls = bundle
                .threads
                .iter()
                .filter(|thread| thread.pull.is_some())
                .count();
            let summary = state.locale.metadata_imported(
                bundle.threads.len() - pulls,
                pulls,
                bundle.releases.len(),
                without_comparison,
            );
            render_page(state, &repo, Some(&bundle.source.url), Some(&summary), None)
        }
        Ok(false) => {
            let Some(current) = load_repo_by_id(&database, repo.id)
                .await?
                .filter(|repo| repo.owner_id == owner_id)
            else {
                return repository_not_found(state);
            };
            let source = registered_source(&database, repo.id).await?;
            let error = if bundle.source.private && !current.is_private() {
                PRIVATE
            } else if source
                .as_deref()
                .is_some_and(|source| source != bundle.source.url)
            {
                SOURCE_MISMATCH
            } else {
                UNAVAILABLE
            };
            render_page(state, &current, source.as_deref(), None, Some(error))
        }
        Err(error) => fail(error),
    }
}

fn render_page(
    state: &AppState,
    repo: &Repo,
    source: Option<&str>,
    success: Option<&str>,
    error: Option<Failure>,
) -> Result<Response> {
    Ok(render(&MetadataPage {
        chrome: Chrome::new(state),
        title: page_title(&[state.locale.t(Text::MetadataImport), &repo.full_name()]),
        repo,
        source,
        success,
        error: error.map(|(message, _)| state.locale.t(message)),
    })?
    .with_status(error.map_or(200, |(_, status)| status)))
}

async fn registered_source(database: &D1Database, id: i64) -> Result<Option<String>> {
    let registered: Option<RegisteredSource> = db::first(
        database,
        "SELECT source_url FROM repo_metadata_sources WHERE repo_id = ?",
        &[js_int(id)],
    )
    .await?;
    Ok(registered.map(|source| source.source_url))
}

async fn validate_git(
    env: &Env,
    repo: &Repo,
    bundle: &mut Bundle,
) -> std::result::Result<usize, Failure> {
    let ids = bundle.object_ids()?;
    let mut missing = HashSet::new();
    if !ids.is_empty() {
        let handle = RepoHandle::open(env, repo).await.map_err(|_| UNAVAILABLE)?;
        if !bundle.releases.is_empty() {
            let inventory = refs::load(&handle).await.map_err(|_| UNAVAILABLE)?;
            let tags: HashMap<_, _> = inventory
                .tags
                .iter()
                .map(|tag| (tag.name.as_str(), tag.peeled.as_deref().unwrap_or(&tag.oid)))
                .collect();
            if bundle
                .releases
                .iter()
                .any(|release| tags.get(release.tag.as_str()) != Some(&release.commit.as_str()))
            {
                return Err(TAG_MISMATCH);
            }
        }
        for id in ids {
            let commit = handle.read_commit(id).await.map_err(|_| UNAVAILABLE)?;
            if !commit.is_some_and(|commit| {
                commit.hash == id && commit.tree.as_deref().is_some_and(is_object_id)
            }) {
                missing.insert(id.to_owned());
            }
        }
        if bundle
            .releases
            .iter()
            .any(|release| missing.contains(&release.commit))
        {
            return Err(TAG_MISMATCH);
        }
    }

    let mut without_comparison = 0;
    for pull in bundle
        .threads
        .iter_mut()
        .filter_map(|thread| thread.pull.as_mut())
    {
        if pull
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.object_ids().any(|id| missing.contains(id)))
        {
            pull.snapshot = None;
        }
        if pull
            .merge_commit
            .as_ref()
            .is_some_and(|id| missing.contains(id))
        {
            pull.merge_commit = None;
        }
        without_comparison += usize::from(pull.snapshot.is_none());
    }
    Ok(without_comparison)
}

// D1 runs a batch as one transaction. Every mutation uses the same destination
// guard, so a transfer, privacy change, or competing source registration cannot
// move a preflighted bundle into a repository that no longer accepts it.
async fn write_bundle(
    database: &D1Database,
    repo: &Repo,
    bundle: &Bundle,
) -> std::result::Result<bool, Failure> {
    let json = serde_json::to_string(bundle).map_err(|_| UNAVAILABLE)?;
    let args = [
        js_int(repo.id),
        js_int(repo.owner_id),
        js_text(&repo.storage_id),
        js_text(&bundle.source.url),
        js_int(i64::from(bundle.source.private)),
        js_text(repo.git_backend.name()),
        js_text(&json),
    ];
    let mut registration = args[..6].to_vec();
    registration.push(js_int(now()));
    let register = database
        .prepare(
            "INSERT INTO repo_metadata_sources (repo_id, source_url, imported_at)
         SELECT id, ?4, ?7 FROM repos
          WHERE id = ?1 AND owner_id = ?2 AND artifacts_repo = ?3
            AND (?5 = 0 OR visibility = 'private') AND git_backend = ?6
         ON CONFLICT(repo_id) DO NOTHING",
        )
        .bind(&registration)
        .map_err(|_| UNAVAILABLE)?;
    const TARGET: &str = "WITH target AS (
        SELECT r.id FROM repos r JOIN repo_metadata_sources s ON s.repo_id = r.id
         WHERE r.id = ?1 AND r.owner_id = ?2 AND r.artifacts_repo = ?3 AND s.source_url = ?4
           AND (?5 = 0 OR r.visibility = 'private') AND r.git_backend = ?6
    )";
    let mut statements = vec![register];
    for sql in [
        "INSERT INTO issues (repo_id, number, kind, title, body, state, created_at, updated_at, closed_at, metadata_imported)
         SELECT target.id, j.value ->> 'number', CASE WHEN j.value ->> 'pull' IS NULL THEN 'issue' ELSE 'pull' END,
                j.value ->> 'title', j.value ->> 'body', CASE WHEN j.value ->> 'state' = 'open' THEN 'open' ELSE 'closed' END,
                j.value ->> 'created_at', j.value ->> 'updated_at', j.value ->> 'closed_at', 1
           FROM target, json_each(?7, '$.threads') j",
        "INSERT INTO pulls (issue_id, head_branch, base_branch, head_commit, base_commit, merge_base, merged, merged_at, merge_commit)
         SELECT i.id, j.value ->> '$.pull.head_branch', j.value ->> '$.pull.base_branch',
                j.value ->> '$.pull.snapshot.head_commit', j.value ->> '$.pull.snapshot.base_commit',
                j.value ->> '$.pull.snapshot.merge_base', j.value ->> 'state' = 'merged',
                j.value ->> '$.pull.merged_at', j.value ->> '$.pull.merge_commit'
           FROM target JOIN issues i ON i.repo_id = target.id
           JOIN json_each(?7, '$.threads') j ON i.number = j.value ->> 'number'
          WHERE j.value ->> 'pull' IS NOT NULL",
        "INSERT INTO repo_releases (repo_id, tag_name, commit_hash, title, notes, created_at, published_at, metadata_imported)
         SELECT target.id, j.value ->> 'tag', j.value ->> 'commit', j.value ->> 'title', j.value ->> 'notes',
                j.value ->> 'created_at', j.value ->> 'published_at', 1
           FROM target, json_each(?7, '$.releases') j",
        "UPDATE repos
            SET next_issue_number = MAX(next_issue_number, COALESCE(
                    (SELECT MAX(value ->> 'number') + 1 FROM json_each(?7, '$.threads')), 1))
          WHERE id IN (SELECT id FROM target) RETURNING id",
    ] {
        statements.push(database.prepare(format!("{TARGET} {sql}")).bind(&args).map_err(|_| UNAVAILABLE)?);
    }
    let results = database.batch(statements).await.map_err(|error| {
        let message = error.to_string();
        if message.contains("UNIQUE constraint failed: issues.repo_id, issues.number")
            || message
                .contains("UNIQUE constraint failed: repo_releases.repo_id, repo_releases.tag_name")
        {
            CONFLICT
        } else {
            console_error!(
                "Could not write repository metadata import for repository {}",
                repo.id
            );
            UNAVAILABLE
        }
    })?;
    let receipts = results
        .last()
        .ok_or(UNAVAILABLE)?
        .results::<Receipt>()
        .map_err(|_| UNAVAILABLE)?;
    Ok(receipts.iter().any(|receipt| receipt.id == repo.id))
}
