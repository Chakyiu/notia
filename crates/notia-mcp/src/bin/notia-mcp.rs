//! MCP server for unlocked notes in a Notia vault.
//!
//! ```text
//! notia-mcp --vault ~/Notes --token "$NOTIA_MCP_TOKEN"
//! ```
//!
//! A note is unlocked only when its frontmatter contains `unlocked: true`.

use std::{env, path::PathBuf, process};

use notia_mcp::{DEFAULT_MAX_NOTE_BYTES, Host, bind, serve_stdio};

fn main() {
    if let Err(error) = run() {
        eprintln!("notia-mcp: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;
    if args.help {
        print_help();
        return Ok(());
    }
    if args.stdio && args.listen_set {
        return Err("--stdio does not use --listen".into());
    }
    if args.stdio && args.token.is_some() {
        return Err("--stdio does not use --token".into());
    }
    let vault = vault_path(args.vault)?;
    let host = Host::open(&vault, args.max_bytes)?;
    if args.stdio {
        eprintln!(
            "Reading MCP messages on stdin. The vault is {}.",
            host.root().display()
        );
        eprintln!("Locked notes are not served. Stdio does not check a token.");
        return serve_stdio(&host);
    }
    let token = load_token(args.token)?;
    let listener = bind(&args.listen)?;
    eprintln!("Listening on {}/mcp", listener.url());
    eprintln!("Vault is {}", host.root().display());
    eprintln!(
        "The token is the only password. Locked notes are not served. Anyone who can reach this address and knows the token can read and change unlocked notes."
    );
    listener.serve(host, &token);
    Ok(())
}

struct Args {
    help: bool,
    stdio: bool,
    listen_set: bool,
    vault: Option<String>,
    listen: String,
    token: Option<String>,
    max_bytes: u64,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = Self {
            help: false,
            stdio: false,
            listen_set: false,
            vault: None,
            listen: "127.0.0.1:8788".into(),
            token: None,
            max_bytes: DEFAULT_MAX_NOTE_BYTES,
        };
        let mut rest = env::args().skip(1);
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "-h" | "--help" => args.help = true,
                "--stdio" => args.stdio = true,
                "--vault" => args.vault = Some(need(&mut rest, "--vault")?),
                "--listen" => {
                    args.listen_set = true;
                    args.listen = need(&mut rest, "--listen")?;
                }
                "--token" => args.token = Some(need(&mut rest, "--token")?),
                "--max-bytes" => {
                    let value = need(&mut rest, "--max-bytes")?;
                    args.max_bytes = value
                        .parse()
                        .map_err(|_| format!("--max-bytes needs a number of bytes, not {value}"))?;
                    if args.max_bytes == 0 {
                        return Err("--max-bytes must be at least 1".into());
                    }
                }
                other => return Err(format!("unknown argument {other}. Try --help")),
            }
        }
        Ok(args)
    }
}

fn need(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn vault_path(flag: Option<String>) -> Result<PathBuf, String> {
    if let Some(path) = flag {
        if path.trim().is_empty() {
            return Err("--vault needs a folder".into());
        }
        return Ok(PathBuf::from(path));
    }
    if let Ok(path) = env::var("NOTIA_MCP_VAULT") {
        let path = path.trim().to_string();
        if !path.is_empty() {
            eprintln!("Using NOTIA_MCP_VAULT.");
            return Ok(PathBuf::from(path));
        }
    }
    Err("--vault is required. It is also read from NOTIA_MCP_VAULT.".into())
}

fn load_token(flag: Option<String>) -> Result<String, String> {
    if let Some(token) = flag {
        check_token(&token)?;
        eprintln!("Using the token from the command line.");
        return Ok(token);
    }
    if let Ok(token) = env::var("NOTIA_MCP_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            check_token(&token)?;
            eprintln!("Using NOTIA_MCP_TOKEN.");
            return Ok(token);
        }
    }
    Err("HTTP needs a token. Pass --token or set NOTIA_MCP_TOKEN.".into())
}

fn check_token(token: &str) -> Result<(), String> {
    if token.is_empty()
        || token.len() > 512
        || !token.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(
            "the token must be 1–512 printable characters without spaces or newlines".into(),
        );
    }
    Ok(())
}

fn print_help() {
    eprintln!(
        "\
Usage: notia-mcp --vault DIR [--listen ADDR] [--token TOKEN] [--max-bytes N]
       notia-mcp --stdio --vault DIR [--max-bytes N]

  --vault DIR      Vault folder. Also read from NOTIA_MCP_VAULT.
  --listen ADDR    Address to bind. Default: 127.0.0.1:8788
  --token TOKEN    Shared secret. Also read from NOTIA_MCP_TOKEN.
                   HTTP requests send it as Authorization: Bearer TOKEN.
  --max-bytes N    Largest note, in bytes. Default: {max}
  --stdio          Speak MCP on stdin and stdout instead of HTTP. No token.

A note is unlocked only when its frontmatter contains `unlocked: true`.
Other notes are locked: they are not listed, and read or write returns an
error without their contents. Writes cannot create a note, and a write must
keep the `unlocked: true` line.

The token is the only password on HTTP. The default bind is localhost.
Anyone who can reach the address and knows the token can read and change
unlocked notes. Put HTTPS in front of this process if you publish it.

GET /v1/health needs no token. MCP messages are POST /mcp.
",
        max = DEFAULT_MAX_NOTE_BYTES
    );
}
