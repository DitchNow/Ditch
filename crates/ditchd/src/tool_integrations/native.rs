//! Versioned ABI to the optional, in-process Swift automation module.
use ditch_core::tool_integrations::MAX_RESULT_BYTES;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

pub type Completion = extern "C" fn(u64, *const u8, usize);
pub type Invoke = unsafe extern "C" fn(u64, *const u8, usize, Completion) -> i32;
pub type Cancel = extern "C" fn(u64);
pub type Revoke = extern "C" fn();
pub type Permission = extern "C" fn(u32);
pub type VerifyPeer = unsafe extern "C" fn(i32, u32, *const u8, usize) -> i32;
pub type Forget = unsafe extern "C" fn(*const u8, usize);

pub struct Api {
    pub invoke: Invoke,
    pub cancel: Cancel,
    pub revoke: Revoke,
    pub permission: Permission,
    pub verify_peer: VerifyPeer,
    pub forget: Forget,
    pub host_name: String,
}
static API: OnceLock<Api> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static QUARANTINED: AtomicBool = AtomicBool::new(false);
struct Pending {
    tx: mpsc::Sender<Value>,
    cancelled: Arc<AtomicBool>,
    _desktop_lock: Option<File>,
}
type PendingTable = (Mutex<HashMap<u64, Pending>>, Condvar);
static PENDING: OnceLock<PendingTable> = OnceLock::new();
fn pending() -> &'static PendingTable {
    PENDING.get_or_init(|| (Mutex::new(HashMap::new()), Condvar::new()))
}

pub fn register(api: Api) -> bool {
    API.set(api).is_ok()
}
pub fn available() -> bool {
    API.get().is_some() && !QUARANTINED.load(Ordering::Acquire)
}
pub fn host_name() -> String {
    API.get()
        .map(|a| a.host_name.clone())
        .unwrap_or_else(|| "This Mac".into())
}

pub fn forget_session(run: uuid::Uuid) {
    if let Some(api) = API.get() {
        let session = run.to_string();
        unsafe {
            (api.forget)(session.as_ptr(), session.len());
        }
    }
}

/// The exact expected executable is provided by Ditch, never the peer.
pub fn verify_peer(pid: i32, role: u32, expected: &std::path::Path) -> bool {
    let Some(api) = API.get() else { return false };
    let path = expected.to_string_lossy();
    // SAFETY: Swift borrows these bytes synchronously and never retains them.
    unsafe { (api.verify_peer)(pid, role, path.as_ptr(), path.len()) == 0 }
}

pub fn request_permission(kind: u32) {
    if let Some(api) = API.get() {
        (api.permission)(kind);
    }
}

extern "C" fn complete(id: u64, bytes: *const u8, length: usize) {
    let value = if bytes.is_null() || length > MAX_RESULT_BYTES {
        serde_json::json!({"ok":false,"error":"invalidNativeResult"})
    } else {
        // SAFETY: the signed native module guarantees a live borrowed buffer
        // for this callback. JSON is copied before returning to Swift.
        let data = unsafe { std::slice::from_raw_parts(bytes, length) };
        serde_json::from_slice(data)
            .unwrap_or_else(|_| serde_json::json!({"ok":false,"error":"invalidNativeResult"}))
    };
    let (table, wake) = pending();
    let mut table = table.lock().unwrap();
    if let Some(entry) = table.remove(&id) {
        let _ = entry.tx.send(value);
    }
    if table.is_empty() {
        QUARANTINED.store(false, Ordering::Release);
    }
    wake.notify_all();
}

/// The desktop lock is retained until actual native completion, including if
/// the caller times out. A hung operation cannot release ownership prematurely.
pub fn invoke(request: &Value, cancelled: &Arc<AtomicBool>, desktop_lock: Option<File>) -> Value {
    let Some(api) = API.get().filter(|_| available()) else {
        return serde_json::json!({"ok":false,"error":"unavailable"});
    };
    let bytes = match serde_json::to_vec(request) {
        Ok(b) if b.len() <= ditch_core::tool_integrations::MAX_REQUEST_BYTES => b,
        _ => return serde_json::json!({"ok":false,"error":"invalidArguments"}),
    };
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    {
        let mut table = pending().0.lock().unwrap();
        if !table.is_empty() || cancelled.load(Ordering::Acquire) {
            return serde_json::json!({"ok":false,"error":"busyOrRevoked"});
        }
        table.insert(
            id,
            Pending {
                tx,
                cancelled: cancelled.clone(),
                _desktop_lock: desktop_lock,
            },
        );
    }
    // SAFETY: Swift copies input synchronously and uses the stable C callback.
    if unsafe { (api.invoke)(id, bytes.as_ptr(), bytes.len(), complete) } != 0 {
        pending().0.lock().unwrap().remove(&id);
        pending().1.notify_all();
        return serde_json::json!({"ok":false,"error":"busy"});
    }
    if cancelled.load(Ordering::Acquire) {
        (api.cancel)(id);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(value) => return value,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return serde_json::json!({"ok":false,"error":"outcome_unknown"});
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
            (api.cancel)(id);
            if let Ok(value) = rx.recv_timeout(Duration::from_secs(2)) {
                return value;
            }
            // Keep the pending entry (and kernel desktop lock) until completion.
            let table = pending().0.lock().unwrap();
            if table.contains_key(&id) {
                QUARANTINED.store(true, Ordering::Release);
            }
            return serde_json::json!({"ok":false,"error":"outcome_unknown"});
        }
    }
}

pub fn revoke_and_drain() -> bool {
    let Some(api) = API.get() else { return true };
    (api.revoke)();
    let (table, wake) = pending();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut table = table.lock().unwrap();
    while !table.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            QUARANTINED.store(true, Ordering::Release);
            return false;
        }
        table = wake.wait_timeout(table, remaining).unwrap().0;
    }
    true
}

pub fn drain_scope(cancelled: &Arc<AtomicBool>) -> bool {
    cancelled.store(true, Ordering::Release);
    let Some(api) = API.get() else { return true };
    let (table, wake) = pending();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut table = table.lock().unwrap();
    loop {
        let ids: Vec<u64> = table
            .iter()
            .filter(|(_, p)| Arc::ptr_eq(&p.cancelled, cancelled))
            .map(|(id, _)| *id)
            .collect();
        if ids.is_empty() {
            return true;
        }
        for id in ids {
            (api.cancel)(id);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            QUARANTINED.store(true, Ordering::Release);
            return false;
        }
        table = wake
            .wait_timeout(table, remaining.min(Duration::from_millis(20)))
            .unwrap()
            .0;
    }
}
