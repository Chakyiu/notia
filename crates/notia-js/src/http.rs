//! HTTP for community plugins. A plugin asks, the request runs off the UI
//! thread, and the reply is delivered to the promise `requestUrl` returned.
//! Only `http` and `https` are allowed. The plugin is already something the
//! user chose to enable, which is the same trust as reading the vault.

use std::{collections::BTreeMap, time::Duration};

use serde::Deserialize;
use serde_json::{Map, Value, json};

/// How many requests one plugin may have open at once.
pub(crate) const MAX_IN_FLIGHT: usize = 8;
const MAX_URL: usize = 8 * 1024;
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_RESPONSE: u64 = 4 * 1024 * 1024;
const MAX_HEADERS: usize = 40;
const MAX_HEADER: usize = 8 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// What `requestUrl` sends. `timeout` is milliseconds.
#[derive(Deserialize)]
pub(crate) struct Request {
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) method: String,
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) timeout: Option<f64>,
}

pub(crate) struct Prepared {
    url: String,
    method: String,
    headers: BTreeMap<String, String>,
    body: String,
    timeout: Duration,
}

/// Checks the request and fills in the method and user agent.
pub(crate) fn prepare(request: Request) -> Result<Prepared, String> {
    let url = request.url.trim().to_string();
    check_url(&url)?;
    if request.body.len() > MAX_BODY {
        return Err("the request is too large".into());
    }
    if request.headers.len() > MAX_HEADERS {
        return Err("the request has too many headers".into());
    }
    let mut headers = BTreeMap::new();
    for (name, value) in request.headers {
        check_header(&name, &value)?;
        headers.insert(name, value);
    }
    if !headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("user-agent"))
    {
        headers.insert(
            "User-Agent".into(),
            format!("Notia/{}", env!("CARGO_PKG_VERSION")),
        );
    }

    let mut method = request.method.trim().to_ascii_uppercase();
    if method.is_empty() {
        method = if request.body.is_empty() {
            "GET".into()
        } else {
            "POST".into()
        };
    }
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
    ) {
        return Err(format!("“{method}” is not a method Notia can send"));
    }

    let timeout = match request.timeout {
        Some(ms) if ms.is_finite() && ms > 0. => {
            Duration::from_millis(ms.clamp(1_000., 180_000.) as u64)
        }
        _ => DEFAULT_TIMEOUT,
    };

    Ok(Prepared {
        url,
        method,
        headers,
        body: request.body,
        timeout,
    })
}

/// Performs `prepared` and returns `{ status, headers, text }`.
pub(crate) fn perform(mut prepared: Prepared) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(prepared.timeout))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();

    for _ in 0..4 {
        let mut response = send(&agent, &prepared).map_err(|error| {
            format!(
                "could not reach the server: {}",
                scrub(&prepared.url, &error.to_string())
            )
        })?;
        let status = response.status().as_u16();
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return read_body(status, &mut response);
        }
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .trim()
            .to_string();
        let _ = response.body_mut().with_config().limit(1024).read_to_vec();
        if matches!(status, 301 | 302 | 303) {
            prepared.method = "GET".into();
            prepared.body.clear();
        }
        prepared.url = resolve_redirect(&prepared.url, &location)?;
    }
    Err("the server redirected too many times".into())
}

fn apply_headers<Any>(
    mut request: ureq::RequestBuilder<Any>,
    headers: &BTreeMap<String, String>,
) -> ureq::RequestBuilder<Any> {
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request
}

fn send(
    agent: &ureq::Agent,
    prepared: &Prepared,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let headers = &prepared.headers;
    // A body and no body are different builder types, so each method sends
    // itself instead of sharing one builder.
    match prepared.method.as_str() {
        "POST" => {
            let request = apply_headers(agent.post(&prepared.url), headers);
            if prepared.body.is_empty() {
                request.send_empty()
            } else {
                request.send(prepared.body.as_str())
            }
        }
        "PUT" => {
            let request = apply_headers(agent.put(&prepared.url), headers);
            if prepared.body.is_empty() {
                request.send_empty()
            } else {
                request.send(prepared.body.as_str())
            }
        }
        "PATCH" => {
            let request = apply_headers(agent.patch(&prepared.url), headers);
            if prepared.body.is_empty() {
                request.send_empty()
            } else {
                request.send(prepared.body.as_str())
            }
        }
        "DELETE" => apply_headers(agent.delete(&prepared.url), headers).call(),
        "HEAD" => apply_headers(agent.head(&prepared.url), headers).call(),
        _ => apply_headers(agent.get(&prepared.url), headers).call(),
    }
}

fn read_body(
    status: u16,
    response: &mut ureq::http::Response<ureq::Body>,
) -> Result<Value, String> {
    let mut headers = Map::new();
    for (name, value) in response.headers() {
        let Ok(text) = value.to_str() else {
            continue;
        };
        if text.len() > MAX_HEADER || headers.len() >= MAX_HEADERS {
            continue;
        }
        headers.insert(
            name.as_str().to_ascii_lowercase(),
            Value::String(text.to_string()),
        );
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE)
        .read_to_vec()
        .map_err(|error| format!("could not read the reply: {error}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "the reply is not text".to_string())?;
    Ok(json!({
        "status": status,
        "headers": headers,
        "text": text,
    }))
}

fn check_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("the address is empty".into());
    }
    if url.len() > MAX_URL {
        return Err("the address is too long".into());
    }
    if url
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err("the address contains a space or a control character".into());
    }
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("the address must start with http:// or https://".into());
    }
    Ok(())
}

fn check_header(name: &str, value: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("“{name}” is not a header name"));
    }
    if value.len() > MAX_HEADER || value.bytes().any(|byte| byte == b'\r' || byte == b'\n') {
        return Err(format!("the {name} header is not usable"));
    }
    Ok(())
}

/// `location` must stay on http or https. Relative redirects are resolved
/// against the URL that redirected, without its query.
fn resolve_redirect(from: &str, location: &str) -> Result<String, String> {
    if location.is_empty() {
        return Err("the server redirected without an address".into());
    }
    let absolute = if location.to_ascii_lowercase().starts_with("http://")
        || location.to_ascii_lowercase().starts_with("https://")
    {
        location.to_string()
    } else if location.starts_with('/') {
        let scheme_end = from
            .find("://")
            .ok_or("the address must start with http:// or https://")?;
        let rest = &from[scheme_end + 3..];
        let host = rest.split('/').next().unwrap_or(rest);
        format!("{}://{host}{location}", &from[..scheme_end])
    } else {
        return Err("the server redirected somewhere that is not http".into());
    };
    check_url(&absolute)?;
    Ok(absolute)
}

/// Drops the query, so a key passed there is not written to the log.
fn scrub(url: &str, message: &str) -> String {
    let Some(query) = url.find('?') else {
        return message.to_string();
    };
    let shown = url[..query].to_string();
    message.replace(url, &shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str) -> Request {
        Request {
            url: url.into(),
            method: String::new(),
            headers: BTreeMap::new(),
            body: String::new(),
            timeout: None,
        }
    }

    #[test]
    fn only_http_addresses_are_sent() {
        assert!(prepare(request("file:///etc/passwd")).is_err());
        assert!(prepare(request("javascript:alert(1)")).is_err());
        assert!(prepare(request("https://example.com/v1 chat")).is_err());
        assert!(prepare(request("")).is_err());
        let prepared = prepare(request("https://example.com/v1")).unwrap();
        assert_eq!(prepared.method, "GET");
        assert!(prepared.headers.contains_key("User-Agent"));
    }

    #[test]
    fn a_body_selects_post_and_a_redirect_stays_on_http() {
        let mut with_body = request("http://127.0.0.1:9/v1");
        with_body.body = "{}".into();
        assert_eq!(prepare(with_body).unwrap().method, "POST");
        assert!(resolve_redirect("https://api.example/v1/chat", "file:///tmp/x").is_err());
        assert_eq!(
            resolve_redirect("https://api.example/v1/chat?key=secret", "/v1/models").unwrap(),
            "https://api.example/v1/models"
        );
    }

    #[test]
    fn perform_posts_to_a_local_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buf = [0; 8192];
            let mut got = Vec::new();
            loop {
                match std::io::Read::read(&mut stream, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        got.extend_from_slice(&buf[..n]);
                        if got.windows(4).any(|window| window == b"\r\n\r\n")
                            && got.ends_with(b"ping")
                        {
                            break;
                        }
                    }
                }
            }
            let saw_ping = got.windows(4).any(|window| window == b"ping");
            let body = if saw_ping { "pong" } else { "missing" };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
        });

        let mut spec = request(&format!("http://{address}/echo"));
        spec.method = "POST".into();
        spec.body = "ping".into();
        spec.headers
            .insert("Content-Type".into(), "text/plain".into());
        let value = perform(prepare(spec).unwrap()).unwrap();
        assert_eq!(value["status"], 200);
        assert_eq!(value["text"], "pong");
    }
}
