//! The HTTP API of `notia-sync-server`.
//!
//! `GET /v1/health` needs no token. Everything else needs
//! `Authorization: Bearer <token>` and `X-Notia-Vault`, which is the vault id
//! percent-encoded. Uploads and deletes name the hash the client last read in
//! `X-Notia-Base`, and the server refuses the write when that hash is stale.
//! `GET /v1/head` returns how many times the vault has changed, so a client
//! can watch it without downloading every file.

use std::io::Read as _;

use crate::{
    SyncError,
    paths::decode_path,
    snapshot::FileRev,
    store::{Store, StoreWrite, base_hash},
};

pub struct Listener {
    server: tiny_http::Server,
}

pub fn bind(addr: &str) -> Result<Listener, SyncError> {
    let server = tiny_http::Server::http(addr)
        .map_err(|error| SyncError::new(format!("could not listen on {addr}: {error}")))?;
    Ok(Listener { server })
}

impl Listener {
    pub fn url(&self) -> String {
        format!("http://{}", self.server.server_addr())
    }

    pub fn serve(self, store: Store, token: &str) {
        for request in self.server.incoming_requests() {
            handle(&store, token, request);
        }
    }
}

enum Route {
    Health,
    Head,
    Manifest,
    File(String),
    BadPath,
    Unknown,
}

fn handle(store: &Store, token: &str, mut request: tiny_http::Request) {
    let method = method_name(request.method());
    let url = request.url().to_string();
    let route = route(&url);
    let auth = header(&request, "authorization");
    let vault_header = header(&request, "x-notia-vault");
    let base_header = header(&request, "x-notia-base");
    let mtime_header = header(&request, "x-notia-mtime");

    let reply = dispatch(
        store,
        token,
        method,
        route,
        auth.as_deref(),
        vault_header.as_deref(),
        base_header.as_deref(),
        mtime_header.as_deref(),
        &mut request,
    );
    let status = reply.status;
    let path = url.split('?').next().unwrap_or(&url);
    eprintln!("notia-sync-server: {method} {path} {status}");
    let _ = request.respond(reply.into_response());
}

struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    extra: Vec<(String, String)>,
}

impl Reply {
    fn json(status: u16, body: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: body.to_string().into_bytes(),
            extra: Vec::new(),
        }
    }

    fn error(status: u16, message: &str) -> Self {
        Self::json(status, serde_json::json!({ "error": message }))
    }

    fn into_response(self) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
        let mut response = tiny_http::Response::from_data(self.body).with_status_code(self.status);
        if let Ok(header) =
            tiny_http::Header::from_bytes(b"Content-Type".as_slice(), self.content_type.as_bytes())
        {
            response = response.with_header(header);
        }
        for (name, value) in self.extra {
            if let Ok(header) = tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()) {
                response = response.with_header(header);
            }
        }
        response
    }
}

fn dispatch(
    store: &Store,
    token: &str,
    method: &str,
    route: Route,
    authorization: Option<&str>,
    vault_header: Option<&str>,
    base_header: Option<&str>,
    mtime_header: Option<&str>,
    request: &mut tiny_http::Request,
) -> Reply {
    if matches!(route, Route::Health) {
        return if method == "GET" {
            Reply::json(200, serde_json::json!({ "ok": true }))
        } else {
            Reply::error(405, "method not allowed")
        };
    }
    if !authorized(authorization, token) {
        return Reply::error(401, "the server rejected the token");
    }
    let Some(vault) = vault_header.and_then(|value| {
        decode_path(value).or_else(|| {
            // An unencoded id still has to be a single safe path segment.
            decode_path(&crate::paths::encode_path(value))
        })
    }) else {
        return Reply::error(400, "the vault id is missing or not allowed");
    };
    // decode_path accepts any safe relative path. A vault id is one segment.
    if vault.contains('/') {
        return Reply::error(400, "the vault id is not allowed");
    }

    match route {
        Route::Health => Reply::error(500, "unreachable"),
        Route::Unknown => Reply::error(404, "not found"),
        Route::BadPath => Reply::error(400, "that path is not allowed"),
        Route::Head => head(store, method, &vault),
        Route::Manifest => manifest(store, method, &vault),
        Route::File(path) => file(
            store,
            method,
            &vault,
            &path,
            base_header,
            mtime_header,
            request,
        ),
    }
}

fn head(store: &Store, method: &str, vault: &str) -> Reply {
    if method != "GET" {
        return Reply::error(405, "method not allowed");
    }
    match store.manifest(vault) {
        Ok(manifest) => Reply::json(
            200,
            serde_json::json!({ "generation": manifest.generation.unwrap_or(0) }),
        ),
        Err(error) => Reply::error(400, error.message()),
    }
}

fn manifest(store: &Store, method: &str, vault: &str) -> Reply {
    if method != "GET" {
        return Reply::error(405, "method not allowed");
    }
    match store.manifest(vault) {
        Ok(manifest) => Reply::json(
            200,
            serde_json::json!({
                "generation": manifest.generation.unwrap_or(0),
                "files": manifest.files,
            }),
        ),
        Err(error) => Reply::error(400, error.message()),
    }
}

fn file(
    store: &Store,
    method: &str,
    vault: &str,
    path: &str,
    base_header: Option<&str>,
    mtime_header: Option<&str>,
    request: &mut tiny_http::Request,
) -> Reply {
    match method {
        "GET" => match store.get(vault, path) {
            Ok(Some((rev, bytes))) => Reply {
                status: 200,
                content_type: "application/octet-stream",
                body: bytes,
                extra: vec![
                    ("X-Notia-Sha256".into(), rev.sha256),
                    ("X-Notia-Mtime".into(), rev.mtime_ms.to_string()),
                ],
            },
            Ok(None) => Reply::error(404, "the file is not on the server"),
            Err(error) => Reply::error(400, error.message()),
        },
        "PUT" => {
            let body = match read_body(request, store.max_bytes()) {
                Ok(body) => body,
                Err(ReadBody::TooLarge) => {
                    return Reply::error(413, "the file is larger than the server allows");
                }
                Err(ReadBody::Io(error)) => {
                    return Reply::error(400, &format!("could not read the upload: {error}"));
                }
            };
            let base = match base_hash(base_header) {
                Ok(base) => base,
                Err(error) => return Reply::error(400, error.message()),
            };
            let mtime = mtime_header
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            match store.put(vault, path, &body, mtime, base.as_deref()) {
                Ok(StoreWrite::Applied(rev)) => {
                    applied(200, rev_json(&rev), vault_generation(store, vault))
                }
                Ok(StoreWrite::Conflict) => {
                    Reply::error(412, "the file changed and will be merged on the next sync")
                }
                Err(error) => {
                    let status = if error.message().contains("larger") {
                        413
                    } else if error.message().contains("not allowed") {
                        400
                    } else {
                        500
                    };
                    Reply::error(status, error.message())
                }
            }
        }
        "DELETE" => {
            let base = match base_hash(base_header) {
                Ok(Some(base)) => base,
                Ok(None) => {
                    return Reply::error(400, "the delete did not name the revision it read");
                }
                Err(error) => return Reply::error(400, error.message()),
            };
            match store.delete(vault, path, &base) {
                Ok(StoreWrite::Applied(_)) => {
                    applied(204, serde_json::Value::Null, vault_generation(store, vault))
                }
                Ok(StoreWrite::Conflict) => {
                    Reply::error(412, "the file changed and will be merged on the next sync")
                }
                Err(error) => Reply::error(400, error.message()),
            }
        }
        _ => Reply::error(405, "method not allowed"),
    }
}

enum ReadBody {
    TooLarge,
    Io(std::io::Error),
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

fn vault_generation(store: &Store, vault: &str) -> u64 {
    store
        .manifest(vault)
        .ok()
        .and_then(|manifest| manifest.generation)
        .unwrap_or(0)
}

/// A write that landed. The generation header tells a watching client that
/// this vault moved, including a delete, which has no JSON body.
fn applied(status: u16, mut body: serde_json::Value, generation: u64) -> Reply {
    if let Some(object) = body.as_object_mut() {
        object.insert("generation".into(), serde_json::json!(generation));
    }
    let mut reply = if status == 204 {
        Reply {
            status,
            content_type: "application/json",
            body: Vec::new(),
            extra: Vec::new(),
        }
    } else {
        Reply::json(status, body)
    };
    reply
        .extra
        .push(("X-Notia-Generation".into(), generation.to_string()));
    reply
}

fn rev_json(rev: &FileRev) -> serde_json::Value {
    serde_json::json!({
        "sha256": rev.sha256,
        "size": rev.size,
        "mtime_ms": rev.mtime_ms,
    })
}

fn route(url: &str) -> Route {
    let path = url.split('?').next().unwrap_or(url);
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return Route::Unknown;
    }
    if path == "/v1/health" {
        return Route::Health;
    }
    if path == "/v1/head" {
        return Route::Head;
    }
    if path == "/v1/manifest" {
        return Route::Manifest;
    }
    if let Some(rest) = path.strip_prefix("/v1/files/") {
        return match decode_path(rest) {
            Some(file) => Route::File(file),
            None => Route::BadPath,
        };
    }
    Route::Unknown
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
pub(crate) struct TestServer {
    pub url: String,
    _dir: tempfile::TempDir,
}

#[cfg(test)]
pub(crate) fn start_test_server(token: &str, max_bytes: u64) -> TestServer {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open(dir.path(), max_bytes).expect("store");
    let listener = bind("127.0.0.1:0").expect("listen");
    let url = listener.url();
    let token = token.to_string();
    std::thread::spawn(move || listener.serve(store, &token));
    TestServer { url, _dir: dir }
}
