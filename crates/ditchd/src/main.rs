// Community `ditchd` composition root.
//
// All authoritative local/SSH execution lives in the Community-owned shared
// runtime. Edition-specific builds provide statically linked hooks; they do
// not launch plugins or a second service.

mod acceptance_evidence;
mod project_clone;
mod app_server_client;
mod codex_app_server;
mod session_namer;
mod task_planner;
mod edition;
mod github;
mod skill_files;
mod ssh_remote;
mod remote_lifecycle;
mod remote_process;

include!("runtime_shared.rs");
