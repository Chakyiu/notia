//! MCP host for one Notia vault.
//!
//! A Markdown note is unlocked only when its frontmatter contains
//! `unlocked: true` ([`notia_core::NoteMeta::unlocked`]). Locked notes are
//! omitted from listings. Reading or writing them fails and does not return
//! their contents. The process can speak streamable HTTP, the same shape of
//! host as `notia-sync-server`, or MCP on stdin and stdout.

mod gate;
mod rpc;
mod serve;

pub use gate::{DEFAULT_MAX_NOTE_BYTES, Host};
pub use serve::{Listener, bind, serve_stdio};
