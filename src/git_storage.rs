use crate::artifacts::{self, Artifacts};
use crate::git_wire;
use crate::i18n::RepoText;
use crate::models::Repo;
use crate::native_git::{ArtifactsMergeTransport, GitService, GitServiceRepo, RemoteRepo, WriteError};
use js_sys::Uint8Array;
use serde::Deserialize;
use wasm_bindgen_futures::JsFuture;
use worker::web_sys::Blob;
use worker::{Env, Error, Fetch, Request, RequestInit, RequestRedirect, Response, ResponseBody, Result};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum GitBackend {
    Artifacts,
    Native,
    Durable,
}

impl GitBackend {
    pub(crate) fn selected(env: &Env) -> Result<Self> {
        match env.var("GIT_BACKEND").ok().map(|value| value.to_string()).as_deref() {
            None | Some("artifacts") => Ok(Self::Artifacts),
            Some("native") => Ok(Self::Native),
            Some("durable") => Ok(Self::Durable),
            Some(_) => Err(Error::RustError("GIT_BACKEND must be artifacts, native or durable".into())),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Artifacts => "artifacts",
            Self::Native => "native",
            Self::Durable => "durable",
        }
    }

    pub(crate) fn label(self) -> RepoText {
        match self {
            Self::Artifacts => RepoText::ArtifactsStorage,
            Self::Native => RepoText::NativeGitStorage,
            Self::Durable => RepoText::DurableGitStorage,
        }
    }
}

pub(crate) enum GitStorage {
    Artifacts(Artifacts),
    Service(GitService),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatedRepo {
    pub name: String,
    #[serde(default)]
    pub remote: String,
    pub default_branch: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Commit {
    pub hash: String,
    pub tree: Option<String>,
    pub message: String,
    pub author: String,
    pub author_email: Option<String>,
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct TreeEntry {
    pub name: String,
    pub is_dir: bool,
    pub hash: String,
    pub size: Option<u64>,
    pub mode: &'static str,
}

pub(crate) fn is_object_id(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

impl TreeEntry {
    pub(crate) fn new(name: String, mode: &str, hash: String, size: Option<u64>) -> Result<Self> {
        let mode = match mode {
            "040000" | "40000" => "040000",
            "100644" => "100644",
            "100755" => "100755",
            "120000" => "120000",
            "160000" => "160000",
            _ => return Err(Error::RustError("Invalid Git tree mode".into())),
        };
        if name.is_empty() || name.contains('/') || matches!(name.as_str(), "." | "..") || !is_object_id(&hash) {
            return Err(Error::RustError("Invalid Git tree entry".into()));
        }
        Ok(Self { name, is_dir: mode == "040000", hash, size, mode })
    }
}

pub(crate) enum BlobRead {
    Bytes(Vec<u8>),
    TooLarge,
}

pub(crate) enum GitFile {
    Artifacts(Blob),
    Stream { response: Response, size: u64 },
}

impl GitFile {
    pub(crate) fn size(&self) -> u64 {
        match self {
            Self::Artifacts(blob) => blob.size() as u64,
            Self::Stream { size, .. } => *size,
        }
    }

    pub(crate) async fn bytes(self, limit: usize) -> Result<Vec<u8>> {
        match self {
            Self::Artifacts(blob) => {
                let blob = if blob.size() > limit as f64 {
                    blob.slice_with_f64_and_f64(0.0, limit as f64).map_err(Error::from)?
                } else {
                    blob
                };
                let buffer = JsFuture::from(blob.array_buffer()).await.map_err(Error::from)?;
                Ok(Uint8Array::new(&buffer).to_vec())
            }
            Self::Stream { response, size } => {
                crate::native_git::file_bytes(response, size.min(limit as u64)).await
            }
        }
    }

    pub(crate) fn discard(self) {
        if let Self::Stream { response, .. } = self {
            crate::native_git::discard(response);
        }
    }

    pub(crate) fn into_response(self) -> Result<Response> {
        match self {
            Self::Artifacts(blob) => Response::from_body(ResponseBody::Stream(blob.stream())),
            Self::Stream { response, .. } => Ok(response),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CopyError {
    Pending,
    Missing,
    Collision,
    Rejected,
    Unknown,
}

impl GitStorage {
    pub(crate) fn from_env(env: &Env, backend: GitBackend) -> Result<Self> {
        match backend {
            GitBackend::Artifacts => Artifacts::from_env(env).map(Self::Artifacts),
            GitBackend::Native => GitService::from_env(env).map(Self::Service),
            GitBackend::Durable => GitService::durable(env).map(Self::Service),
        }
    }

    pub(crate) async fn create(&self, id: &str, description: &str) -> Result<CreatedRepo> {
        match self {
            Self::Artifacts(storage) => storage.create(id, description).await,
            Self::Service(storage) => storage.create(id).await,
        }
    }

    pub(crate) async fn get(self, id: &str) -> Result<RepoHandle> {
        match self {
            Self::Artifacts(storage) => storage.get(id).await.map(RepoHandle::Artifacts),
            Self::Service(storage) => storage.repo(id).map(RepoHandle::Service),
        }
    }

    pub(crate) async fn delete(&self, id: &str) -> std::result::Result<(), CopyError> {
        let result = match self {
            Self::Artifacts(storage) => storage.delete(id).await,
            Self::Service(storage) => storage.delete(id).await,
        };
        match result {
            Err(CopyError::Missing) => Ok(()),
            result => result,
        }
    }

    pub(crate) async fn start_copy(
        &self, id: &str, description: &str, source: Option<&str>, url: &str,
    ) -> std::result::Result<(), CopyError> {
        match self {
            Self::Artifacts(storage) => storage.start_copy(id, description, source, url).await,
            Self::Service(storage) => storage.start_copy(id, source, url).await,
        }
    }

    pub(crate) async fn copy_info(&self, id: &str) -> std::result::Result<CreatedRepo, CopyError> {
        match self {
            Self::Artifacts(storage) => storage.copy_info(id).await,
            Self::Service(storage) => storage.copy_info(id).await,
        }
    }
}

pub(crate) enum RepoHandle {
    Artifacts(artifacts::RepoHandle),
    Service(GitServiceRepo),
}

impl RepoHandle {
    pub(crate) async fn open(env: &Env, repo: &Repo) -> Result<Self> {
        GitStorage::from_env(env, repo.git_backend)?.get(&repo.storage_id).await
    }

    pub(crate) async fn log(&self, git_ref: &str, limit: u32) -> Result<Vec<Commit>> {
        match self {
            Self::Artifacts(repo) => repo.log(git_ref, limit).await,
            Self::Service(repo) => repo.log(git_ref, limit).await,
        }
    }

    pub(crate) async fn head_commit(&self, git_ref: &str) -> Result<Option<Commit>> {
        Ok(self.log(git_ref, 1).await?.into_iter().next())
    }

    pub(crate) async fn read_commit(&self, hash: &str) -> Result<Option<Commit>> {
        match self {
            Self::Artifacts(repo) => repo.read_commit(hash).await,
            Self::Service(repo) => Ok(repo.read_commit(hash).await?.map(|value| value.commit)),
        }
    }

    pub(crate) async fn commit_parents(&self, hash: &str) -> Result<Option<Vec<String>>> {
        match self {
            Self::Artifacts(repo) => repo.commit_parents(hash).await,
            Self::Service(repo) => Ok(repo.read_commit(hash).await?.map(|value| value.parents)),
        }
    }

    pub(crate) async fn read_tree(&self, hash: &str) -> Result<Vec<TreeEntry>> {
        self.read_tree_bounded(hash, u32::MAX).await?.ok_or_else(|| {
            Error::RustError("Git tree exceeds the entry limit".into())
        })
    }

    pub(crate) async fn read_tree_bounded(&self, hash: &str, limit: u32) -> Result<Option<Vec<TreeEntry>>> {
        match self {
            Self::Artifacts(repo) => repo.read_tree_bounded(hash, limit).await,
            Self::Service(repo) => repo.read_tree_bounded(hash, limit).await,
        }
    }

    pub(crate) async fn read_blob_bounded(&self, hash: &str, limit: usize) -> Result<BlobRead> {
        match self {
            Self::Artifacts(repo) => repo.read_blob_bounded(hash, limit).await,
            Self::Service(repo) => repo.read_blob_bounded(hash, limit).await,
        }
    }

    pub(crate) async fn read_file(&self, git_ref: &str, path: &str, limit: Option<usize>) -> Result<Option<GitFile>> {
        match self {
            Self::Artifacts(repo) => Ok(repo.read_file(git_ref, path).await?.map(GitFile::Artifacts)),
            Self::Service(repo) => repo.read_file(git_ref, path, limit).await,
        }
    }

    pub(crate) async fn write_ref(&self, reference: &str, before: &str, after: &str) -> std::result::Result<(), WriteError> {
        if let Self::Service(repo) = self {
            return repo.write_ref(reference, after, before).await;
        }
        let body = git_wire::update_request(reference, before, after).await.map_err(|_| WriteError::Unavailable)?;
        let init = RequestInit { method: worker::Method::Post, body: Some(body), ..Default::default() };
        init.headers.set("Content-Type", "application/x-git-receive-pack-request").map_err(|_| WriteError::Unavailable)?;
        let response = self.git_request("git-receive-pack", "", true, "", init).await.map_err(|_| WriteError::Unavailable)?;
        if response.status_code() != 200 {
            return Err(WriteError::Unavailable);
        }
        let ResponseBody::Stream(body) = response.body() else { return Err(WriteError::Unavailable) };
        let applied = git_wire::receive_report(body, false).await.map_err(|_| WriteError::Unavailable)?;
        if applied.iter().any(|name| name == reference) { Ok(()) } else { Err(WriteError::Stale) }
    }

    pub(crate) async fn set_default_branch(&self, branch: &str) -> std::result::Result<bool, WriteError> {
        match self {
            Self::Service(repo) => {
                repo.set_head(&format!("refs/heads/{branch}")).await?;
                Ok(false)
            }
            Self::Artifacts(_) => Ok(true),
        }
    }

    pub(crate) async fn merge_saved(
        &self, env: &Env, reference: &str, base: &str, head: &str, fast_forward: bool,
        source: Option<(&Self, &str)>,
    ) -> std::result::Result<String, WriteError> {
        match (self, source) {
            (Self::Service(repo), None) if !repo.is_durable() => repo.merge(reference, base, head, None).await,
            (Self::Service(repo), Some((Self::Service(source), id))) if !repo.is_durable() && !source.is_durable() => {
                repo.merge(reference, base, head, Some(id)).await
            }
            (_, None) if fast_forward => {
                self.write_ref(reference, base, head).await?;
                Ok(head.to_owned())
            }
            (Self::Artifacts(repo), source @ (None | Some((Self::Artifacts(_), _)))) => {
                let transport = ArtifactsMergeTransport::from_env(env).await?;
                let remote = repo.remote().await.map_err(|_| WriteError::Unavailable)?;
                let token = repo.create_token("write", 300).await.map_err(|_| WriteError::Unavailable)?;
                let target = RemoteRepo { remote: &remote, token: &token };
                let source_credentials = if let Some((Self::Artifacts(source), _)) = source {
                    Some((source.remote().await.map_err(|_| WriteError::Unavailable)?,
                        source.create_token("read", 300).await.map_err(|_| WriteError::Unavailable)?))
                } else {
                    None
                };
                let source = source_credentials.as_ref().map(|(remote, token)| RemoteRepo { remote, token });
                transport.merge_remote(reference, base, head, &target, source.as_ref()).await
            }
            _ => Err(WriteError::Unsupported),
        }
    }

    pub(crate) async fn git_request(
        &self, endpoint: &str, query: &str, write: bool, saved_remote: &str, mut init: RequestInit,
    ) -> Result<Response> {
        let (remote, authorization) = match self {
            Self::Artifacts(repo) => {
                let token = repo.create_token(if write { "write" } else { "read" }, 3600).await?;
                let remote = match repo.remote().await {
                    Ok(remote) => remote,
                    Err(_) if !saved_remote.is_empty() => saved_remote.to_owned(),
                    Err(error) => return Err(error),
                };
                (remote, format!("Bearer {token}"))
            }
            Self::Service(repo) => return repo.git_request(endpoint, query, init).await,
        };
        let mut url = format!("{}/{endpoint}", remote.trim_end_matches('/'));
        if !query.is_empty() {
            url.push('?');
            url.push_str(query);
        }
        init.headers.set("Authorization", &authorization)?;
        init.redirect = RequestRedirect::Manual;
        Fetch::Request(Request::new_with_init(&url, &init)?).send().await
    }
}
