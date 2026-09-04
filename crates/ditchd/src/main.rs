// Community `ditchd` composition root.
//
// All authoritative local/SSH execution lives in the Community-owned shared
// runtime. Edition-specific builds provide statically linked hooks; they do
// not launch plugins or a second service.

mod codex_app_server;
mod edition;
mod ssh_remote;

include!("runtime_shared.rs");
