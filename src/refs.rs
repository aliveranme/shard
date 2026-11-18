use crate::artifacts::{is_object_id, RepoHandle};
use crate::models::Repo;
use crate::templates::Chrome;
use askama::Template;
use js_sys::{Reflect, Uint8Array};
use std::collections::BTreeMap;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use worker::web_sys::ReadableStreamDefaultReader;
use worker::{Fetch, Headers, Method, Request, RequestInit, RequestRedirect, ResponseBody};

const MAX_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 512;
const ADVERTISEMENT: &str = "application/x-git-upload-pack-advertisement";

#[derive(Template)]
#[template(path = "refs.html")]
pub(crate) struct RefsPage {
    pub chrome: Chrome,
    pub title: String,
    pub repo: Repo,
    pub is_owner: bool,
    pub tab: &'static str,
    pub inventory: Inventory,
    pub notice: Option<&'static str>,
}

#[derive(Default)]
pub(crate) struct Inventory {
    pub(crate) branches: Vec<Branch>,
    pub(crate) tags: Vec<Tag>,
}

pub(crate) struct Branch {
    pub(crate) name: String,
    pub(crate) oid: String,
}

pub(crate) struct Tag {
    pub(crate) name: String,
    pub(crate) oid: String,
    pub(crate) peeled: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RefsError {
    Limit,
    Unavailable,
}

fn unavailable(stage: &str) -> RefsError {
    worker::console_log!("repository references: {stage} failed");
    RefsError::Unavailable
}

pub(crate) async fn load(handle: &RepoHandle) -> Result<Inventory, RefsError> {
    let remote = handle.remote().await.map_err(|_| unavailable("resolve remote"))?;
    let token = handle.create_token("read", 3600).await.map_err(|_| unavailable("mint read token"))?;
    let url = format!("{}/info/refs?service=git-upload-pack", remote.trim_end_matches('/'));
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {token}")).map_err(|_| unavailable("set authorization header"))?;
    headers.set("Accept", ADVERTISEMENT).map_err(|_| unavailable("set accept header"))?;
    // Omit Git-Protocol so upload-pack advertises refs directly, without v2 negotiation.
    let init = RequestInit {
        method: Method::Get,
        headers,
        // Workers supports manual and follow. The status check below rejects redirects.
        redirect: RequestRedirect::Manual,
        ..Default::default()
    };
    let request = Request::new_with_init(&url, &init).map_err(|_| unavailable("construct discovery request"))?;
    let response = Fetch::Request(request).send().await.map_err(|_| unavailable("fetch advertisement"))?;
    let status = response.status_code();
    if status != 200 {
        worker::console_log!("repository references: advertisement returned HTTP {status}");
        return Err(RefsError::Unavailable);
    }
    let content_type = response.headers().get("content-type").map_err(|_| unavailable("read content type"))?;
    if !content_type.as_deref().is_some_and(|value| {
        value.split(';').next().is_some_and(|mime| mime.trim().eq_ignore_ascii_case(ADVERTISEMENT))
    }) {
        return Err(unavailable("validate advertisement content type"));
    }
    match response.body() {
        ResponseBody::Body(bytes) => parse_advertisement(bytes),
        ResponseBody::Stream(stream) => {
            let reader: ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
            let mut bytes = Vec::new();
            let done = JsValue::from_str("done");
            let value = JsValue::from_str("value");
            loop {
                let chunk = JsFuture::from(reader.read()).await.map_err(|_| unavailable("read advertisement stream"))?;
                if Reflect::get(&chunk, &done).ok().and_then(|done| done.as_bool()) == Some(true) {
                    return parse_advertisement(&bytes);
                }
                let chunk: Uint8Array = Reflect::get(&chunk, &value)
                    .map_err(|_| unavailable("access advertisement chunk"))?
                    .dyn_into().map_err(|_| unavailable("validate advertisement chunk"))?;
                let len = chunk.length() as usize;
                if len > MAX_BYTES - bytes.len() {
                    let _ = reader.cancel();
                    return Err(RefsError::Limit);
                }
                let offset = bytes.len();
                bytes.resize(offset + len, 0);
                chunk.copy_to(&mut bytes[offset..]);
            }
        }
        ResponseBody::Empty => Err(unavailable("read advertisement body")),
    }
}

fn parse_advertisement(bytes: &[u8]) -> Result<Inventory, RefsError> {
    parse(bytes).map_err(|error| match error {
        RefsError::Limit => RefsError::Limit,
        RefsError::Unavailable => unavailable("parse advertisement packets"),
    })
}

fn packet<'a>(bytes: &mut &'a [u8]) -> Result<Option<&'a [u8]>, RefsError> {
    let header = bytes.get(..4).ok_or(RefsError::Unavailable)?;
    if !header.iter().all(u8::is_ascii_hexdigit) {
        return Err(RefsError::Unavailable);
    }
    let len = usize::from_str_radix(std::str::from_utf8(header).map_err(|_| RefsError::Unavailable)?, 16)
        .map_err(|_| RefsError::Unavailable)?;
    if len == 0 {
        *bytes = &bytes[4..];
        return Ok(None);
    }
    if !(4..=65520).contains(&len) {
        return Err(RefsError::Unavailable);
    }
    let data = bytes.get(4..len).ok_or(RefsError::Unavailable)?;
    *bytes = &bytes[len..];
    Ok(Some(data.strip_suffix(b"\n").unwrap_or(data)))
}

fn parse(mut bytes: &[u8]) -> Result<Inventory, RefsError> {
    if bytes.len() > MAX_BYTES {
        return Err(RefsError::Limit);
    }
    let mut next = packet(&mut bytes)?;
    if next == Some(b"# service=git-upload-pack") {
        if packet(&mut bytes)?.is_some() {
            return Err(RefsError::Unavailable);
        }
        next = packet(&mut bytes)?;
    }
    if next == Some(b"version 1") {
        next = packet(&mut bytes)?;
    }

    let mut inventory = Inventory::default();
    let mut tags = BTreeMap::new();
    let mut peeled = BTreeMap::new();
    let mut records = 0;
    while let Some(data) = next {
        records += 1;
        if records > MAX_RECORDS {
            return Err(RefsError::Limit);
        }
        let line = data.splitn(2, |&b| b == 0).next().unwrap_or_default();
        let line = std::str::from_utf8(line).map_err(|_| RefsError::Unavailable)?;
        let (oid, name) = line.split_once(' ').ok_or(RefsError::Unavailable)?;
        if !is_object_id(oid) {
            return Err(RefsError::Unavailable);
        }
        if let Some(name) = name.strip_prefix("refs/heads/") {
            inventory.branches.push(Branch {
                name: name.to_owned(),
                oid: oid.to_owned(),
            });
        } else if let Some(name) = name.strip_prefix("refs/tags/") {
            if let Some(base) = name.strip_suffix("^{}") {
                peeled.insert(base, oid);
            } else {
                tags.insert(name, oid);
            }
        }
        next = packet(&mut bytes)?;
    }
    if !bytes.is_empty() {
        return Err(RefsError::Unavailable);
    }
    inventory.branches.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    inventory.tags = tags.into_iter().map(|(name, oid)| Tag {
        name: name.to_owned(),
        oid: oid.to_owned(),
        peeled: peeled.remove(name).map(str::to_owned),
    }).collect();
    Ok(inventory)
}
