use js_sys::{Reflect, Uint8Array};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{prelude::*, JsCast};
use worker::web_sys::ReadableStream;
use worker::{Error, Request, Response, ResponseBody, Result};

pub(crate) const ZERO_OID: &str = "0000000000000000000000000000000000000000";

#[wasm_bindgen(module = "/src/git_wire.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn shard_receive_request(
        body: &ReadableStream,
        encoding: &str,
        max_duration_ms: u32,
    ) -> std::result::Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn shard_receive_report(
        body: &ReadableStream,
        sideband: bool,
    ) -> std::result::Result<String, JsValue>;
    #[wasm_bindgen(catch)]
    async fn shard_default_head(
        body: &ReadableStream,
        branch: &str,
    ) -> std::result::Result<ReadableStream, JsValue>;
    #[wasm_bindgen(catch)]
    async fn shard_update_request(
        reference: &str,
        before: &str,
        after: &str,
    ) -> std::result::Result<Uint8Array, JsValue>;
}

#[derive(Deserialize, Serialize)]
pub(crate) struct RefUpdate {
    pub name: String,
    pub before: String,
    pub after: String,
}

pub(crate) struct ReceiveRequest {
    pub body: ReadableStream,
    pub updates: Vec<RefUpdate>,
    pub sideband: bool,
    pub report: bool,
}

impl ReceiveRequest {
    pub(crate) async fn inspect(
        req: &Request,
        encoding: &str,
        max_duration_ms: u32,
    ) -> Result<Self> {
        let body = req
            .inner()
            .body()
            .ok_or_else(|| Error::RustError("Missing Git request body".into()))?;
        let parsed = shard_receive_request(&body, encoding, max_duration_ms)
            .await
            .map_err(Error::from)?;
        let get = |name: &str| Reflect::get(&parsed, &JsValue::from_str(name)).map_err(Error::from);
        let updates = get("updates")?
            .as_string()
            .ok_or_else(|| Error::RustError("Missing Git push commands".into()))?;
        Ok(Self {
            body: get("body")?.unchecked_into(),
            updates: serde_json::from_str(&updates)?,
            sideband: get("sideband")?.as_bool() == Some(true),
            report: get("report")?.as_bool() == Some(true),
        })
    }
}

pub(crate) async fn receive_report(body: &ReadableStream, sideband: bool) -> Result<Vec<String>> {
    let accepted = shard_receive_report(body, sideband)
        .await
        .map_err(Error::from)?;
    serde_json::from_str(&accepted).map_err(Into::into)
}

pub(crate) async fn default_head(body: &ReadableStream, branch: &str) -> Result<ReadableStream> {
    shard_default_head(body, branch).await.map_err(Into::into)
}

pub(crate) async fn update_request(reference: &str, before: &str, after: &str) -> Result<JsValue> {
    Ok(shard_update_request(reference, before, after)
        .await
        .map_err(Error::from)?
        .into())
}

pub(crate) fn response_with_stream(response: Response, stream: ReadableStream) -> Result<Response> {
    Ok(Response::from_body(ResponseBody::Stream(stream))?
        .with_status(response.status_code())
        .with_headers(response.headers().clone()))
}

pub(crate) fn valid_ref(name: &str) -> bool {
    name.len() <= 255
        && !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('.')
        && !name.contains(['~', '^', ':', '?', '*', '[', '\\'])
        && !name.bytes().any(|byte| byte <= 32 || byte == 127)
        && !name.contains("..")
        && !name.contains("@{")
        && name != "@"
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}
