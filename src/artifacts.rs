//! Thin Rust wrapper over the Cloudflare Artifacts Workers binding.
//!
//! The binding is a JS RPC capability (JSRPC), so methods are reached through
//! reflection and awaited through `JsFuture`. Everything returns owned Rust
//! data; no JS handle escapes this module.
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use serde_json::Value;
use std::collections::BTreeMap;
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
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub status: &'static str,
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
        Ok(items.iter().filter_map(parse_commit).collect())
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
        let value = call_async(&self.obj, "readCommit", &[JsValue::from_str(hash)]).await?;
        if value.is_null() || value.is_undefined() {
            return Ok(None);
        }
        Ok(parse_commit(&to_json(&value)?))
    }

    pub async fn read_tree(&self, hash: &str) -> Result<Vec<TreeEntry>> {
        let value = call_async(&self.obj, "readTree", &[JsValue::from_str(hash)]).await?;
        if value.is_null() || value.is_undefined() {
            return Ok(Vec::new());
        }
        let json = to_json(&value)?;
        let serde_json::Value::Array(items) = json else {
            return Ok(Vec::new());
        };
        Ok(items
            .iter()
            .filter_map(|item| {
                let name = item.get("name")?.as_str()?.to_string();
                let mode = item
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let kind = item
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let hash = item
                    .get("hash")
                    .or_else(|| item.get("sha"))
                    .or_else(|| item.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                Some(TreeEntry {
                    name,
                    is_dir: kind == "tree" || mode.starts_with("04") || mode == "40000",
                    hash,
                    size: item
                        .get("size")
                        .and_then(|v| v.as_u64()),
                })
            })
            .collect())
    }

    pub async fn head_commit(&self, git_ref: &str) -> Result<Option<Commit>> {
        Ok(self.log(git_ref, 1).await?.into_iter().next())
    }

    /// File-level diff between two refs. Artifacts has no diff API, so this
    /// walks both trees and compares object ids. Depth and entry counts are
    /// bounded to keep a single request within Workers limits.
    pub async fn changed_files(&self, base_ref: &str, head_ref: &str) -> Result<Vec<ChangedFile>> {
        let base = self.head_commit(base_ref).await?;
        let head = self.head_commit(head_ref).await?;
        let (Some(base), Some(head)) = (base, head) else {
            return Ok(Vec::new());
        };
        if base.tree.is_none() || head.tree.is_none() || base.hash == head.hash {
            return Ok(Vec::new());
        }

        let mut base_files = BTreeMap::new();
        let mut head_files = BTreeMap::new();
        fill_tree(self, base.tree.as_deref().unwrap(), "", 0, &mut base_files).await?;
        fill_tree(self, head.tree.as_deref().unwrap(), "", 0, &mut head_files).await?;

        let mut changed = Vec::new();
        for (path, hash) in &head_files {
            match base_files.get(path) {
                None => changed.push(ChangedFile {
                    path: path.clone(),
                    status: "added",
                }),
                Some(old) if old != hash => changed.push(ChangedFile {
                    path: path.clone(),
                    status: "modified",
                }),
                Some(_) => {}
            }
        }
        for path in base_files.keys() {
            if !head_files.contains_key(path) {
                changed.push(ChangedFile {
                    path: path.clone(),
                    status: "deleted",
                });
            }
        }
        changed.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(changed)
    }
}

async fn fill_tree(
    repo: &RepoHandle,
    tree_hash: &str,
    prefix: &str,
    depth: usize,
    out: &mut BTreeMap<String, String>,
) -> Result<()> {
    if depth > 12 || out.len() > 2000 || tree_hash.is_empty() {
        return Ok(());
    }
    for entry in repo.read_tree(tree_hash).await? {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{prefix}/{}", entry.name)
        };
        if entry.is_dir {
            // Recursion through an async fn needs boxing.
            Box::pin(fill_tree(repo, &entry.hash, &path, depth + 1, out)).await?;
        } else {
            out.insert(path, entry.hash);
        }
    }
    Ok(())
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

fn parse_commit(value: &Value) -> Option<Commit> {
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
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
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
        message,
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
