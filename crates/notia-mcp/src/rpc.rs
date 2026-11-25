//! JSON-RPC messages for MCP tools.
//!
//! The server advertises three tools: `list_notes`, `read_note`, and
//! `write_note`. A tool failure is a normal result with `isError: true`, so
//! the client shows the lock error and not a protocol failure.

use serde_json::{Value, json};

use crate::gate::Host;

const INSTRUCTIONS: &str = "\
You can read and write Markdown notes only when the user has unlocked them. \
A note is unlocked when YAML frontmatter at the top of the file contains \
`unlocked: true`. Other files are locked: do not guess paths, and do not try \
to reveal them. When you write a note, keep the `unlocked: true` frontmatter \
line. You cannot create new notes with these tools.";

const PROTOCOL: &str = "2025-03-26";
const PROTOCOLS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

pub(crate) fn parse_error() -> Value {
    error_response(None, -32700, "parse error")
}

/// Handles one JSON-RPC message. Notifications return `None` (no response).
pub(crate) fn handle(host: &Host, message: &Value) -> Option<Value> {
    if !message.is_object() {
        return Some(error_response(None, -32600, "invalid request"));
    }
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return reply_if_request(message, -32600, "invalid request");
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return reply_if_request(message, -32600, "invalid request");
    };
    let id = message.get("id")?;
    let empty = json!({});
    let params = match message.get("params") {
        None | Some(Value::Null) => &empty,
        Some(value) if value.is_object() => value,
        Some(_) => return Some(error_response(Some(id), -32602, "invalid params")),
    };
    Some(dispatch(host, id, method, params))
}

fn reply_if_request(message: &Value, code: i32, text: &str) -> Option<Value> {
    message
        .get("id")
        .map(|id| error_response(Some(id), code, text))
}

fn dispatch(host: &Host, id: &Value, method: &str, params: &Value) -> Value {
    match method {
        "initialize" => initialize(id, params),
        "ping" | "logging/setLevel" => success(id, json!({})),
        "tools/list" => success(id, json!({ "tools": tools() })),
        "tools/call" => tools_call(host, id, params),
        _ => error_response(Some(id), -32601, "method not found"),
    }
}

fn initialize(id: &Value, params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .filter(|version| PROTOCOLS.contains(version))
        .unwrap_or(PROTOCOL);
    success(
        id,
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "notia-mcp",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": INSTRUCTIONS,
        }),
    )
}

fn tools() -> Value {
    json!([
        {
            "name": "list_notes",
            "description": "List Markdown notes the user unlocked for AI. A note is unlocked only when its frontmatter contains `unlocked: true`. Locked notes are omitted and their contents are not returned. Notes over the server size limit are omitted.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "read_note",
            "description": "Read one unlocked Markdown note, including its frontmatter. Locked, missing, and non-note paths return an error and no content.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Vault-relative Markdown path, such as Daily/Today.md"
                    }
                },
                "required": ["path"]
            }
        },
        {
            "name": "write_note",
            "description": "Replace an unlocked Markdown note. The note must already exist and be unlocked. The new text must still include frontmatter `unlocked: true`. This does not create notes and does not change locked notes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Vault-relative Markdown path of an unlocked note"
                    },
                    "content": {
                        "type": "string",
                        "description": "Full Markdown replacement, still containing frontmatter unlocked: true"
                    }
                },
                "required": ["path", "content"]
            }
        }
    ])
}

fn tools_call(host: &Host, id: &Value, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(Some(id), -32602, "unknown tool");
    };
    let args = match params.get("arguments") {
        None => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => return error_response(Some(id), -32602, "arguments must be an object"),
    };
    let outcome = match name {
        "list_notes" => list_notes(host),
        "read_note" => read_note(host, &args),
        "write_note" => write_note(host, &args),
        _ => return error_response(Some(id), -32602, "unknown tool"),
    };
    match outcome {
        Ok(text) => success(id, tool_result(&text, false)),
        Err(message) => success(id, tool_result(&message, true)),
    }
}

fn list_notes(host: &Host) -> Result<String, String> {
    let notes = host.list_unlocked().map_err(|error| error.to_string())?;
    let items: Vec<_> = notes
        .iter()
        .map(|note| json!({ "path": note.path, "title": note.title }))
        .collect();
    serde_json::to_string(&items).map_err(|error| format!("could not list notes: {error}"))
}

fn read_note(host: &Host, args: &Value) -> Result<String, String> {
    let path = required_str(args, "path")?;
    host.read_note(path).map_err(|error| error.to_string())
}

fn write_note(host: &Host, args: &Value) -> Result<String, String> {
    let path = required_str(args, "path")?;
    let Some(content) = args.get("content").and_then(Value::as_str) else {
        let message = if args.get("content").is_some() {
            "content must be a string"
        } else {
            "content is required"
        };
        return Err(message.into());
    };
    host.write_note(path, content)
        .map(|()| format!("wrote {path}"))
        .map_err(|error| error.to_string())
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    match args.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        Some(Value::String(_)) | None => Err(format!("{key} is required")),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn tool_result(text: &str, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

fn success(id: &Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn error_response(id: Option<&Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::test_vault;

    fn call(host: &Host, message: &Value) -> Value {
        handle(host, message).expect("response")
    }

    #[test]
    fn initialize_tells_the_client_about_the_lock() {
        let vault = test_vault(1024);
        let response = call(
            &vault.host,
            &json!({
                "jsonrpc": "2.0",
                "id": "abc",
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "0" }
                }
            }),
        );
        assert_eq!(response["id"], "abc");
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(response["result"]["serverInfo"]["name"], "notia-mcp");
        let instructions = response["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains("unlocked: true"));
        assert!(instructions.contains("cannot create"));
    }

    #[test]
    fn tools_are_list_read_and_write() {
        let vault = test_vault(1024);
        let response = call(
            &vault.host,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        );
        let names: Vec<_> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["list_notes", "read_note", "write_note"]);
    }

    #[test]
    fn tool_calls_hide_locked_notes() {
        let vault = test_vault(1024);
        let locked = call(
            &vault.host,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "read_note",
                    "arguments": { "path": "locked.md" }
                }
            }),
        );
        assert_eq!(locked["result"]["isError"], true);
        let rendered = locked.to_string();
        assert!(rendered.contains("note is locked"));
        assert!(!rendered.contains(vault.secret));

        let open = call(
            &vault.host,
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "read_note",
                    "arguments": { "path": "open.md" }
                }
            }),
        );
        assert_eq!(open["result"]["isError"], false);
        assert!(
            open["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("hello")
        );

        let listed = call(
            &vault.host,
            &json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": "list_notes", "arguments": {} }
            }),
        );
        let text = listed["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("open.md"));
        assert!(!text.contains("locked.md"));
        assert!(!text.contains(vault.secret));

        let wrote = call(
            &vault.host,
            &json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "write_note",
                    "arguments": {
                        "path": "locked.md",
                        "content": "---\nunlocked: true\n---\nnope\n"
                    }
                }
            }),
        );
        assert_eq!(wrote["result"]["isError"], true);
        assert!(!wrote.to_string().contains(vault.secret));
        assert!(
            std::fs::read_to_string(vault.dir.path().join("locked.md"))
                .unwrap()
                .contains(vault.secret)
        );
    }

    #[test]
    fn notifications_have_no_response_and_unknown_methods_fail() {
        let vault = test_vault(1024);
        assert!(
            handle(
                &vault.host,
                &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
            )
            .is_none()
        );
        let response = call(
            &vault.host,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"}),
        );
        assert_eq!(response["error"]["code"], -32601);
        let response = call(
            &vault.host,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "read_note", "arguments": {} }
            }),
        );
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("path is required")
        );
    }
}
