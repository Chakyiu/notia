//! Durable log of MCP tool calls for one vault.
//!
//! Rows live in `.notia/mcp-audit.jsonl` (hidden from note tools). A call
//! starts as `started` and is updated to `ok` or `error` when it finishes, so
//! a reader can see what is in progress and what already happened. Note
//! contents are never stored.

use std::{fs, io, path::Path};

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};

const MAX_ENTRIES: usize = 400;

struct Row {
    time: String,
    action: String,
    target: String,
    status: String,
    error: Option<String>,
}

pub(crate) fn begin(root: &Path, action: &str, target: &str) {
    let action = clip(action, 80);
    let target = clip(target, 500);
    let row = Row {
        time: timestamp(),
        action: action.clone(),
        target: target.clone(),
        status: "started".to_string(),
        error: None,
    };
    if let Err(error) = push(root, row) {
        eprintln!("notia-mcp: could not write the audit log: {error}");
        return;
    }
    eprintln!("notia-mcp: audit {action} {} started", show(&target));
}

pub(crate) fn commit(root: &Path, action: &str, target: &str, status: &str, error: Option<&str>) {
    let action = clip(action, 80);
    let target = clip(target, 500);
    let error = error.map(|text| clip(text, 300));
    let path = audit_path(root);
    if let Err(write_error) = update(&path, &action, &target, status, error.clone()) {
        eprintln!("notia-mcp: could not write the audit log: {write_error}");
        return;
    }
    match &error {
        Some(text) => eprintln!(
            "notia-mcp: audit {action} {} {status} {text}",
            show(&target)
        ),
        None => eprintln!("notia-mcp: audit {action} {} {status}", show(&target)),
    }
}

pub(crate) fn recent(root: &Path, limit: usize) -> Result<String, String> {
    let rows = load(&audit_path(root))
        .map_err(|error| format!("could not read the audit log: {error}"))?;
    let skip = rows.len().saturating_sub(limit);
    Ok(render(&rows[skip..]))
}

fn update(
    path: &Path,
    action: &str,
    target: &str,
    status: &str,
    error: Option<String>,
) -> io::Result<()> {
    let mut rows = load(path)?;
    // shortcut: the last `started` row is this process's call. Two notia-mcp
    // processes on one vault can finish the wrong row; use a file lock if that
    // setup is real.
    if let Some(last) = rows.last_mut()
        && last.action == action
        && last.target == target
        && last.status == "started"
    {
        last.status = status.to_string();
        last.error = error;
        return save(path, &rows);
    }
    rows.push(Row {
        time: timestamp(),
        action: action.to_string(),
        target: target.to_string(),
        status: status.to_string(),
        error,
    });
    trim(&mut rows, MAX_ENTRIES);
    save(path, &rows)
}

fn push(root: &Path, row: Row) -> io::Result<()> {
    let path = audit_path(root);
    let mut rows = load(&path)?;
    rows.push(row);
    trim(&mut rows, MAX_ENTRIES);
    save(&path, &rows)
}

fn trim(rows: &mut Vec<Row>, max: usize) {
    if rows.len() > max {
        let extra = rows.len() - max;
        rows.drain(0..extra);
    }
}

fn load(path: &Path) -> io::Result<Vec<Row>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().filter_map(parse_line).collect())
}

fn parse_line(line: &str) -> Option<Row> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    Some(Row {
        time: value.get("time")?.as_str()?.to_string(),
        action: value.get("action")?.as_str()?.to_string(),
        target: value.get("target")?.as_str()?.to_string(),
        status: value.get("status")?.as_str()?.to_string(),
        error: match value.get("error") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_str()?.to_string()),
        },
    })
}

fn save(path: &Path, rows: &[Row]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut body = String::new();
    for row in rows {
        body.push_str(&row.to_line());
        body.push('\n');
    }
    let temp = path.with_file_name(".mcp-audit.jsonl.notia-tmp");
    fs::write(&temp, body)?;
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

fn render(rows: &[Row]) -> String {
    if rows.is_empty() {
        return "[]".to_string();
    }
    let mut out = String::from("[\n");
    for (index, row) in rows.iter().enumerate() {
        out.push_str("  ");
        out.push_str(&row.to_line());
        if index + 1 != rows.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push(']');
    out
}

fn audit_path(root: &Path) -> std::path::PathBuf {
    root.join(".notia").join("mcp-audit.jsonl")
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn show(target: &str) -> &str {
    if target.is_empty() { "-" } else { target }
}

fn clip(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max {
            break;
        }
        out.push(if ch.is_control() { ' ' } else { ch });
    }
    out
}

impl Row {
    fn to_line(&self) -> String {
        let mut line = format!(
            "{{\"time\":{},\"action\":{},\"target\":{},\"status\":{}",
            json!(self.time),
            json!(self.action),
            json!(self.target),
            json!(self.status),
        );
        if let Some(error) = &self.error {
            line.push_str(",\"error\":");
            line.push_str(&json!(error).to_string());
        }
        line.push('}');
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_closes_the_open_row_and_keeps_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join(".notia")).unwrap();
        fs::write(
            audit_path(root),
            "not json\n{\"time\":\"t\",\"action\":\"old\",\"target\":\"a.md\",\"status\":\"ok\"}\n",
        )
        .unwrap();

        begin(root, "write_note", "open.md");
        let mid = fs::read_to_string(audit_path(root)).unwrap();
        assert!(mid.contains("\"status\":\"started\""));
        assert!(mid.contains("open.md"));

        commit(root, "write_note", "open.md", "ok", None);
        let rows = load(&audit_path(root)).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].action, "write_note");
        assert_eq!(rows[1].target, "open.md");
        assert_eq!(rows[1].status, "ok");
        assert!(rows[1].error.is_none());
        assert!(rows[1].time.ends_with('Z'));
        assert!(
            !fs::read_to_string(audit_path(root))
                .unwrap()
                .contains("started")
        );
    }

    #[test]
    fn trim_drops_the_oldest_rows() {
        let mut rows: Vec<_> = (0..5)
            .map(|index| Row {
                time: "t".to_string(),
                action: "read_note".to_string(),
                target: index.to_string(),
                status: "ok".to_string(),
                error: None,
            })
            .collect();
        trim(&mut rows, 3);
        let targets: Vec<_> = rows.iter().map(|row| row.target.as_str()).collect();
        assert_eq!(targets, vec!["2", "3", "4"]);
    }

    #[test]
    fn targets_stay_one_json_line() {
        let dir = tempfile::tempdir().unwrap();
        begin(dir.path(), "read_note", "bad\n\"path\".md");
        let text = fs::read_to_string(audit_path(dir.path())).unwrap();
        assert_eq!(text.lines().count(), 1);
        let row: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(row["target"], "bad \"path\".md");
        assert_eq!(row["status"], "started");
    }
}
