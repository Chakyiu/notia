//! The sync server a Notia user runs on a computer they control.
//!
//! ```text
//! notia-sync-server --listen 0.0.0.0:8787
//! ```
//!
//! The first run writes a token to `<data>/token` and prints it. Notia sends
//! that token on every request. It is the only password.

use std::{env, fs, io::Write as _, path::PathBuf, process};

use notia_sync::{MAX_FILE_BYTES, Store};

fn main() {
    if let Err(error) = run() {
        eprintln!("notia-sync-server: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;
    if args.help {
        print_help();
        return Ok(());
    }
    fs::create_dir_all(&args.data)
        .map_err(|error| format!("could not create {}: {error}", args.data.display()))?;
    let token = load_token(&args)?;
    let store = Store::open(&args.data, args.max_bytes).map_err(|error| error.to_string())?;
    let listener = notia_sync::bind(&args.listen).map_err(|error| error.to_string())?;
    eprintln!("Listening on {}", listener.url());
    eprintln!("Vaults are stored in {}", args.data.display());
    eprintln!(
        "The token is the only password. Anyone who can reach this address and knows it can read and change synced files."
    );
    listener.serve(store, &token);
    Ok(())
}

struct Args {
    help: bool,
    data: PathBuf,
    listen: String,
    token: Option<String>,
    max_bytes: u64,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = Self {
            help: false,
            data: env::current_dir()
                .map_err(|error| format!("could not find the current directory: {error}"))?
                .join("notia-sync-data"),
            listen: "0.0.0.0:8787".into(),
            token: None,
            max_bytes: MAX_FILE_BYTES,
        };
        let mut rest = env::args().skip(1);
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "-h" | "--help" => args.help = true,
                "--data" => args.data = PathBuf::from(need(&mut rest, "--data")?),
                "--listen" => args.listen = need(&mut rest, "--listen")?,
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

fn load_token(args: &Args) -> Result<String, String> {
    if let Some(token) = &args.token {
        check_token(token)?;
        eprintln!("Using the token from the command line.");
        return Ok(token.clone());
    }
    if let Ok(token) = env::var("NOTIA_SYNC_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            check_token(&token)?;
            eprintln!("Using NOTIA_SYNC_TOKEN.");
            return Ok(token);
        }
    }
    let path = args.data.join("token");
    if let Ok(token) = fs::read_to_string(&path) {
        let token = token.trim().to_string();
        if !token.is_empty() {
            check_token(&token)?;
            eprintln!("Using the token in {}.", path.display());
            return Ok(token);
        }
    }
    let token = random_token()?;
    write_token(&path, &token)?;
    eprintln!("A new token was written to {}.", path.display());
    eprintln!("Copy it into Notia → Settings → Sync. It will not be shown again.");
    eprintln!("{token}");
    Ok(token)
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

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| format!("could not generate a token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn write_token(path: &PathBuf, token: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let mut file = {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        options
            .open(path)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?
    };
    writeln!(file, "{token}")
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    Ok(())
}

fn print_help() {
    eprintln!(
        "\
Usage: notia-sync-server [--data DIR] [--listen ADDR] [--token TOKEN] [--max-bytes N]

  --data DIR       Folder the server keeps vaults in. Default: ./notia-sync-data
  --listen ADDR    Address to bind. Default: 0.0.0.0:8787
  --token TOKEN    Shared secret. Also read from NOTIA_SYNC_TOKEN, then from DIR/token.
                   The first run writes a token to DIR/token and prints it once.
  --max-bytes N    Largest accepted file, in bytes. Default: {max}

In Notia, open Settings → Sync and enter the address, such as
http://192.168.1.20:8787, and the token. Computers that share a vault use the
same vault id.

The token is the only password. Bind to an address only your computers can
reach, or put HTTPS in front of this process. Notia trusts the certificate
store of the system it runs on.",
        max = MAX_FILE_BYTES
    );
}
