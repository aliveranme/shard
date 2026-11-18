//! Thin Rust wrapper over the Cloudflare Artifacts Workers binding.
//!
//! The binding is a JS RPC capability (JSRPC), so methods are reached through
//! reflection and awaited through `JsFuture`.
use crate::git_storage::{is_object_id, BlobRead, Commit, CopyError, CreatedRepo, TreeEntry};
use js_sys::{Array, Function, Object, Promise, Reflect, Symbol, Uint8Array};
use serde_json::Value;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::*;
use worker::web_sys::Blob;

pub struct Artifacts {
    obj: JsValue,
}

pub struct RepoHandle {
    obj: JsValue,
}

impl Artifacts {
    pub fn from_env(env: &Env) -> Result<Self> {
        let obj = Reflect::get(env.as_ref(), &JsValue::from_str("ARTIFACTS"))
            .map_err(js_error)?;
        if obj.is_undefined() || obj.is_null() {
            return Err(Error::RustError(
                "ARTIFACTS binding is not configured in wrangler.jsonc".into(),
            ));
        }
        Ok(Self { obj })
    }

    pub async fn create(&self, name: &str, description: &str) -> Result<CreatedRepo> {
        let opts = Object::new();
        Reflect::set(
            &opts,
            &JsValue::from_str("description"),
            &JsValue::from_str(description),
        )
        .map_err(js_error)?;
        Reflect::set(
            &opts,
            &JsValue::from_str("setDefaultBranch"),
            &JsValue::from_str("main"),
        )
        .map_err(js_error)?;
        Reflect::set(
            &opts,
            &JsValue::from_str("readOnly"),
            &JsValue::from_bool(false),
        )
        .map_err(js_error)?;

        let created = call_async(
            &self.obj,
            "create",
            &[JsValue::from_str(name), opts.into()],
        )
        .await?;
        Ok(CreatedRepo {
            name: field_string(&created, &["name"]).unwrap_or_else(|| name.to_string()),
            remote: field_string(&created, &["remote"]).unwrap_or_default(),
            default_branch: field_string(&created, &["defaultBranch", "default_branch"])
                .unwrap_or_else(|| "main".into()),
        })
    }

    pub async fn get(&self, name: &str) -> Result<RepoHandle> {
        let obj = call_async(&self.obj, "get", &[JsValue::from_str(name)]).await?;
        Ok(RepoHandle { obj })
    }

    pub(crate) async fn delete(&self, name: &str) -> std::result::Result<(), CopyError> {
        call(&self.obj, "delete", &[JsValue::from_str(name)]).await.map_err(copy_error)?;
        Ok(())
    }

    pub(crate) async fn start_copy(
        &self, name: &str, description: &str, source: Option<&str>, url: &str,
    ) -> std::result::Result<(), CopyError> {
        let opts = Object::new();
        let set = |object: &Object, key: &str, value: &JsValue| {
            Reflect::set(object, &JsValue::from_str(key), value).map_err(copy_error)
        };
        set(&opts, "description", &JsValue::from_str(description))?;
        set(&opts, "readOnly", &JsValue::FALSE)?;
        if let Some(source) = source {
            set(&opts, "defaultBranchOnly", &JsValue::FALSE)?;
            let obj = call(&self.obj, "get", &[JsValue::from_str(source)]).await.map_err(copy_error)?;
            let repo = RepoHandle { obj };
            call(&repo.obj, "fork", &[JsValue::from_str(name), opts.into()]).await.map_err(copy_error)?;
        } else {
            let upstream = Object::new();
            set(&upstream, "url", &JsValue::from_str(url))?;
            let target = Object::new();
            set(&target, "name", &JsValue::from_str(name))?;
            set(&target, "opts", &opts)?;
            let params = Object::new();
            set(&params, "source", &upstream)?;
            set(&params, "target", &target)?;
            call(&self.obj, "import", &[params.into()]).await.map_err(copy_error)?;
        }
        Ok(())
    }

    pub(crate) async fn copy_info(&self, name: &str) -> std::result::Result<CreatedRepo, CopyError> {
        let obj = call(&self.obj, "get", &[JsValue::from_str(name)]).await.map_err(copy_error)?;
        let repo = RepoHandle { obj };
        let info = call(&repo.obj, "info", &[]).await.map_err(copy_error)?;
        let field = |key| field_string(&info, &[key]).filter(|value| !value.is_empty()).ok_or(CopyError::Unknown);
        Ok(CreatedRepo { name: field("name")?, remote: field("remote")?, default_branch: field("defaultBranch")? })
    }
}

impl RepoHandle {
    /// Repository metadata. Only the fields shard needs are read, tolerantly:
    /// Artifacts may add fields without breaking the forge.
    pub async fn info(&self) -> Result<Value> {
        let value = call_async(&self.obj, "info", &[]).await?;
        to_json(&value)
    }

    pub async fn remote(&self) -> Result<String> {
        let info = self.info().await?;
        info.get("remote")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| Error::RustError("Artifacts repo metadata has no `remote`".into()))
    }

    pub async fn create_token(&self, scope: &str, ttl: u32) -> Result<String> {
        let result = call_async(
            &self.obj,
            "createToken",
            &[JsValue::from_str(scope), JsValue::from_f64(ttl as f64)],
        )
        .await?;
        field_string(&result, &["plaintext", "token"])
            .ok_or_else(|| Error::RustError("Artifacts returned no token plaintext".into()))
    }

    pub async fn log(&self, git_ref: &str, limit: u32) -> Result<Vec<Commit>> {
        let opts = Object::new();
        Reflect::set(&opts, &JsValue::from_str("ref"), &JsValue::from_str(git_ref))
            .map_err(js_error)?;
        Reflect::set(
            &opts,
            &JsValue::from_str("limit"),
            &JsValue::from_f64(limit as f64),
        )
        .map_err(js_error)?;
        let value = call_async(&self.obj, "log", &[opts.into()]).await?;
        let json = to_json(&value)?;
        let serde_json::Value::Array(items) = json else {
            return Ok(Vec::new());
        };
        Ok(items.iter().filter_map(|item| parse_commit(item, false)).collect())
    }

    pub async fn read_file(&self, git_ref: &str, path: &str) -> Result<Option<Blob>> {
        let opts = Object::new();
        Reflect::set(&opts, &JsValue::from_str("ref"), &JsValue::from_str(git_ref))
            .map_err(js_error)?;
        Reflect::set(&opts, &JsValue::from_str("path"), &JsValue::from_str(path))
            .map_err(js_error)?;
        let blob = call_async(&self.obj, "readFile", &[opts.into()]).await?;
        if blob.is_null() || blob.is_undefined() {
            return Ok(None);
        }
        let blob = blob.dyn_into::<Blob>()
            .map_err(|_| Error::RustError("Artifacts returned an invalid file blob".into()))?;
        Ok(Some(blob))
    }

    pub async fn read_commit(&self, hash: &str) -> Result<Option<Commit>> {
        let Some(data) = self.read_commit_data(hash).await? else { return Ok(None) };
        let commit = parse_commit(&data, true).filter(|commit| commit.hash == hash)
            .ok_or_else(|| Error::RustError("Artifacts returned mismatched commit metadata".into()))?;
        Ok(Some(commit))
    }

    async fn read_commit_data(&self, hash: &str) -> Result<Option<Value>> {
        let value = call_async(&self.obj, "readCommit", &[JsValue::from_str(hash)]).await?;
        if value.is_null() || value.is_undefined() {
            return Ok(None);
        }
        Ok(Some(to_json(&value)?))
    }

    pub async fn commit_parents(&self, hash: &str) -> Result<Option<Vec<String>>> {
        let Some(mut data) = self.read_commit_data(hash).await? else { return Ok(None) };
        if data.get("hash").and_then(Value::as_str) != Some(hash) {
            return Err(Error::RustError("Artifacts returned mismatched commit metadata".into()));
        }
        let parents = std::mem::take(&mut data["parents"]);
        let parents = serde_json::from_value::<Vec<String>>(parents)
            .map_err(|_| Error::RustError("Git commit parent list is unavailable".into()))?;
        if parents.iter().any(|id| !is_object_id(id)) {
            return Err(Error::RustError("Invalid Git commit parent".into()));
        }
        Ok(Some(parents))
    }

    /// `None` means the budget is exhausted. Missing objects are errors.
    pub async fn read_tree_bounded(&self, hash: &str, limit: u32) -> Result<Option<Vec<TreeEntry>>> {
        let value = call_async(&self.obj, "readTree", &[JsValue::from_str(hash)]).await?;
        if !Array::is_array(&value) {
            return Err(Error::RustError("Git tree is missing or invalid".into()));
        }
        let items: Array = value.unchecked_into();
        if items.length() > limit {
            return Ok(None);
        }
        let mut entries = Vec::with_capacity(items.length() as usize);
        for item in items.iter() {
            let name = field_string(&item, &["name"]).unwrap_or_default();
            let mode = field_string(&item, &["mode"]).unwrap_or_default();
            let hash = field_string(&item, &["hash", "sha", "id"]).unwrap_or_default();
            let size = Reflect::get(&item, &JsValue::from_str("size"))
                .ok().and_then(|value| value.as_f64()).map(|size| size as u64);
            entries.push(TreeEntry::new(name, &mode, hash, size)?);
        }
        Ok(Some(entries))
    }

    pub async fn read_blob_bounded(&self, hash: &str, limit: usize) -> Result<BlobRead> {
        let blob = call_async(&self.obj, "readBlob", &[JsValue::from_str(hash)]).await?;
        if blob.is_null() || blob.is_undefined() {
            return Err(Error::RustError("Git blob is missing".into()));
        }
        let size = Reflect::get(&blob, &JsValue::from_str("size"))
            .map_err(js_error)?.as_f64()
            .ok_or_else(|| Error::RustError("Git blob size is unavailable".into()))?;
        if size > limit as f64 {
            return Ok(BlobRead::TooLarge);
        }
        let buffer = call_async(&blob, "arrayBuffer", &[]).await?;
        Ok(BlobRead::Bytes(Uint8Array::new(&buffer).to_vec()))
    }

}

impl Drop for RepoHandle {
    fn drop(&mut self) {
        if let Some(dispose) = Reflect::get(&self.obj, Symbol::dispose().as_ref())
            .ok().and_then(|method| method.dyn_into::<Function>().ok())
        {
            let _ = dispose.call0(&self.obj);
        }
    }
}

fn copy_error(error: JsValue) -> CopyError {
    match field_string(&error, &["code"]).as_deref() {
        Some("IMPORT_IN_PROGRESS" | "FORK_IN_PROGRESS") => CopyError::Pending,
        Some("NOT_FOUND") => CopyError::Missing,
        Some("ALREADY_EXISTS") => CopyError::Collision,
        Some("INVALID_INPUT" | "INVALID_REPO_NAME" | "INVALID_URL" | "REMOTE_AUTH_REQUIRED" | "UPSTREAM_UNAVAILABLE" | "MEMORY_LIMIT") => CopyError::Rejected,
        _ => CopyError::Unknown,
    }
}

async fn call_async(obj: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue> {
    call(obj, method, args).await.map_err(js_error)
}

async fn call(obj: &JsValue, method: &str, args: &[JsValue]) -> std::result::Result<JsValue, JsValue> {
    let function = Reflect::get(obj, &JsValue::from_str(method))?.dyn_into::<Function>()
        .map_err(|_| js_sys::Error::new(&format!("binding has no `{method}` method")))?;
    let call_args = Array::new();
    for arg in args {
        call_args.push(arg);
    }
    let promise = function.apply(obj, &call_args)?.dyn_into::<Promise>()
        .map_err(|_| js_sys::Error::new(&format!("`{method}` did not return a promise")))?;
    JsFuture::from(promise).await
}

fn field_string(obj: &JsValue, names: &[&str]) -> Option<String> {
    for name in names {
        let value = Reflect::get(obj, &JsValue::from_str(name)).ok()?;
        if let Some(s) = value.as_string() {
            return Some(s);
        }
    }
    None
}

fn to_json(value: &JsValue) -> Result<Value> {
    let text = js_sys::JSON::stringify(value)
        .map_err(js_error)?
        .as_string()
        .unwrap_or_default();
    serde_json::from_str(&text)
        .map_err(|e| Error::RustError(format!("invalid Artifacts JSON: {e}")))
}

fn parse_commit(value: &Value, full_message: bool) -> Option<Commit> {
    let hash = value
        .get("hash")
        .or_else(|| value.get("sha"))
        .or_else(|| value.get("oid"))
        .or_else(|| value.get("id"))
        .and_then(|v| v.as_str())?
        .to_string();
    let message = value
        .get("message")
        .or_else(|| value.get("summary"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let message = if full_message { message } else { message.lines().next().unwrap_or_default() };
    let author = value.get("author").and_then(|a| {
        if let Some(name) = a.as_str() {
            Some(name.to_string())
        } else {
            a.get("name").and_then(|n| n.as_str()).map(str::to_string)
        }
    });
    let timestamp = value
        .get("authoredAt")
        .or_else(|| value.get("committedAt"))
        .or_else(|| value.get("author").and_then(|a| a.get("timestamp")).or_else(|| value.get("timestamp")))
        .and_then(|v| {
            if let Some(n) = v.as_i64() {
                Some(if n > 10_000_000_000 { n / 1000 } else { n })
            } else {
                v.as_str()?.parse::<i64>().ok()
            }
        });
    let tree = value
        .get("treeHash")
        .or_else(|| value.get("tree"))
        .and_then(|t| {
            t.as_str()
                .map(str::to_string)
                .or_else(|| t.get("hash")?.as_str().map(str::to_string))
        });
    Some(Commit {
        hash,
        tree,
        message: message.to_string(),
        author: author.unwrap_or_else(|| "unknown".into()),
        timestamp,
    })
}

fn js_error(value: JsValue) -> Error {
    let detail = value
        .as_string()
        .or_else(|| Reflect::get(&value, &JsValue::from_str("message")).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| format!("{value:?}"));
    Error::RustError(format!("Artifacts: {detail}"))
}
