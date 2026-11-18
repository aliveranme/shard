//! Thin Rust wrapper over the Cloudflare Artifacts Workers binding.
//!
//! The binding is a JS RPC capability (JSRPC), so methods are reached through
//! reflection and awaited through `JsFuture`. Everything returns owned Rust
//! data; no JS handle escapes this module.
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use serde_json::Value;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::*;

#[derive(Debug, Clone)]
pub struct CreatedRepo {
    pub name: String,
    pub remote: String,
    pub default_branch: String,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub hash: String,
    pub tree: Option<String>,
    pub message: String,
    pub author: String,
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub name: String,
    pub is_dir: bool,
    pub hash: String,
    pub size: Option<u64>,
    pub mode: &'static str,
}

pub enum BlobRead {
    Bytes(Vec<u8>),
    TooLarge,
}

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

    pub async fn delete(&self, name: &str) -> Result<bool> {
        let obj = call_async(&self.obj, "delete", &[JsValue::from_str(name)]).await?;
        Ok(obj.as_bool().unwrap_or(false))
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

    pub async fn read_file(&self, git_ref: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let opts = Object::new();
        Reflect::set(&opts, &JsValue::from_str("ref"), &JsValue::from_str(git_ref))
            .map_err(js_error)?;
        Reflect::set(&opts, &JsValue::from_str("path"), &JsValue::from_str(path))
            .map_err(js_error)?;
        let blob = call_async(&self.obj, "readFile", &[opts.into()]).await?;
        if blob.is_null() || blob.is_undefined() {
            return Ok(None);
        }
        let buffer = call_async(&blob, "arrayBuffer", &[]).await?;
        let bytes = Uint8Array::new(&buffer).to_vec();
        Ok(Some(bytes))
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

    pub async fn read_tree(&self, hash: &str) -> Result<Vec<TreeEntry>> {
        self.read_tree_bounded(hash, u32::MAX).await?.ok_or_else(|| {
            Error::RustError("Git tree exceeds the entry limit".into())
        })
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
            let name = field_string(&item, &["name"])
                .filter(|name| !name.is_empty() && !name.contains('/') && name != "." && name != "..");
            let mode = field_string(&item, &["mode"]).and_then(|mode| match mode.as_str() {
                "040000" | "40000" => Some("040000"),
                "100644" => Some("100644"),
                "100755" => Some("100755"),
                "120000" => Some("120000"),
                "160000" => Some("160000"),
                _ => None,
            });
            let hash = field_string(&item, &["hash", "sha", "id"]).filter(|id| is_object_id(id));
            let (Some(name), Some(mode), Some(hash)) = (name, mode, hash) else {
                return Err(Error::RustError("Invalid Git tree entry".into()));
            };
            entries.push(TreeEntry {
                name,
                is_dir: mode == "040000",
                mode,
                hash,
                size: Reflect::get(&item, &JsValue::from_str("size"))
                    .ok().and_then(|value| value.as_f64()).map(|size| size as u64),
            });
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

    pub async fn head_commit(&self, git_ref: &str) -> Result<Option<Commit>> {
        Ok(self.log(git_ref, 1).await?.into_iter().next())
    }

}

pub(crate) fn is_object_id(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

async fn call_async(obj: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue> {
    let function = Reflect::get(obj, &JsValue::from_str(method))
        .map_err(js_error)?
        .dyn_into::<Function>()
        .map_err(|_| {
            Error::RustError(format!(
                "Artifacts binding has no `{method}` method (binding not deployed?)"
            ))
        })?;
    let call_args = Array::new();
    for arg in args {
        call_args.push(arg);
    }
    let output = function.apply(obj, &call_args).map_err(js_error)?;
    let promise = output.dyn_into::<Promise>().map_err(|_| {
        Error::RustError(format!("Artifacts `{method}` did not return a promise"))
    })?;
    JsFuture::from(promise).await.map_err(js_error)
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
