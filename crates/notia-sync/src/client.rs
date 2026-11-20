//! HTTP calls the app makes to a sync server.

use std::time::Duration;

use crate::{FileRev, Manifest, SyncError, paths::encode_path, require_vault_id, sha256_hex};

pub(crate) struct SyncClient {
    agent: ureq::Agent,
    base_url: String,
    token: String,
    vault: String,
    max_bytes: u64,
}

pub(crate) enum WriteResult {
    Applied {
        rev: FileRev,
        /// Present when the server names the vault generation after the write.
        generation: Option<u64>,
    },
    Rejected,
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
    generation: Option<u64>,
}

pub(crate) fn normalize_server(url: &str) -> Result<String, SyncError> {
    let url = url.trim().trim_end_matches('/').to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(SyncError::new(
            "the server address must start with http:// or https://",
        ));
    }
    if url
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(SyncError::new("the server address contains a space"));
    }
    Ok(url)
}

pub(crate) fn require_token(token: &str) -> Result<&str, SyncError> {
    let token = token.trim();
    if token.is_empty() || token.len() > 512 {
        return Err(SyncError::new("the token is empty"));
    }
    if !token.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(SyncError::new(
            "the token can only contain printable characters without spaces",
        ));
    }
    Ok(token)
}

impl SyncClient {
    pub(crate) fn connect(
        server: &str,
        token: &str,
        vault: &str,
        max_bytes: u64,
    ) -> Result<Self, SyncError> {
        let base_url = normalize_server(server)?;
        let token = require_token(token)?.to_string();
        let vault = require_vault_id(vault)?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        Ok(Self {
            agent,
            base_url,
            token,
            vault,
            max_bytes: max_bytes.max(1),
        })
    }

    pub(crate) fn manifest(&self) -> Result<Manifest, SyncError> {
        let response = self.exchange(self.auth(self.agent.get(self.url("v1/manifest"))).call())?;
        if response.status == 200 {
            return serde_json::from_slice(&response.body).map_err(|error| {
                SyncError::new(format!(
                    "the server sent a manifest that could not be read: {error}"
                ))
            });
        }
        Err(status_error(response.status, &response.body))
    }

    /// `None` when this server has no `/v1/head` (it answers 404).
    pub(crate) fn head(&self) -> Result<Option<u64>, SyncError> {
        let response = self.exchange(self.auth(self.agent.get(self.url("v1/head"))).call())?;
        if response.status == 404 {
            return Ok(None);
        }
        if response.status == 200 {
            let value: serde_json::Value =
                serde_json::from_slice(&response.body).map_err(|error| {
                    SyncError::new(format!(
                        "the server sent a head that could not be read: {error}"
                    ))
                })?;
            let generation = value
                .get("generation")
                .and_then(|generation| generation.as_u64())
                .ok_or_else(|| SyncError::new("the server sent a head that could not be read"))?;
            return Ok(Some(generation));
        }
        Err(status_error(response.status, &response.body))
    }

    pub(crate) fn download(&self, path: &str) -> Result<Vec<u8>, SyncError> {
        let response = self.exchange(self.auth(self.agent.get(self.file_url(path))).call())?;
        if response.status == 200 {
            if response.body.len() as u64 > self.max_bytes {
                return Err(SyncError::new(format!(
                    "{path} is larger than this app will sync"
                )));
            }
            return Ok(response.body);
        }
        Err(status_error(response.status, &response.body))
    }

    pub(crate) fn upload(
        &self,
        path: &str,
        bytes: &[u8],
        mtime_ms: u64,
        base: Option<&str>,
    ) -> Result<WriteResult, SyncError> {
        let mut request = self
            .auth(self.agent.put(self.file_url(path)))
            .header("Content-Type", "application/octet-stream")
            .header("X-Notia-Mtime", mtime_ms.to_string());
        if let Some(base) = base {
            request = request.header("X-Notia-Base", base);
        }
        let response = self.exchange(request.send(bytes))?;
        if response.status == 200 {
            let rev: FileRev = serde_json::from_slice(&response.body).map_err(|error| {
                SyncError::new(format!(
                    "the server accepted {path} but the reply was unreadable: {error}"
                ))
            })?;
            if rev.sha256 != sha256_hex(bytes) {
                return Err(SyncError::new(format!(
                    "the server stored a different copy of {path}"
                )));
            }
            return Ok(WriteResult::Applied {
                rev,
                generation: response
                    .generation
                    .or_else(|| json_generation(&response.body)),
            });
        }
        if response.status == 409 || response.status == 412 {
            return Ok(WriteResult::Rejected);
        }
        Err(status_error(response.status, &response.body))
    }

    pub(crate) fn delete(&self, path: &str, base: &str) -> Result<WriteResult, SyncError> {
        let response = self.exchange(
            self.auth(self.agent.delete(self.file_url(path)))
                .header("X-Notia-Base", base)
                .call(),
        )?;
        if response.status == 200 || response.status == 204 {
            return Ok(WriteResult::Applied {
                rev: FileRev {
                    sha256: String::new(),
                    size: 0,
                    mtime_ms: 0,
                },
                generation: response
                    .generation
                    .or_else(|| json_generation(&response.body)),
            });
        }
        if response.status == 409 || response.status == 412 {
            return Ok(WriteResult::Rejected);
        }
        Err(status_error(response.status, &response.body))
    }

    fn auth<T>(&self, request: ureq::RequestBuilder<T>) -> ureq::RequestBuilder<T> {
        request
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-Notia-Vault", encode_path(&self.vault))
            .header(
                "User-Agent",
                concat!("notia-sync/", env!("CARGO_PKG_VERSION")),
            )
    }

    fn file_url(&self, path: &str) -> String {
        self.url(&format!("v1/files/{}", encode_path(path)))
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    fn exchange(
        &self,
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<HttpResponse, SyncError> {
        let mut response = result.map_err(|error| match error {
            ureq::Error::StatusCode(status) => status_error(status, &[]),
            other => SyncError::new(format!("could not reach the server: {other}")),
        })?;
        let status = response.status().as_u16();
        let generation = response
            .headers()
            .get("x-notia-generation")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse().ok());
        let limit = if status == 200 {
            self.max_bytes.max(8 * 1024 * 1024)
        } else {
            64 * 1024
        };
        let body = response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|error| {
                SyncError::new(format!("could not read the server's reply: {error}"))
            })?;
        Ok(HttpResponse {
            status,
            body,
            generation,
        })
    }
}

fn json_generation(body: &[u8]) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("generation")?.as_u64())
}

fn status_error(status: u16, body: &[u8]) -> SyncError {
    if status == 401 {
        return SyncError::new("the server rejected the token");
    }
    if status == 413 {
        return SyncError::new("the server refused a file that is too large");
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body)
        && let Some(message) = value.get("error").and_then(|error| error.as_str())
        && !message.is_empty()
    {
        return SyncError::new(message.to_string());
    }
    SyncError::new(format!("the server returned HTTP {status}"))
}
