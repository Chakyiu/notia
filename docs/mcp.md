# Hosting and using notia-mcp

`notia-mcp` is a standalone Model Context Protocol (MCP) server for a Notia vault. It uses the same self-hosted model as `notia-sync-server`, exposing read and write tools only for Markdown notes that you explicitly unlock in frontmatter.

- [1. Hosting alongside notia-sync](#1-hosting-alongside-notia-sync)
- [2. Install and build](#2-install-and-build)
- [3. Running the server](#3-running-the-server)
- [4. CLI flags and environment variables](#4-cli-flags-and-environment-variables)
- [5. Unlocking notes and MCP tools](#5-unlocking-notes-and-mcp-tools)
- [6. Connecting an MCP client](#6-connecting-an-mcp-client)
- [7. Security notes](#7-security-notes)

---

## 1. Hosting alongside notia-sync

`notia-mcp` (`crates/notia-mcp`) and `notia-sync-server` (`crates/notia-sync`) are separate binaries in the same workspace with parallel CLI and Docker layouts.

| Property | `notia-sync-server` | `notia-mcp` |
| --- | --- | --- |
| Purpose | Two-way vault sync for Notia apps | MCP tools (`list_notes`, `read_note`, `write_note`) for AI clients |
| Transports | HTTP | Streamable HTTP (`POST /mcp`) or `--stdio` |
| Default `--listen` | `0.0.0.0:8787` | `127.0.0.1:8788` |
| Target folder flag | `--data DIR` (default `./notia-sync-data`) | `--vault DIR` or `NOTIA_MCP_VAULT` (required, must exist) |
| Token option / env | `--token` / `NOTIA_SYNC_TOKEN` (auto-generates `DIR/token` if unset) | `--token` / `NOTIA_MCP_TOKEN` (required for HTTP; ignored by `--stdio`) |
| Default `--max-bytes` | `67108864` (64 MiB) | `1048576` (1 MiB) |
| Health check | `GET /v1/health` (unauthenticated) | `GET /v1/health` or `GET /health` (unauthenticated) |
| Docker Compose dir | `docker/sync/` (named `sync-data` volume) | `docker/mcp/` (`${NOTIA_VAULT}:/vault` bind mount) |

### Do not point `notia-mcp` at `notia-sync-server`'s data directory

`notia-sync-server` stores each vault under `<data>/vaults/<vault-id>/files/` alongside `<data>/vaults/<vault-id>/manifest.json`. It updates `manifest.json` and the vault's `generation` counter only when a Notia client uploads or deletes files over the sync HTTP API (`crates/notia-sync/src/store.rs`).

`notia-mcp` reads and writes files directly on disk (`notia_core::Vault::write_note`) and does not update `manifest.json`. If you point `notia-mcp --vault` (or Docker `NOTIA_VAULT`) at `notia-sync-data/vaults/<vault-id>/files/`:

- Real-time sync will not detect edits made through MCP because `generation` and `manifest.json` stay unchanged.
- Syncing Notia clients can reject downloads with a hash mismatch (`<path> from the server did not match its hash`) or overwrite MCP edits on the next sync.

### Recommended setup with Sync

Point `notia-mcp` at a **synced client vault folder** (for example `~/Notes` on a computer running Notia with **Settings → Sync** enabled), not at the sync server's storage directory:

1. Host `notia-sync-server` on port `8787` (via `notia-sync-server` or `docker/sync/`).
2. Open your vault in Notia on the computer where your AI tools run (or a dedicated always-on desktop/workstation) and enable **Sync automatically** or **Real-time sync** in **Settings → Sync**.
3. Start `notia-mcp` pointing `--vault` (or `NOTIA_VAULT` in Docker) at that local synced vault folder.

With that layout, MCP writes land atomically on disk, the Notia Sync plugin detects the change and uploads it to `notia-sync-server`, and edits synced down from other devices become visible to `notia-mcp` on the next tool call. There is currently no standalone headless sync client CLI; sync client logic runs inside the Notia desktop and mobile apps.

If you run `notia-sync-server` and `notia-mcp` on the same machine, they use distinct default ports (`8787` and `8788`), distinct token variables (`NOTIA_SYNC_TOKEN` and `NOTIA_MCP_TOKEN`), and separate Compose directories (`docker/sync/` and `docker/mcp/`).

---

## 2. Install and build

### Prerequisites

- **Rust 1.88 or newer** (the workspace uses Rust edition 2024 and `let` chains).
- **Patched `gpui-component` checkout**: `Cargo.toml` patches `gpui-component` to `third_party/gpui-component`, which is gitignored. Even though `notia-mcp` does not link GPUI, Cargo resolves workspace patches on every build, and `docker/mcp/Dockerfile` copies that directory into the image build context.

Fetch the dependency once from the repository root before running any `cargo` or `docker compose --build` command:

```sh
bash scripts/fetch-gpui-component.sh
```

### Build or install from source

Run directly with Cargo:

```sh
export NOTIA_MCP_TOKEN="replace-with-a-random-token"
cargo run -p notia-mcp -- --vault ~/Notes
```

Build a release binary (`target/release/notia-mcp`, or `target\release\notia-mcp.exe` on Windows):

```sh
cargo build -p notia-mcp --release
```

Or install `notia-mcp` into Cargo's `bin` directory (`~/.cargo/bin` by default):

```sh
cargo install --path crates/notia-mcp --locked
```

Verify the binary:

```sh
notia-mcp --help
```

---

## 3. Running the server

### HTTP mode (default)

HTTP mode binds `127.0.0.1:8788` by default and requires a bearer token (`NOTIA_MCP_TOKEN` or `--token`). Unlike `notia-sync-server`, `notia-mcp` never auto-generates a token file and refuses to start HTTP mode without one.

```sh
export NOTIA_MCP_TOKEN="replace-with-a-random-token"
notia-mcp --vault ~/Notes
```

To bind a different address or change the max note size (in bytes):

```sh
notia-mcp --vault ~/Notes --listen 127.0.0.1:8788 --max-bytes 2097152
```

HTTP endpoints:

- `GET /v1/health` (also `GET /health`): returns `200` with `{"ok":true}`; requires no token.
- `POST /mcp`: streamable HTTP JSON-RPC 2.0 endpoint. Returns `200` with a JSON response for requests, or `202` with an empty body for notifications.

Required HTTP request headers on `POST /mcp`:

- `Authorization: Bearer <token>` (case-insensitive `Bearer` scheme).
- `Content-Type: application/json` (if sent).
- `Accept` (if sent) must allow `application/json`, `text/event-stream`, `application/*`, `text/*`, or `*/*`.
- `Origin` (when sent by a browser) must use `http://` or `https://` on `localhost`, `127.0.0.1`, or `[::1]`; any other `Origin` returns `403`.

Access logs go to `stderr` as `notia-mcp: <METHOD> <PATH> <STATUS>` without tokens or note contents.

### Stdio mode

For local MCP clients that launch the server as a child process, pass `--stdio`:

```sh
notia-mcp --stdio --vault ~/Notes
```

In `--stdio` mode, `notia-mcp` reads newline-delimited JSON-RPC messages from `stdin`, writes JSON responses to `stdout`, and logs status lines to `stderr`. It does not open a port and does not check a token (passing `--listen` or `--token` with `--stdio` exits with an error).

### Docker Compose (`docker/mcp`)

`docker/mcp` mirrors `docker/sync` for containerized hosting. Run `bash scripts/fetch-gpui-component.sh` from the repository root first so `third_party/gpui-component` exists in the build context.

```sh
cd docker/mcp
cp .env.example .env
```

Edit `docker/mcp/.env` and replace the placeholders:

```dotenv
NOTIA_MCP_TOKEN=replace-with-openssl-rand-hex-32-output
NOTIA_VAULT=/absolute/path/to/your/vault
```

Generate a random token with:

```sh
openssl rand -hex 32
```

On Linux, set `NOTIA_UID` and `NOTIA_GID` in `docker/mcp/.env` to the account that owns the host vault (`id -u` and `id -g`). Unlike `docker/sync`, `docker/mcp` does not `chown` the mounted vault folder and defaults to `0:0` in `docker/mcp/compose.yaml`.

Start the container:

```sh
docker compose up --build -d
```

What `docker/mcp/compose.yaml` configures:

- Publishes `127.0.0.1:8788:8788` (localhost only on the host; inside the container, `notia-mcp` binds `0.0.0.0:8788` so Docker port forwarding can reach it).
- Mounts `${NOTIA_VAULT}:/vault` read-write.
- Health check polls `http://127.0.0.1:8788/v1/health` every 30s.

On Docker Desktop for Windows, `NOTIA_VAULT` can be a Windows path such as `C:/Users/me/Notes` (see `docker/mcp/.env.example`).

---

## 4. CLI flags and environment variables

### CLI flags

| Flag | Default | Required | Description |
| --- | --- | --- | --- |
| `--vault DIR` | `NOTIA_MCP_VAULT` | Yes (or env var) | Existing vault directory. Startup fails if missing or not a directory. |
| `--listen ADDR` | `127.0.0.1:8788` | No | TCP `host:port` to bind in HTTP mode. Rejected if combined with `--stdio`. |
| `--token TOKEN` | `NOTIA_MCP_TOKEN` | HTTP mode (or env var) | Shared bearer token (1–512 printable ASCII chars, no spaces/newlines). Rejected if combined with `--stdio`. |
| `--max-bytes N` | `1048576` (1 MiB) | No | Maximum note size in bytes (`>= 1`). HTTP request bodies allow `max-bytes + 65536` bytes. |
| `--stdio` | Off | No | Speak MCP over stdin/stdout instead of HTTP. Ignores `NOTIA_MCP_TOKEN`. |
| `-h`, `--help` | — | No | Print usage and exit. |

### Environment variables

| Variable | Used by | Precedence / Notes |
| --- | --- | --- |
| `NOTIA_MCP_VAULT` | `notia-mcp` binary | Used when `--vault` is omitted. Trimmed; empty value is ignored. |
| `NOTIA_MCP_TOKEN` | `notia-mcp` binary, `docker/mcp` | Used in HTTP mode when `--token` is omitted. Preferred over `--token` so the secret does not appear in process arguments. |
| `NOTIA_VAULT` | `docker/mcp/compose.yaml` | Host vault path mounted to `/vault` inside the container (`.env` only; note the variable name differs from `NOTIA_MCP_VAULT`). |
| `NOTIA_UID` | `docker/mcp/compose.yaml` | Optional container UID (defaults to `0` if unset). Set to `$(id -u)` on Linux. |
| `NOTIA_GID` | `docker/mcp/compose.yaml` | Optional container GID (defaults to `0` if unset). Set to `$(id -g)` on Linux. |

---

## 5. Unlocking notes and MCP tools

### The per-note locker (`unlocked: true`)

Notes are **locked by default**. There is no vault-wide read access and no UI toggle in Notia yet. To expose a note to MCP clients, add YAML frontmatter at the very top of the `.md` file with `unlocked: true`:

```yaml
---
unlocked: true
---
# Visible to AI
```

Exact parser rules (`crates/notia-core/src/note.rs` and `crates/notia-mcp/src/gate.rs`):

- Frontmatter must start on line 1 with `---` (no leading whitespace or UTF-8 BOM before `---`) and close with `---` or `...`.
- The key is case-insensitive (`unlocked` / `Unlocked`) and unindented; the value must evaluate to `true` (case-insensitive, optional single or double quotes).
- **Forms that stay locked**:
  - `unlocked: false`, `unlocked: yes`, `unlocked: 1`
  - Inline comments such as `unlocked: true # ok`
  - Indented keys (`  unlocked: true`) or keys in the note body outside frontmatter
  - Hidden files and folders (any segment starting with `.`, such as `.notia/` or `.trash/`)
  - Non-Markdown files (including `.canvas`, `.kanban`, images, `.txt`)
  - Symlinks (both symlinked files and symlinked directories)

### Exposed MCP tools

`notia-mcp` implements MCP protocol versions `2024-11-05`, `2025-03-26` (default), and `2025-06-18`, and exposes three tools (`crates/notia-mcp/src/rpc.rs`):

| Tool | Arguments | Behavior |
| --- | --- | --- |
| `list_notes` | `{}` | Returns a JSON array of `{"path", "title"}` for unlocked `.md` notes under `--max-bytes`. Locked notes, hidden folders, non-`.md` files, symlinks, and oversized files are omitted. |
| `read_note` | `{"path": "..."}` | Returns the full raw Markdown text of an unlocked note, including its frontmatter. |
| `write_note` | `{"path": "...", "content": "..."}` | Atomically replaces an existing unlocked `.md` note. Cannot create new notes, cannot write locked notes, and refuses content that removes or invalidates `unlocked: true`. |

Paths are vault-relative (e.g. `Daily/Today.md`; both `/` and `\` separators are accepted and normalized). Tool errors return normal MCP results with `isError: true`:

- `note is locked` — file exists as a valid Markdown path inside the vault, but is not unlocked.
- `note is not available` — file is missing, hidden, non-Markdown, a symlink, outside the vault root, or uses an invalid path (`..`, absolute paths, drive colons, control chars, empty segments).
- `note is larger than the server allows` — file or write payload exceeds `--max-bytes`.
- ``the note must stay unlocked; keep `unlocked: true` in its frontmatter`` — `write_note` tried to remove `unlocked: true`.

---

## 6. Connecting an MCP client

### Streamable HTTP client config

```json
{
  "mcpServers": {
    "notia": {
      "url": "http://127.0.0.1:8788/mcp",
      "headers": {
        "Authorization": "Bearer YOUR_TOKEN"
      }
    }
  }
}
```

Replace `YOUR_TOKEN` with the token from `NOTIA_MCP_TOKEN`. Some clients expect an explicit transport key such as `"type": "http"` or `"type": "streamable-http"`; keep the same URL and `Authorization` header.

### Stdio client config

```json
{
  "mcpServers": {
    "notia": {
      "command": "notia-mcp",
      "args": ["--stdio", "--vault", "/path/to/vault"]
    }
  }
}
```

If `notia-mcp` is not on your `PATH`, set `"command"` to the full path of your built binary (for example `/path/to/notia/target/release/notia-mcp` or `C:\\path\\to\\notia\\target\\release\\notia-mcp.exe`).

### Quick verification

Verify HTTP mode with `curl`:

```sh
curl -fsS http://127.0.0.1:8788/v1/health

curl -fsS -X POST http://127.0.0.1:8788/mcp \
  -H "Authorization: Bearer $NOTIA_MCP_TOKEN" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_notes","arguments":{}}}'
```

Verify an unauthenticated request is rejected (`401`):

```sh
curl -s -o /dev/null -w "%{http_code}\n" -X POST http://127.0.0.1:8788/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize"}'
```

Verify `--stdio` mode directly:

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_notes","arguments":{}}}' \
  | notia-mcp --stdio --vault ~/Notes
```

---

## 7. Security notes

- **The token is the only credential on HTTP.** Anyone who can reach the bind address and knows the token can list, read, and overwrite every unlocked note in the vault.
- **Prefer `NOTIA_MCP_TOKEN` over `--token`.** Command-line arguments passed via `--token` can be read from process listings (`ps` on Unix, `Win32_Process` on Windows) by other local users. Set `NOTIA_MCP_TOKEN` in the environment (or `docker/mcp/.env`) instead.
- **HTTP traffic is plaintext and unthrottled.** `--listen` defaults to `127.0.0.1:8788` (loopback only). There is no built-in TLS and no built-in rate limiting. If you publish the server beyond localhost (for example `--listen 0.0.0.0:8788`), bind only to a private network or place an HTTPS reverse proxy in front of `notia-mcp`.
- **Browser Origin protection.** When an HTTP request includes an `Origin` header, `POST /mcp` allows only `http://` or `https://` origins for `localhost`, `127.0.0.1`, or `[::1]` and returns `403` for everything else (including `Origin: null`). Authentication is checked before the request body is buffered.
- **Docker container privileges.** `docker/mcp/compose.yaml` defaults to `user: "${NOTIA_UID:-0}:${NOTIA_GID:-0}"` with `${NOTIA_VAULT}:/vault` mounted read-write. Running as root (`0:0`) over a host bind mount lets the container alter ownership and permissions inside that vault directory on the host. On Linux, set `NOTIA_UID` and `NOTIA_GID` in `docker/mcp/.env` to your own `id -u` and `id -g`, keep the default `127.0.0.1:8788:8788` port binding, and consider adding `cap_drop: [ALL]` and `security_opt: ["no-new-privileges:true"]` in `compose.yaml`.
- **Path and symlink boundaries.** Every path goes through `Host::load`, which rejects control characters, traversal (`..`), absolute paths, colons, hidden paths, non-Markdown extensions, and any symlink (`symlink_metadata`) plus post-canonicalization root containment (`canonicalize().starts_with(vault.root())`).
- **Existence and size signals for locked paths.** If a token-holder guesses a path directly via `read_note` or `write_note`, a locked Markdown file returns `note is locked` while a nonexistent path returns `note is not available`, and `--max-bytes` is checked before reading frontmatter (`note is larger than the server allows`). Keep the token secret and the listener private.
- **Concurrent edits.** `write_note` performs an atomic temp-file-and-rename write with no modification-time or ETag check. If you edit the same unlocked note in Notia and via MCP at the same time, the last write wins.
