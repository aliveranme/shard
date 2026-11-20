use crate::git_storage::{is_object_id, BlobRead, Commit, CopyError, CreatedRepo, GitFile, RepoHandle, TreeEntry, MAX_COMMIT_BYTES};
use js_sys::Reflect;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;
use worker::durable::{ObjectNamespace, Stub};
use worker::{Env, Error, Fetch, Method, Request, RequestInit, RequestRedirect, Response, ResponseBody, Result, Url};

const MAX_COMPARISON_BYTES: u64 = 4 * 1024 * 1024;
const ARTIFACTS_MERGE_BINDING: &str = "ARTIFACTS_MERGE_CONTAINER";
const ARTIFACTS_MERGE_PATH: &str = "/v1/artifacts/merge";

pub(crate) enum ArtifactsMergeTransport {
    Container(Stub),
    Native(GitService),
}

pub(crate) enum GitService {
    Native { origin: String, authorization: String },
    Durable(ObjectNamespace),
    R2(ObjectNamespace),
}

pub(crate) struct GitServiceRepo {
    client: GitService,
    id: String,
}

pub(crate) enum SnapshotRead<T> {
    Found(T),
    Missing,
    Limited,
}

#[derive(Serialize)]
struct Creation<'a> {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential: Option<&'a ImportCredential<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum ImportCredential<'a> {
    Basic { username: &'a str, password: &'a str },
    Bearer { token: &'a str },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WriteError {
    Missing,
    Stale,
    Conflict,
    Rejected,
    Unsupported,
    Unavailable,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RefWrite<'a> {
    reference: &'a str,
    new_oid: &'a str,
    old_oid: &'a str,
}

#[derive(Serialize)]
struct HeadWrite<'a> {
    reference: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MergeInputs<'a> {
    reference: &'a str,
    base_oid: &'a str,
    head_oid: &'a str,
}

#[derive(Serialize)]
struct MergeWrite<'a> {
    #[serde(flatten)]
    inputs: MergeInputs<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,
}

#[derive(Serialize)]
pub(crate) struct RemoteRepo<'a> {
    pub(crate) remote: &'a str,
    pub(crate) token: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeHealth {
    protocol: u8,
    merge_enabled: bool,
}

#[derive(Serialize)]
struct RemoteMerge<'a> {
    #[serde(flatten)]
    inputs: MergeInputs<'a>,
    target: &'a RemoteRepo<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a RemoteRepo<'a>>,
}

#[derive(Deserialize)]
struct MergeResult {
    oid: String,
}

#[derive(Deserialize)]
pub(crate) struct CommitData {
    #[serde(flatten)]
    pub commit: Commit,
    pub parents: Vec<String>,
}

#[derive(Deserialize)]
struct Entry {
    name: String,
    mode: String,
    hash: String,
    size: Option<u64>,
}

impl ArtifactsMergeTransport {
    pub(crate) async fn from_env(env: &Env) -> std::result::Result<Self, WriteError> {
        let binding = Reflect::get(env.as_ref(), &JsValue::from_str(ARTIFACTS_MERGE_BINDING))
            .map_err(|_| WriteError::Unavailable)?;
        if binding.is_undefined() {
            return GitService::from_env(env).map(Self::Native).map_err(|_| WriteError::Unsupported);
        }
        let container = env.durable_object(ARTIFACTS_MERGE_BINDING)
            .and_then(|namespace| namespace.get_by_name("artifacts-merge"))
            .map_err(|_| WriteError::Unavailable)?;
        let mut response = container.fetch_with_str("https://internal/healthz").await
            .map_err(|_| WriteError::Unavailable)?;
        if response.status_code() != 200 {
            discard(response);
            return Err(WriteError::Unavailable);
        }
        let health: MergeHealth = response.json().await.map_err(|_| WriteError::Unavailable)?;
        if health.protocol != 1 {
            return Err(WriteError::Unavailable);
        }
        if !health.merge_enabled {
            return Err(WriteError::Unsupported);
        }
        Ok(Self::Container(container))
    }

    pub(crate) async fn merge_remote(
        &self, reference: &str, base_oid: &str, head_oid: &str, target: &RemoteRepo<'_>, source: Option<&RemoteRepo<'_>>,
    ) -> std::result::Result<String, WriteError> {
        let payload = RemoteMerge { inputs: MergeInputs { reference, base_oid, head_oid }, target, source };
        let response = match self {
            Self::Native(native) => native.write(None, ARTIFACTS_MERGE_PATH, &payload).await?,
            Self::Container(container) => {
                let mut init = json_post(&payload).map_err(|_| WriteError::Unavailable)?;
                init.redirect = RequestRedirect::Manual;
                let request = Request::new_with_init("https://internal/v1/artifacts/merge", &init)
                    .map_err(|_| WriteError::Unavailable)?;
                container.fetch_with_request(request).await.map_err(|_| WriteError::Unavailable)?
            }
        };
        merge_result(response).await
    }
}

impl GitService {
    pub(crate) fn from_env(env: &Env) -> Result<Self> {
        let raw = env.var("NATIVE_GIT_ORIGIN")?.to_string();
        let origin = git_origin(&raw).ok_or_else(invalid_origin)?;
        let token = env.secret("NATIVE_GIT_TOKEN")?.to_string();
        if token.is_empty() || !token.bytes().all(|byte| (b'!'..=b'~').contains(&byte)) {
            return Err(Error::RustError("NATIVE_GIT_TOKEN must be a nonempty token".into()));
        }
        Ok(Self::Native { origin, authorization: format!("Bearer {token}") })
    }

    pub(crate) fn durable(env: &Env) -> Result<Self> {
        env.durable_object("GIT_REPOS").map(Self::Durable)
    }

    pub(crate) fn r2(env: &Env) -> Result<Self> {
        env.durable_object("GIT_R2_REPOS").map(Self::R2)
    }

    fn origin(&self) -> &str {
        match self {
            Self::Native { origin, .. } => origin,
            Self::Durable(_) | Self::R2(_) => "https://internal",
        }
    }

    pub(crate) fn repo(self, id: &str) -> Result<GitServiceRepo> {
        if !valid_id(id) {
            return Err(Error::RustError("Invalid Git storage ID".into()));
        }
        Ok(GitServiceRepo { client: self, id: id.to_owned() })
    }

    async fn send(&self, id: Option<&str>, url: &str, mut init: RequestInit) -> Result<Response> {
        init.redirect = RequestRedirect::Manual;
        match self {
            Self::Native { authorization, .. } => {
                init.headers.set("Authorization", authorization)?;
                Fetch::Request(Request::new_with_init(url, &init)?).send().await
            }
            Self::Durable(namespace) | Self::R2(namespace) => {
                let id = id.ok_or_else(|| Error::RustError("Missing Git storage ID".into()))?;
                let request = Request::new_with_init(url, &init)?;
                namespace.get_by_name(id)?.fetch_with_request(request).await
            }
        }
    }

    async fn control(&self, id: &str, method: Method, creation: Option<Creation<'_>>) -> Result<Response> {
        if !valid_id(id) {
            return Err(Error::RustError("Invalid Git storage ID".into()));
        }
        let init = match creation {
            Some(creation) => json_post(&creation)?,
            None => RequestInit { method, ..Default::default() },
        };
        self.send(Some(id), &format!("{}/v1/repos/{id}", self.origin()), init).await
    }

    async fn write(&self, id: Option<&str>, path: &str, payload: &impl Serialize) -> std::result::Result<Response, WriteError> {
        let init = json_post(payload).map_err(|_| WriteError::Unavailable)?;
        self.send(id, &format!("{}{path}", self.origin()), init).await.map_err(|_| WriteError::Unavailable)
    }

    pub(crate) async fn create(&self, id: &str) -> Result<CreatedRepo> {
        let response = self.control(id, Method::Post, Some(Creation { kind: "create", source: None, credential: None })).await?;
        if response.status_code() != 201 {
            return Err(unexpected(response));
        }
        metadata(response, id).await
    }

    pub(crate) async fn delete(&self, id: &str) -> std::result::Result<(), CopyError> {
        let response = self.control(id, Method::Delete, None).await.map_err(|_| CopyError::Unknown)?;
        match response.status_code() {
            204 => Ok(()),
            _ => Err(copy_error(response)),
        }
    }

    pub(crate) async fn start_copy(&self, id: &str, source: Option<&str>, url: &str) -> std::result::Result<(), CopyError> {
        self.start_copy_with_credential(id, source, url, None).await
    }

    pub(crate) async fn start_copy_with_credential(
        &self, id: &str, source: Option<&str>, url: &str, credential: Option<&ImportCredential<'_>>,
    ) -> std::result::Result<(), CopyError> {
        if credential.is_some() && matches!(self, Self::Durable(_)) {
            return Err(CopyError::Rejected);
        }
        let creation = match source {
            Some(source) => Creation { kind: "fork", source: Some(source), credential },
            None => Creation { kind: "import", source: Some(url), credential },
        };
        let response = self.control(id, Method::Post, Some(creation)).await.map_err(|_| CopyError::Unknown)?;
        match response.status_code() {
            201 | 202 => {
                discard(response);
                Ok(())
            }
            _ => Err(copy_error(response)),
        }
    }

    pub(crate) async fn copy_info(&self, id: &str) -> std::result::Result<CreatedRepo, CopyError> {
        let response = self.control(id, Method::Get, None).await.map_err(|_| CopyError::Unknown)?;
        if response.status_code() != 200 {
            return Err(copy_error(response));
        }
        metadata(response, id).await.map_err(|_| CopyError::Unknown)
    }
}

impl GitServiceRepo {
    pub(crate) fn is_durable(&self) -> bool {
        matches!(self.client, GitService::Durable(_))
    }

    pub(crate) async fn source_archive(&self, commit: &str, name: &str) -> Result<Option<Response>> {
        if !matches!(self.client, GitService::R2(_)) {
            return Ok(None);
        }
        let response = self.read("archive", &[("oid", commit), ("name", name)]).await?;
        if response.status_code() == 200 {
            return Ok(Some(response));
        }
        discard(response);
        Ok(None)
    }

    pub(crate) fn shares_comparison_backend(&self, source: &Self) -> bool {
        match (&self.client, &source.client) {
            (GitService::Native { origin: left, .. }, GitService::Native { origin: right, .. }) => left == right,
            (GitService::R2(_), GitService::R2(_)) => true,
            _ => false,
        }
    }

    pub(crate) async fn r2_control(&self, endpoint: &str, payload: &impl Serialize) -> Result<Response> {
        if !matches!(self.client, GitService::R2(_)) {
            return Err(Error::RustError("Invalid R2 Git backend".into()));
        }
        let url = format!("{}/v1/r2/{}/{endpoint}", self.client.origin(), self.id);
        self.client.send(Some(&self.id), &url, json_post(payload)?).await
    }

    async fn read(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<Response> {
        let mut url = Url::parse(&format!("{}/v1/repos/{}/{endpoint}", self.client.origin(), self.id))?;
        url.query_pairs_mut().extend_pairs(params.iter().copied());
        self.client.send(Some(&self.id), url.as_str(), RequestInit::default()).await
    }

    pub(crate) fn comparison_pair<'a>(target: &'a RepoHandle, source: &'a RepoHandle) -> Option<(&'a Self, &'a Self)> {
        match (target, source) {
            (RepoHandle::Service(target), RepoHandle::Service(source))
                if target.shares_comparison_backend(source) =>
            {
                Some((target, source))
            }
            _ => None,
        }
    }

    async fn snapshot_read<T: serde::de::DeserializeOwned>(
        &self, endpoint: &str, params: &[(&str, &str)],
    ) -> Result<SnapshotRead<T>> {
        let response = self.read(endpoint, params).await?;
        match response.status_code() {
            404 if response.headers().get("X-Shard-Comparison")?.as_deref() == Some("1") => {
                discard(response);
                // Only the comparison endpoint can report a missing snapshot.
                Ok(SnapshotRead::Missing)
            }
            413 => {
                discard(response);
                Ok(SnapshotRead::Limited)
            }
            200 => {
                let length = response_length(&response, "Content-Length")?;
                if length > MAX_COMPARISON_BYTES {
                    discard(response);
                    return Ok(SnapshotRead::Limited);
                }
                let bytes = file_bytes(response, length).await?;
                Ok(SnapshotRead::Found(serde_json::from_slice(&bytes)?))
            }
            _ => Err(unexpected(response)),
        }
    }

    pub(crate) async fn merge_bases(&self, source: &Self, base: &str, head: &str) -> Result<SnapshotRead<Vec<String>>> {
        self.snapshot_read("merge-base", &[("source", &source.id), ("base", base), ("head", head)]).await
    }

    pub(crate) async fn file_changes<T: serde::de::DeserializeOwned>(
        &self, source: &Self, base: &str, head: &str,
    ) -> Result<SnapshotRead<T>> {
        self.snapshot_read("compare", &[("source", &source.id), ("base", base), ("head", head)]).await
    }

    pub(crate) async fn integrated(&self, tip: &str, base: &str, head: &str) -> Result<SnapshotRead<bool>> {
        self.snapshot_read("integrated", &[("tip", tip), ("base", base), ("head", head)]).await
    }

    async fn write(&self, endpoint: &str, payload: &impl Serialize) -> std::result::Result<Response, WriteError> {
        self.client.write(Some(&self.id), &format!("/v1/repos/{}/{endpoint}", self.id), payload).await
    }

    pub(crate) async fn write_ref(
        &self, reference: &str, new_oid: &str, old_oid: &str,
    ) -> std::result::Result<(), WriteError> {
        write_status(self.write("ref", &RefWrite { reference, new_oid, old_oid }).await?)
    }

    pub(crate) async fn set_head(&self, reference: &str) -> std::result::Result<(), WriteError> {
        write_status(self.write("head", &HeadWrite { reference }).await?)
    }

    pub(crate) async fn merge(
        &self, reference: &str, base_oid: &str, head_oid: &str, source_id: Option<&str>,
    ) -> std::result::Result<String, WriteError> {
        let payload = MergeWrite { inputs: MergeInputs { reference, base_oid, head_oid }, source: source_id };
        merge_result(self.write("merge", &payload).await?).await
    }

    pub(crate) async fn log(&self, reference: &str, limit: u32) -> Result<Vec<Commit>> {
        let response = self.read("log", &[("ref", reference), ("limit", &limit.to_string())]).await?;
        json(response).await
    }

    pub(crate) async fn read_commit(&self, hash: &str) -> Result<Option<CommitData>> {
        let response = self.read("commit", &[("oid", hash)]).await?;
        if response.status_code() == 404 {
            discard(response);
            return Ok(None);
        }
        let data: CommitData = json(response).await?;
        if data.commit.hash != hash || data.parents.iter().any(|hash| !is_object_id(hash)) {
            return Err(Error::RustError("Git service returned invalid commit metadata".into()));
        }
        Ok(Some(data))
    }

    pub(crate) async fn read_commit_bytes(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        let response = self.read("commit-raw", &[("oid", hash)]).await?;
        match response.status_code() {
            404 | 413 => {
                discard(response);
                Ok(None)
            }
            200 => {
                let length = response_length(&response, "Content-Length");
                match length {
                    Ok(length) if length <= MAX_COMMIT_BYTES as u64 => {
                        file_bytes(response, length).await.map(Some)
                    }
                    Ok(_) => {
                        discard(response);
                        Ok(None)
                    }
                    Err(error) => {
                        discard(response);
                        Err(error)
                    }
                }
            }
            _ => Err(unexpected(response)),
        }
    }

    pub(crate) async fn read_tree_bounded(&self, hash: &str, limit: u32) -> Result<Option<Vec<TreeEntry>>> {
        let response = self.read("tree", &[("oid", hash), ("limit", &limit.to_string())]).await?;
        if response.status_code() == 413 {
            discard(response);
            return Ok(None);
        }
        let entries: Vec<Entry> = json(response).await?;
        entries.into_iter().map(|entry| TreeEntry::new(entry.name, &entry.mode, entry.hash, entry.size))
            .collect::<Result<_>>().map(Some)
    }

    pub(crate) async fn read_blob_bounded(&self, hash: &str, limit: usize) -> Result<BlobRead> {
        let response = self.read("blob", &[("oid", hash), ("limit", &limit.to_string())]).await?;
        match response.status_code() {
            413 => {
                discard(response);
                Ok(BlobRead::TooLarge)
            }
            200 => {
                let length = response_length(&response, "Content-Length")?;
                if length > limit as u64 {
                    discard(response);
                    return Ok(BlobRead::TooLarge);
                }
                Ok(BlobRead::Bytes(file_bytes(response, length).await?))
            }
            _ => Err(unexpected(response)),
        }
    }

    pub(crate) async fn read_file(&self, reference: &str, path: &str, limit: Option<usize>) -> Result<Option<GitFile>> {
        let mut params = [("ref", reference), ("path", path), ("limit", "")];
        let bound;
        let params = match limit {
            Some(limit) => {
                bound = limit.to_string();
                params[2].1 = &bound;
                &params[..]
            }
            None => &params[..2],
        };
        let response = self.read("file", params).await?;
        match response.status_code() {
            200 => {
                let size = response_length(&response, "X-Shard-Object-Size")?;
                Ok(Some(GitFile::Stream { response, size }))
            }
            404 => {
                discard(response);
                Ok(None)
            }
            _ => Err(unexpected(response)),
        }
    }

    pub(crate) async fn git_request(&self, endpoint: &str, query: &str, init: RequestInit) -> Result<Response> {
        let mut url = format!("{}/v1/git/{}.git/{endpoint}", self.client.origin(), self.id);
        if !query.is_empty() {
            url.push('?');
            url.push_str(query);
        }
        self.client.send(Some(&self.id), &url, init).await
    }
}

fn json_post(payload: &impl Serialize) -> Result<RequestInit> {
    let init = RequestInit {
        method: Method::Post,
        body: Some(serde_json::to_string(payload)?.into()),
        ..Default::default()
    };
    init.headers.set("Content-Type", "application/json")?;
    Ok(init)
}

fn write_error(response: Response) -> WriteError {
    let status = response.status_code();
    discard(response);
    match status {
        404 => WriteError::Missing,
        409 => WriteError::Conflict,
        412 => WriteError::Stale,
        422 => WriteError::Rejected,
        501 => WriteError::Unsupported,
        _ => WriteError::Unavailable,
    }
}

fn write_status(response: Response) -> std::result::Result<(), WriteError> {
    if response.status_code() != 204 {
        return Err(write_error(response));
    }
    discard(response);
    Ok(())
}

async fn merge_result(mut response: Response) -> std::result::Result<String, WriteError> {
    if response.status_code() != 200 {
        return Err(write_error(response));
    }
    let result: MergeResult = response.json().await.map_err(|_| WriteError::Unavailable)?;
    if !is_object_id(&result.oid) || result.oid.bytes().all(|byte| byte == b'0') {
        return Err(WriteError::Unavailable);
    }
    Ok(result.oid)
}

pub(crate) fn git_origin(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "[::1]"));
    (url.host_str().is_some() && (url.scheme() == "https" || url.scheme() == "http" && loopback)
        && url.username().is_empty() && url.password().is_none() && url.path() == "/"
        && url.query().is_none() && url.fragment().is_none()).then(|| url.origin().ascii_serialization())
}

fn invalid_origin() -> Error {
    Error::RustError("NATIVE_GIT_ORIGIN must be an HTTPS origin or loopback HTTP origin".into())
}

fn valid_id(id: &str) -> bool {
    id.starts_with("shard-") && id.len() <= 128
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(crate) fn discard(response: Response) {
    if let ResponseBody::Stream(stream) = response.body() {
        let _ = stream.cancel();
    }
}

fn unexpected(response: Response) -> Error {
    let status = response.status_code();
    discard(response);
    Error::RustError(format!("Git service returned HTTP {status}"))
}

fn copy_error(response: Response) -> CopyError {
    let status = response.status_code();
    discard(response);
    match status {
        202 => CopyError::Pending,
        404 => CopyError::Missing,
        409 => CopyError::Collision,
        413 | 422 => CopyError::Rejected,
        _ => CopyError::Unknown,
    }
}

async fn metadata(mut response: Response, id: &str) -> Result<CreatedRepo> {
    let metadata: CreatedRepo = response.json().await?;
    if metadata.name != id || metadata.default_branch.is_empty() {
        return Err(Error::RustError("Git service returned invalid repository metadata".into()));
    }
    Ok(metadata)
}

async fn json<T: serde::de::DeserializeOwned>(mut response: Response) -> Result<T> {
    if response.status_code() != 200 {
        return Err(unexpected(response));
    }
    response.json().await
}

fn response_length(response: &Response, header: &str) -> Result<u64> {
    response.headers().get(header)?.and_then(|length| length.parse().ok())
        .ok_or_else(|| Error::RustError("Git service omitted its file size".into()))
}

pub(crate) async fn file_bytes(mut response: Response, length: u64) -> Result<Vec<u8>> {
    if response_length(&response, "Content-Length")? != length {
        discard(response);
        return Err(Error::RustError("Git service returned an unexpected file size".into()));
    }
    let bytes = response.bytes().await?;
    if bytes.len() as u64 != length {
        return Err(Error::RustError("Git service returned incomplete file bytes".into()));
    }
    Ok(bytes)
}
