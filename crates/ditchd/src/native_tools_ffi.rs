//! Exported registration boundary shared by both embedded runtime libraries.
use crate::tool_integrations::native;

/// # Safety
/// Called once by the trusted Swift StatusHost with function pointers from the
/// signed, ABI-checked module. That module must remain loaded for process life.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ditch_register_automation(
    version: u32,
    invoke: Option<native::Invoke>,
    cancel: Option<native::Cancel>,
    revoke: Option<native::Revoke>,
    permission: Option<native::Permission>,
    verify_peer: Option<native::VerifyPeer>,
    forget: Option<native::Forget>,
    name: *const u8,
    name_length: usize,
) -> i32 {
    if version != 3 || name.is_null() || name_length > 256 {
        return 1;
    }
    let (
        Some(invoke),
        Some(cancel),
        Some(revoke),
        Some(permission),
        Some(verify_peer),
        Some(forget),
    ) = (invoke, cancel, revoke, permission, verify_peer, forget)
    else {
        return 1;
    };
    let host_name =
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(name, name_length) })
            .into_owned();
    if native::register(native::Api {
        invoke,
        cancel,
        revoke,
        permission,
        verify_peer,
        forget,
        host_name,
    }) {
        0
    } else {
        1
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ditch_desktop_session_unavailable() {
    crate::tool_integrations::revoke_all();
}
