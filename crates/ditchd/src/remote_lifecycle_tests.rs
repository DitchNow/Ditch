use super::*;

// Real disposable processes and Unix sockets exercise the legacy wire contract
// and handoff. No service is installed and no user's remote host is contacted.
const DAEMON: &str = r#"#!/usr/bin/env python3
import json, os, socket, sys, uuid
from pathlib import Path
root = Path(os.environ['HOME']) / '.ditch'
build = '__BUILD__'
modern = __MODERN__
identity = str(uuid.uuid4())
sock = root / 'run/ditchd.sock'
if sys.argv[1:] == ['version-json']:
    print(json.dumps({'build_identifier': build}))
    sys.exit(0)
sock.parent.mkdir(parents=True, exist_ok=True)
sock.unlink(missing_ok=True)
server = socket.socket(socket.AF_UNIX)
server.bind(str(sock)); server.listen(8)
while True:
    client, _ = server.accept()
    with client:
        line = client.makefile('rb').readline()
        if not line: continue
        request = json.loads(line)
        body = request['body']; exiting = False
        active = int((root/'busy').read_text()) if (root/'busy').exists() else 0
        if body == 'RuntimeStatus':
            response = {'RuntimeStatus': {'identity':'fixture', 'edition':'__EDITION__',
                'pid':os.getpid(), 'socket_path':str(sock), 'active_session_count':active,
                'build_identifier':build, 'instance_id':identity,
                'capabilities':['remote_runtime_protocol_v4'] + (['remote_maintenance_v1'] if modern else [])}}
        elif isinstance(body, dict) and 'RemoteRuntimeMaintenance' in body:
            operation = body['RemoteRuntimeMaintenance']
            if active or operation['instance_id'] != identity:
                response = {'Error': {'code':'runtime_busy', 'message':'busy or changed'}}
            else:
                response = 'Accepted'
                exiting = operation['action'] == 'exit'
        elif body == 'Shutdown':
            response = 'Accepted'; exiting = True
        else:
            response = {'Error': {'code':'unexpected', 'message':'unexpected request'}}
        request['body'] = response
        client.sendall((json.dumps(request)+'\n').encode())
    if exiting: break
server.close(); sock.unlink(missing_ok=True)
"#;

struct Fixture {
    paths: AppPaths,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("lc-{}", &Uuid::new_v4().simple().to_string()[..8]));
        let data = root.join(".ditch");
        fs::create_dir_all(data.join("run")).unwrap();
        fs::create_dir_all(data.join("bin")).unwrap();
        fs::create_dir_all(data.join("state")).unwrap();
        fs::write(data.join("state/saved-session"), "keep me").unwrap();
        let paths = AppPaths {
            socket_path: data.join("run/ditchd.sock"),
            database_path: data.join("state/ditch.sqlite3"),
            logs_dir: data.join("logs"),
            scrollback_dir: data.join("state/scrollback"),
            data_dir: data,
        };
        Self { paths, root }
    }
    fn release(&self, name: &str, modern: bool) -> PathBuf {
        let relative = PathBuf::from(format!("versions/fixture/target/{name}"));
        let binary = self
            .paths
            .data_dir
            .join("bin")
            .join(&relative)
            .join("ditchd");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(
            &binary,
            DAEMON
                .replace("__BUILD__", name)
                .replace("__MODERN__", if modern { "True" } else { "False" })
                .replace("__EDITION__", crate::RUNTIME_EDITION),
        )
        .unwrap();
        fs::set_permissions(binary, fs::Permissions::from_mode(0o700)).unwrap();
        relative
    }
    fn running(&self, release: &Path, build: &str) -> RuntimeStatus {
        activate(&self.paths, Some(release)).unwrap();
        start(&self.paths, Manager::Direct).unwrap();
        wait_build(&self.paths, build, crate::RUNTIME_EDITION, None).unwrap()
    }
    fn journal(&self, old: &RuntimeStatus, previous: PathBuf, replacement: PathBuf) -> Journal {
        Journal {
            manager: Manager::Direct,
            previous: Some(previous),
            replacement,
            previous_build: Some(old.build_identifier.clone()),
            previous_edition: Some(old.edition.clone()),
            replacement_build: "new".into(),
            token: Uuid::new_v4(),
            old_instance: Some(old.instance_id),
            phase: "prepared".into(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.paths.data_dir.join("busy"));
        if let Ok(Some(running)) = status(&self.paths) {
            let _ = stop(&self.paths, &running, Manager::Direct, Uuid::new_v4(), true);
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn legacy_direct_upgrade_requires_confirmation_then_replaces_actual_process() {
    let f = Fixture::new();
    let old_path = f.release("old", false);
    let new_path = f.release("new", true);
    let old = f.running(&old_path, "old");
    assert!(
        prepare(&f.paths, &old, Uuid::new_v4(), false)
            .unwrap_err()
            .to_string()
            .contains("legacy_maintenance_required")
    );
    assert_eq!(status(&f.paths).unwrap().unwrap().pid, old.pid);
    let journal = f.journal(&old, old_path, new_path.clone());
    let _lock = lock(&f.paths, true).unwrap();
    handoff(&f.paths, journal, Some(&old), true).unwrap();
    let current = status(&f.paths).unwrap().unwrap();
    assert_eq!(current.build_identifier, "new");
    assert_ne!(old.pid, current.pid);
    assert_ne!(old.instance_id, current.instance_id);
    assert_eq!(
        fs::read_link(f.paths.data_dir.join("bin/current")).unwrap(),
        new_path
    );
    assert_eq!(
        fs::read_to_string(f.paths.data_dir.join("state/saved-session")).unwrap(),
        "keep me"
    );
    assert!(!journal_path(&f.paths).exists());
}

#[test]
fn busy_legacy_runtime_is_not_stopped_even_after_confirmation() {
    let f = Fixture::new();
    let old_path = f.release("old", false);
    let old = f.running(&old_path, "old");
    fs::write(f.paths.data_dir.join("busy"), "1").unwrap();
    let error = stop(&f.paths, &old, Manager::Direct, Uuid::new_v4(), true).unwrap_err();
    assert!(error.to_string().contains("active"));
    assert_eq!(status(&f.paths).unwrap().unwrap().pid, old.pid);
}

#[test]
fn failed_replacement_start_restores_and_restarts_previous_runtime() {
    let f = Fixture::new();
    let old_path = f.release("old", false);
    let old = f.running(&old_path, "old");
    let missing = PathBuf::from("versions/fixture/target/missing");
    let journal = f.journal(&old, old_path.clone(), missing);
    let error = handoff(&f.paths, journal, Some(&old), true).unwrap_err();
    assert!(error.to_string().contains("previous runtime restored"));
    let current = status(&f.paths).unwrap().unwrap();
    assert_eq!(current.build_identifier, "old");
    assert_ne!(current.pid, old.pid);
    assert_eq!(
        fs::read_link(f.paths.data_dir.join("bin/current")).unwrap(),
        old_path
    );
    assert!(!journal_path(&f.paths).exists());
}

#[test]
fn completed_activation_with_lost_reply_is_recovered_without_restart() {
    let f = Fixture::new();
    let old_path = f.release("old", false);
    let old = f.running(&old_path, "old");
    let new_path = f.release("new", true);
    let mut journal = f.journal(&old, old_path, new_path.clone());
    stop(&f.paths, &old, Manager::Direct, journal.token, true).unwrap();
    journal.phase = "activated".into();
    write_json(&journal_path(&f.paths), &journal).unwrap();
    let current = f.running(&new_path, "new");
    assert!(recover(&f.paths, &journal, true).unwrap());
    assert_eq!(status(&f.paths).unwrap().unwrap().pid, current.pid);
}

#[test]
fn upgrade_lock_and_journal_prevent_reconnect_bootstrap_and_duplicate_repair() {
    let f = Fixture::new();
    let owner = lock(&f.paths, true).unwrap();
    assert!(lock(&f.paths, true).is_err());
    assert!(bootstrap_lock(&f.paths).is_err());
    drop(owner);
    fs::write(journal_path(&f.paths), "{}").unwrap();
    assert!(bootstrap_lock(&f.paths).is_err());
    fs::remove_file(journal_path(&f.paths)).unwrap();
    assert!(bootstrap_lock(&f.paths).is_ok());
}

#[test]
fn release_paths_cannot_escape_runtime_versions() {
    for path in [
        "/tmp/other",
        "versions/../../state",
        "../versions/build",
        "state/data",
    ] {
        assert!(validate_release(Path::new(path)).is_err());
    }
    assert!(validate_release(Path::new("versions/0.1.1/linux/digest")).is_ok());
}

#[test]
fn service_ownership_requires_the_actual_daemon_pid() {
    for manager in [Manager::Systemd, Manager::Launchd] {
        assert_eq!(owned_manager(Some(42), manager, 42).unwrap(), manager);
        assert!(owned_manager(Some(42), manager, 41).is_err());
        assert!(owned_manager(Some(42), manager, 0).is_err());
        assert!(owned_manager(None, manager, 42).is_err());
    }
}

#[test]
fn old_install_request_does_not_implicitly_confirm_legacy_maintenance() {
    let request: ClientRequest = serde_json::from_value(serde_json::json!({
        "InstallRemoteRuntime": {"alias": "fixture"}
    }))
    .unwrap();
    assert!(matches!(
        request,
        ClientRequest::InstallRemoteRuntime {
            legacy_maintenance_confirmed: false,
            ..
        }
    ));
}

#[test]
fn modern_direct_upgrade_and_repeated_repair_preserve_data() {
    let f = Fixture::new();
    let old_path = f.release("old", true);
    let old = f.running(&old_path, "old");
    let new_path = f.release("new", true);
    let journal = f.journal(&old, old_path, new_path.clone());
    handoff(&f.paths, journal, Some(&old), false).unwrap();
    let current = status(&f.paths).unwrap().unwrap();
    let journal = f.journal(&current, new_path.clone(), new_path);
    handoff(&f.paths, journal, Some(&current), false).unwrap();
    assert_eq!(status(&f.paths).unwrap().unwrap().build_identifier, "new");
    assert_eq!(
        fs::read_to_string(f.paths.data_dir.join("state/saved-session")).unwrap(),
        "keep me"
    );
}

#[test]
fn interrupted_pointer_switch_restores_old_link_without_restarting_old_daemon() {
    let f = Fixture::new();
    let old_path = f.release("old", false);
    let old = f.running(&old_path, "old");
    let new_path = f.release("new", true);
    let mut journal = f.journal(&old, old_path.clone(), new_path.clone());
    journal.phase = "switching".into();
    write_json(&journal_path(&f.paths), &journal).unwrap();
    activate(&f.paths, Some(&new_path)).unwrap();
    assert!(!recover(&f.paths, &journal, true).unwrap());
    assert_eq!(status(&f.paths).unwrap().unwrap().pid, old.pid);
    assert_eq!(
        fs::read_link(f.paths.data_dir.join("bin/current")).unwrap(),
        old_path
    );
}
