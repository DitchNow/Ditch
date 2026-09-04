// Proprietary Commercial `ditchd` composition root.
//
// This statically composes private capability providers into the exact
// Community runtime implementation pinned under `community/`. It still emits
// one executable and owns one process/session/store authority.

#[path = "../../../community/crates/ditchd/src/codex_app_server.rs"]
mod codex_app_server;
mod edition;
mod remote_control;
#[path = "../../../community/crates/ditchd/src/ssh_remote.rs"]
mod ssh_remote;

include!("../../../community/crates/ditchd/src/runtime_shared.rs");
