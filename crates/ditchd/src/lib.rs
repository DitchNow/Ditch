//! Embeddable entry point for the macOS runtime host.
//!
//! The normal `ditchd` binary and the macOS menu-bar runtime use the same
//! implementation. On macOS the Swift host links this crate statically and
//! runs the socket server on a background thread while AppKit owns the main
//! thread.

#![allow(clippy::items_after_test_module)]

include!("main.rs");

/// Supplies a release-only credential directly to the in-process macOS
/// runtime before its request loop starts. The standalone daemon never calls
/// this entry point.
///
/// # Safety
///
/// `bytes` must address `length` readable bytes and remain valid for the
/// duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ditch_set_official_build_credential(
    bytes: *const u8,
    length: usize,
) -> i32 {
    if bytes.is_null() || length == 0 || length > 128 {
        return 1;
    }
    // SAFETY: The Swift caller passes a contiguous buffer that remains alive
    // for this call. The credential is copied before the function returns.
    let credential = unsafe { std::slice::from_raw_parts(bytes, length) };
    if ditch_upgrade::configure_official_build_credential(credential) {
        0
    } else {
        1
    }
}

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
