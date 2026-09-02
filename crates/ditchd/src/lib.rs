//! Embeddable entry point for the macOS runtime host.
//!
//! The normal `ditchd` binary and the macOS menu-bar runtime use the same
//! implementation. On macOS the Swift host links this crate statically and
//! runs the socket server on a background thread while AppKit owns the main
//! thread.

#![allow(clippy::items_after_test_module)]

include!("main.rs");

#[unsafe(no_mangle)]
pub extern "C" fn ditch_runtime_run() -> i32 {
    match run_runtime() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{RUNTIME_IDENTITY} failed: {error}");
            1
        }
    }
}
