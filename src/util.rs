use wasm_bindgen::JsValue;
use worker::*;

pub fn now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

pub fn header(req: &Request, name: &str) -> Option<String> {
    req.headers().get(name).ok().flatten()
}

pub fn cookie(req: &Request, name: &str) -> Option<String> {
    let raw = header(req, "cookie")?;
    raw.split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

pub fn query_param(req: &Request, name: &str) -> Option<String> {
    let url = req.url().ok()?;
    url.query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

pub fn base_url(req: &Request) -> String {
    match req.url() {
        Ok(u) => {
            let host = u.host_str().unwrap_or("localhost");
            match u.port() {
                Some(port) => format!("{}://{}:{}", u.scheme(), host, port),
                None => format!("{}://{}", u.scheme(), host),
            }
        }
        Err(_) => "http://localhost".into(),
    }
}

pub fn is_secure(req: &Request) -> bool {
    req.url().map(|u| u.scheme() == "https").unwrap_or(false)
}

/// Redirect with a plain Location header so relative paths stay relative.
pub fn redirect(path: &str) -> Result<Response> {
    let headers = Headers::new();
    headers.set("Location", path)?;
    let mut resp = Response::empty()?.with_status(302).with_headers(headers);
    resp.headers_mut().set("Cache-Control", "no-store")?;
    Ok(resp)
}

pub fn set_cookie(token: &str, max_age: i64, secure: bool) -> Result<String> {
    Ok(format!(
        "shard_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    ))
}

pub fn clear_cookie() -> String {
    "shard_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0".into()
}

pub fn js_text(value: &str) -> JsValue {
    JsValue::from_str(value)
}

pub fn js_int(value: i64) -> JsValue {
    JsValue::from_f64(value as f64)
}

pub fn js_null() -> JsValue {
    JsValue::NULL
}

pub fn form_value(data: &FormData, name: &str) -> Option<String> {
    data.get_field(name).filter(|v| !v.is_empty())
}

pub fn form_bool(data: &FormData, name: &str) -> bool {
    matches!(
        data.get_field(name).as_deref(),
        Some("on") | Some("1") | Some("true")
    )
}

/// Normalize client IP addresses so IPv6 hosts within one `/64` share rate-limit and throttle buckets.
pub fn normalize_client_ip(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.to_string(),
        Ok(std::net::IpAddr::V6(v6)) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                v4.to_string()
            } else {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        }
        Err(_) => trimmed.to_owned(),
    }
}

/// Only allow local, absolute paths as post-login redirect targets.
/// Percent-encode non-ASCII or space bytes so `Location` is always a valid HTTP header value.
pub fn safe_next(next: Option<String>) -> String {
    match next {
        Some(n)
            if n.starts_with('/')
                && !n.starts_with("//")
                && !n.contains('\\')
                && !n.chars().any(char::is_control) =>
        {
            if n.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
                n
            } else {
                let mut encoded = String::with_capacity(n.len());
                for byte in n.bytes() {
                    if (0x21..=0x7e).contains(&byte) {
                        encoded.push(byte as char);
                    } else {
                        encoded.push_str(&format!("%{byte:02X}"));
                    }
                }
                encoded
            }
        }
        _ => "/".into(),
    }
}

/// Repository and user names: alphanumerics, dot, dash and underscore.
pub fn valid_slug(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 100 {
        return false;
    }
    let first = bytes[0] as char;
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    if name.ends_with('.') || name.contains("..") || name.to_ascii_lowercase().ends_with(".git") {
        return false;
    }
    bytes
        .iter()
        .all(|b| (*b as char).is_ascii_alphanumeric() || matches!(*b as char, '.' | '_' | '-'))
}

pub fn valid_email(email: &str) -> bool {
    if email.len() > 254 || email.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let mut parts = email.split('@');
    let (local, domain) = match (parts.next(), parts.next(), parts.next()) {
        (Some(l), Some(d), None) => (l, d),
        _ => return false,
    };
    !local.is_empty()
        && local.len() <= 64
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Percent-encode a branch or file path for use in a URL.
pub fn url_path(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

pub fn short_hash(hash: &str) -> String {
    hash.chars().take(7).collect()
}

pub fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}
