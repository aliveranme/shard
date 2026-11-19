//! Password, session and token cryptography.
//!
//! Hashing runs through WebCrypto rather than a Rust implementation: PBKDF2 at
//! 100k iterations is native on Workers and does not burn the Worker's CPU
//! budget the way a pure-Rust KDF would.
use wasm_bindgen::prelude::*;
use worker::*;

const PBKDF2_ITERATIONS: u32 = 100_000;

#[wasm_bindgen(inline_js = r#"
function shardToHex(buffer) {
  return Array.from(new Uint8Array(buffer))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

export function shard_random_hex(n) {
  const bytes = new Uint8Array(n);
  crypto.getRandomValues(bytes);
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

export async function shard_sha256_hex(text) {
  const data = new TextEncoder().encode(text);
  return shardToHex(await crypto.subtle.digest("SHA-256", data));
}

export async function shard_pbkdf2_hex(password, saltHex, iterations) {
  const salt = new Uint8Array(saltHex.match(/.{2}/g).map((b) => parseInt(b, 16)));
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(password),
    "PBKDF2",
    false,
    ["deriveBits"],
  );
  const bits = await crypto.subtle.deriveBits(
    { name: "PBKDF2", hash: "SHA-256", salt, iterations },
    key,
    256,
  );
  return shardToHex(bits);
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = shard_random_hex, catch)]
    fn random_hex_js(n: u32) -> Result<String, JsValue>;

    #[wasm_bindgen(js_name = shard_sha256_hex, catch)]
    async fn sha256_hex_js(text: &str) -> Result<String, JsValue>;

    #[wasm_bindgen(js_name = shard_pbkdf2_hex, catch)]
    async fn pbkdf2_hex_js(password: &str, salt_hex: &str, iterations: u32)
        -> Result<String, JsValue>;
}

fn js_err(context: &str, err: JsValue) -> Error {
    let detail = err
        .as_string()
        .unwrap_or_else(|| format!("{err:?}"));
    Error::RustError(format!("{context}: {detail}"))
}

/// 32 bytes of entropy as hex, used for session cookies and API tokens.
pub fn random_token() -> Result<String> {
    random_hex_js(32).map_err(|e| js_err("random token", e))
}

/// Salt used by the password KDF.
pub fn random_salt() -> Result<String> {
    random_hex_js(16).map_err(|e| js_err("random salt", e))
}

pub async fn sha256_hex(text: &str) -> Result<String> {
    sha256_hex_js(text).await.map_err(|e| js_err("sha256", e))
}

/// Formats as `pbkdf2_sha256$<iterations>$<salt>$<hash>`.
pub async fn hash_password(password: &str) -> Result<String> {
    let salt = random_salt()?;
    let hash = pbkdf2_hex_js(password, &salt, PBKDF2_ITERATIONS)
        .await
        .map_err(|e| js_err("pbkdf2", e))?;
    Ok(format!("pbkdf2_sha256${PBKDF2_ITERATIONS}${salt}${hash}"))
}

pub async fn verify_password(password: &str, stored: &str) -> Result<bool> {
    let mut parts = stored.split('$');
    let (Some("pbkdf2_sha256"), Some(iterations), Some(salt), Some(expected)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Ok(false);
    };
    let iterations: u32 = iterations.parse().unwrap_or(0);
    if iterations == 0 {
        return Ok(false);
    }
    let computed = pbkdf2_hex_js(password, salt, iterations)
        .await
        .map_err(|e| js_err("pbkdf2", e))?;
    Ok(constant_time_eq(computed.as_bytes(), expected.as_bytes()))
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
