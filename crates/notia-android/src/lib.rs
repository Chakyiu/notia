//! The Android app's view of a Notia vault.
//!
//! The screens are Kotlin. They send one JSON request at a time and get one
//! JSON response. Notes, search, and sync are the same code the desktop app uses.
//! GPUI has no Android backend, so this crate does not draw anything.

mod service;

#[cfg(target_os = "android")]
mod android;

pub use service::handle;
