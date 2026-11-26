//! HTTP transport for `notia-mcp`, and MCP on stdin/stdout.
//!
//! `GET /v1/health` needs no token. `POST /mcp` is streamable HTTP: one
//! JSON-RPC message in, one JSON response out (or `202` for a notification).
//! Requests need `Authorization: Bearer <token>`. A browser `Origin` is
//! accepted only for localhost, so a web page cannot call the server.

use std::io::{self, Read as _, Write as _};

use serde_json::Value;

use crate::{
    gate::Host,
    rpc::{self, handle},
};

pub struct Listener {
    server: tiny_http::Server,
}

pub fn bind(addr: &str) -> Result<Listener, String> {
    let server = tiny_http::Server::http(addr)
        .map_err(|error| format!("could not listen on {addr}: {error}"))?;
    Ok(Listener { server })
}

impl Listener {
    pub fn url(&self) -> String {
        format!("http://{}", self.server.server_addr())
    }

    pub fn serve(self, host: Host, token: &str) {
        for request in self.server.incoming_requests() {
            handle_http(&host, token, request);
        }
    }
}

/// Reads newline-delimited MCP messages from stdin and writes responses to stdout.
pub fn serve_stdio(host: &Host) -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lines() {
        let line = line.map_err(|error| format!("could not read stdin: {error}"))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(line) {
            Ok(message) => handle(host, &message),
            Err(_) => Some(rpc::parse_error()),
        };
        if let Some(response) = response {
            writeln!(stdout, "{response}")
                .map_err(|error| format!("could not write stdout: {error}"))?;
            stdout
                .flush()
                .map_err(|error| format!("could not write stdout: {error}"))?;
        }
    }
    Ok(())
}

enum Route {
    Health,
    Mcp,
    Unknown,
}

fn handle_http(host: &Host, token: &str, mut request: tiny_http::Request) {
    let method = method_name(request.method());
    let url = request.url().to_string();
    let route = route(&url);
    let reply = dispatch(host, token, method, route, &mut request);
    let status = reply.status;
    let path = url.split('?').next().unwrap_or(&url);
    eprintln!("notia-mcp: {method} {path} {status}");
    let _ = request.respond(reply.into_response());
}

struct Reply {
    status: u16,
    content_type: Option<&'static str>,
    body: Vec<u8>,
}

impl Reply {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            content_type: Some("application/json"),
            body: body.to_string().into_bytes(),
        }
    }

    fn accepted() -> Self {
        Self {
            status: 202,
            content_type: None,
            body: Vec::new(),
        }
    }

    fn http_error(status: u16, message: &str) -> Self {
        Self::json(
            status,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32000, "message": message }
            }),
        )
    }

    fn into_response(self) -> tiny_http::Response<io::Cursor<Vec<u8>>> {
        let mut response = tiny_http::Response::from_data(self.body).with_status_code(self.status);
        if let Some(content_type) = self.content_type
            && let Ok(header) =
                tiny_http::Header::from_bytes(b"Content-Type".as_slice(), content_type.as_bytes())
        {
            response = response.with_header(header);
        }
        response
    }
}

fn dispatch(
    host: &Host,
    token: &str,
    method: &str,
    route: Route,
    request: &mut tiny_http::Request,
) -> Reply {
    match route {
        Route::Health => {
            if method == "GET" {
                Reply::json(200, serde_json::json!({ "ok": true }))
            } else {
                Reply::http_error(405, "method not allowed")
            }
        }
        Route::Unknown => Reply::http_error(404, "not found"),
        Route::Mcp => mcp(host, token, method, request),
    }
}

fn mcp(host: &Host, token: &str, method: &str, request: &mut tiny_http::Request) -> Reply {
    if method != "POST" {
        return Reply::http_error(405, "method not allowed");
    }
    if !origin_allowed(header(request, "origin").as_deref()) {
        return Reply::http_error(403, "origin is not allowed");
    }
    if !authorized(header(request, "authorization").as_deref(), token) {
        return Reply::http_error(401, "the server rejected the token");
    }
    if let Some(accept) = header(request, "accept")
        && !accepts_mcp(&accept)
    {
        return Reply::http_error(406, "accept application/json or text/event-stream");
    }
    if let Some(content_type) = header(request, "content-type")
        && !json_content_type(&content_type)
    {
        return Reply::http_error(415, "content type must be application/json");
    }
    let limit = host.max_bytes().saturating_add(64 * 1024);
    let body = match read_body(request, limit) {
        Ok(body) => body,
        Err(ReadBody::TooLarge) => {
            return Reply::http_error(413, "the request is larger than the server allows");
        }
        Err(ReadBody::Io(error)) => {
            return Reply::http_error(400, &format!("could not read the request: {error}"));
        }
    };
    let message = match serde_json::from_slice::<Value>(&body) {
        Ok(message) => message,
        Err(_) => return Reply::json(400, rpc::parse_error()),
    };
    match handle(host, &message) {
        Some(response) => Reply::json(200, response),
        None => Reply::accepted(),
    }
}

fn route(url: &str) -> Route {
    let path = url.split('?').next().unwrap_or(url);
    match path.trim_end_matches('/') {
        "/v1/health" | "/health" => Route::Health,
        "/mcp" => Route::Mcp,
        _ => Route::Unknown,
    }
}

fn origin_allowed(origin: Option<&str>) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    matches!(origin_host(origin), Some("localhost" | "127.0.0.1" | "::1"))
}

fn origin_host(origin: &str) -> Option<&str> {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))?;
    let hostport = rest.split('/').next().unwrap_or(rest);
    if let Some(host) = hostport.strip_prefix('[') {
        return host.split(']').next().filter(|host| !host.is_empty());
    }
    hostport.split(':').next().filter(|host| !host.is_empty())
}

fn accepts_mcp(accept: &str) -> bool {
    accept.split(',').any(|part| {
        let media = part.split(';').next().unwrap_or(part).trim();
        media == "*/*"
            || media.eq_ignore_ascii_case("application/*")
            || media.eq_ignore_ascii_case("text/*")
            || media.eq_ignore_ascii_case("application/json")
            || media.eq_ignore_ascii_case("text/event-stream")
    })
}

fn json_content_type(value: &str) -> bool {
    value
        .split(';')
        .next()
        .unwrap_or(value)
        .trim()
        .eq_ignore_ascii_case("application/json")
}

fn authorized(header: Option<&str>, expected: &str) -> bool {
    let Some(header) = header.map(str::trim) else {
        return false;
    };
    let Some(token) = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
    else {
        return false;
    };
    tokens_match(token.trim(), expected)
}

fn tokens_match(presented: &str, expected: &str) -> bool {
    let presented = presented.as_bytes();
    let expected = expected.as_bytes();
    if presented.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in presented.iter().zip(expected) {
        diff |= left ^ right;
    }
    diff == 0
}

fn header(request: &tiny_http::Request, name: &str) -> Option<String> {
    request.headers().iter().find_map(|header| {
        if header.field.as_str().as_str().eq_ignore_ascii_case(name) {
            Some(header.value.as_str().to_string())
        } else {
            None
        }
    })
}

enum ReadBody {
    TooLarge,
    Io(io::Error),
}

fn read_body(request: &mut tiny_http::Request, max_bytes: u64) -> Result<Vec<u8>, ReadBody> {
    if request
        .body_length()
        .is_some_and(|length| length as u64 > max_bytes)
    {
        return Err(ReadBody::TooLarge);
    }
    let mut body = Vec::new();
    request
        .as_reader()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut body)
        .map_err(ReadBody::Io)?;
    if body.len() as u64 > max_bytes {
        return Err(ReadBody::TooLarge);
    }
    Ok(body)
}

fn method_name(method: &tiny_http::Method) -> &'static str {
    match method {
        tiny_http::Method::Get => "GET",
        tiny_http::Method::Put => "PUT",
        tiny_http::Method::Delete => "DELETE",
        tiny_http::Method::Post => "POST",
        tiny_http::Method::Head => "HEAD",
        tiny_http::Method::Options => "OPTIONS",
        tiny_http::Method::Connect => "CONNECT",
        tiny_http::Method::Trace => "TRACE",
        tiny_http::Method::Patch => "PATCH",
        _ => "OTHER",
    }
}

#[cfg(test)]
mod tests {
    use std::{net::TcpStream, thread, time::Duration};

    use super::*;
    use crate::gate::test_vault;

    #[test]
    fn origin_and_token_checks() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("")));
        assert!(origin_allowed(Some("http://127.0.0.1:8788")));
        assert!(origin_allowed(Some("http://localhost")));
        assert!(origin_allowed(Some("https://[::1]/mcp")));
        assert!(!origin_allowed(Some("http://evil.example")));
        assert!(!origin_allowed(Some("null")));
        assert!(!origin_allowed(Some("http://127.0.0.1.evil.example")));
        assert!(authorized(Some("Bearer test-token"), "test-token"));
        assert!(authorized(Some("bearer test-token"), "test-token"));
        assert!(!authorized(None, "test-token"));
        assert!(!authorized(Some("Bearer other-token"), "test-token"));
        assert!(!authorized(Some("Bearer test-token-extra"), "test-token"));
        assert!(accepts_mcp("*/*"));
        assert!(accepts_mcp("application/json, text/event-stream"));
        assert!(!accepts_mcp("text/html"));
    }

    #[test]
    fn http_requires_a_token_and_hides_locked_notes() {
        let vault = test_vault(1024);
        let listener = bind("127.0.0.1:0").expect("listen");
        let url = listener.url();
        let host = vault.host;
        thread::spawn(move || listener.serve(host, "test-token"));

        let (status, body) = request(&url, "GET", "/v1/health", &[], "");
        assert_eq!(status, 200);
        assert!(body.contains("\"ok\":true"));

        let (status, body) = request(
            &url,
            "POST",
            "/mcp",
            &[("Content-Type", "application/json")],
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        );
        assert_eq!(status, 401);
        assert!(body.contains("rejected the token"));

        let (status, _) = request(
            &url,
            "POST",
            "/mcp",
            &[
                ("Content-Type", "application/json"),
                ("Authorization", "Bearer test-token"),
                ("Origin", "http://evil.example"),
            ],
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        );
        assert_eq!(status, 403);

        let (status, _) = request(
            &url,
            "POST",
            "/mcp",
            &[
                ("Content-Type", "application/json"),
                ("Authorization", "Bearer test-token"),
                ("Accept", "text/html"),
            ],
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        );
        assert_eq!(status, 406);

        let (status, body) = request(
            &url,
            "POST",
            "/mcp",
            &[
                ("Content-Type", "application/json"),
                ("Authorization", "Bearer test-token"),
                ("Origin", "http://127.0.0.1:8788"),
                ("Accept", "application/json, text/event-stream"),
            ],
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read_note","arguments":{"path":"locked.md"}}}"#,
        );
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("note is locked"));
        assert!(!body.contains(vault.secret));

        let (status, body) = request(
            &url,
            "POST",
            "/mcp",
            &[
                ("Content-Type", "application/json"),
                ("Authorization", "Bearer test-token"),
            ],
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );
        assert_eq!(status, 202, "{body}");
    }

    fn request(
        base: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String) {
        let addr = base.trim_start_matches("http://");
        let mut stream = connect(addr);
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let raw = read_http(&mut stream);
        let status = raw
            .split_whitespace()
            .nth(1)
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        let body = raw
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        (status, body)
    }

    fn connect(addr: &str) -> TcpStream {
        let mut last = None;
        for _ in 0..20 {
            match TcpStream::connect(addr) {
                Ok(stream) => return stream,
                Err(error) => {
                    last = Some(error);
                    thread::sleep(Duration::from_millis(25));
                }
            }
        }
        panic!("connect {addr}: {}", last.expect("error"));
    }

    fn read_http(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 2048];
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if complete(&buf) {
                        break;
                    }
                }
                Err(error)
                    if error.kind() == io::ErrorKind::TimedOut
                        || error.kind() == io::ErrorKind::WouldBlock =>
                {
                    break;
                }
                Err(error) => panic!("{error}"),
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn complete(buf: &[u8]) -> bool {
        let Some(split) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&buf[..split]);
        let length = headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        });
        match length {
            Some(length) => buf.len() - (split + 4) >= length,
            None => false,
        }
    }
}
