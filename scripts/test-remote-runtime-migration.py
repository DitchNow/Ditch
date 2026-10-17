#!/usr/bin/env python3
"""Disposable native v140-source -> candidate migration smoke test.
Never targets the real user home, SSH, or a service manager.
"""
import argparse, ctypes, datetime, fcntl, hashlib, json, os, pathlib, platform, shlex, shutil, signal, socket, sqlite3, struct, subprocess, sys, tempfile, termios, time, uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--previous", type=pathlib.Path, required=True, help="Native Community 140 executable")
parser.add_argument("--candidate", type=pathlib.Path, required=True, help="Native candidate executable")
args = parser.parse_args()
old, candidate = args.previous.resolve(), args.candidate.resolve()
old_metadata = json.loads(subprocess.check_output([str(old), "version-json"], text=True))
candidate_metadata = json.loads(subprocess.check_output([str(candidate), "version-json"], text=True))
assert old_metadata["build_identifier"] != candidate_metadata["build_identifier"], "Use two different builds"
target = {
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Darwin", "x86_64"): "x86_64-apple-darwin",
    ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("Linux", "aarch64"): "aarch64-unknown-linux-gnu",
}[(platform.system(), platform.machine())]
root = pathlib.Path(tempfile.mkdtemp(prefix="n140-"))
data = root / ".ditch"
release = data / "bin/versions/fixture/old"
release.mkdir(parents=True)
shutil.copy2(old, release / "ditchd")
(data / "bin/current").symlink_to("versions/fixture/old")
env = dict(os.environ, HOME=str(root), SHELL="/bin/sh")
env.pop("CODEX_HOME", None)
log = open(root / "daemon.log", "ab")
child = None
external = None
external_master = None
def rpc(body):
    request = dict(protocol_version=1, id=str(uuid.uuid4()), sent_at=datetime.datetime.now(datetime.timezone.utc).isoformat(), body=body)
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        client.connect(str(data / "run/ditchd.sock"))
        client.sendall((json.dumps(request)+"\n").encode())
        response = json.loads(client.makefile("rb").readline())
        assert response["id"] == request["id"]
        return response["body"]
def ready():
    for _ in range(100):
        try: return rpc("RuntimeStatus")["RuntimeStatus"]
        except (OSError, KeyError, ValueError): time.sleep(.1)
    raise RuntimeError((root / "daemon.log").read_text())
def start_old():
    global child
    child = subprocess.Popen([str(release / "ditchd"), "daemon", "--remote"], env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
    return ready()
def process_info(pid):
    if platform.system() == "Linux":
        try: fields = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        except FileNotFoundError: return None
        return fields[0], fields[19]
    proc = ctypes.CDLL("/usr/lib/libproc.dylib")
    info = ctypes.create_string_buffer(136)
    if proc.proc_pidinfo(pid, 3, ctypes.c_uint64(0), info, 136) != 136: return None
    state = struct.unpack_from("I", info.raw, 4)[0]
    seconds, micros = struct.unpack_from("QQ", info.raw, 120)
    return ("Z" if state == 5 else "T" if state == 4 else "R"), f"{seconds}:{micros}"
def alive(pid):
    info = process_info(pid)
    return info is not None and info[0] != "Z"
def await_file(path):
    for _ in range(100):
        if path.exists() and path.read_text().strip(): return int(path.read_text())
        time.sleep(.05)
    raise AssertionError(f"Terminal command did not start: {path}")
def open_terminal(project_id, command):
    terminal = rpc({"OpenProjectTerminal": dict(project_id=project_id, columns=80, rows=24)})["ProjectTerminal"]
    assert rpc({"WriteProjectTerminal": dict(terminal_id=terminal["id"], data=list((command+"\n").encode()))}) == "Accepted"
    return terminal
def external_session():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)
try:
    first = start_old()
    project = root / "project"
    project.mkdir()
    (project / "KEEP-ME.txt").write_text("preserved")
    result = rpc({"CreateProject": dict(name="Migration fixture", root=str(project), git_policy="AllowOutsideGit")})
    assert "ProjectCreated" in result, result
    project_id = result["ProjectCreated"]["id"]
    rpc("Shutdown")
    child.wait(timeout=10)
    now = datetime.datetime.now(datetime.timezone.utc).isoformat()
    agent_id = str(uuid.uuid4())
    run = dict(id=agent_id, provider="Codex", state="Completed", launch_mode="Exec", project_id=project_id,
               task_id=None, pane_id=None, native_session_id="saved-native-thread", current_prompt="Preserve this conversation",
               last_visible_action="Completed", state_confidence=1.0, state_evidence="fixture", started_at=now, updated_at=now)
    with sqlite3.connect(data / "state/ditch.sqlite3") as db:
        db.execute("""INSERT INTO agents(id,provider,state,launch_mode,project_id,task_id,pane_id,native_session_id,current_prompt,last_visible_action,state_confidence,state_evidence,started_at,updated_at,run_json)
            VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
            (agent_id, json.dumps("Codex"), json.dumps("Completed"), json.dumps("Exec"), project_id, None, None,
             "saved-native-thread", run["current_prompt"], "Completed", 1.0, "fixture", now, now, json.dumps(run)))
        message = dict(agent_id=agent_id, role="User", text="Preserve this conversation", created_at=now)
        db.execute("INSERT INTO agent_messages(agent_id,sequence,message_json,created_at) VALUES(?,?,?,?)",
                   (agent_id, 1, json.dumps(message), now))
    before = start_old()
    assert before["build_identifier"] == old_metadata["build_identifier"]
    assert before["active_session_count"] == 0
    assert any(a["id"] == agent_id for a in rpc("Snapshot")["Snapshot"]["agents"])
    # A disconnected upgrade helper must not leave the old daemon paused.
    owner = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
    watchdog = subprocess.Popen([str(candidate), "remote-handoff-watchdog", str(before["pid"]),
                                 process_info(before["pid"])[1], str(owner.pid)], env=env)
    try:
        os.kill(before["pid"], signal.SIGSTOP)
        owner.kill(); owner.wait(timeout=5)
        watchdog.wait(timeout=5)
        assert watchdog.returncode == 0
        assert ready()["pid"] == before["pid"]
    finally:
        if owner.poll() is None: owner.kill(); owner.wait(timeout=5)
        if watchdog.poll() is None: watchdog.kill(); watchdog.wait(timeout=5)
        os.kill(before["pid"], signal.SIGCONT)
    # An unrelated terminal on the same account must survive the upgrade.
    external_master, slave = os.openpty()
    external = subprocess.Popen(["/bin/sh", "-l"], stdin=slave, stdout=slave, stderr=slave,
                                env=env, preexec_fn=external_session)
    os.close(slave)
    shell_pid = root / "shell.pid"
    command_pid = root / "command.pid"
    open_terminal(project_id, f"echo $$ > {shlex.quote(str(shell_pid))}; sleep 300 & echo $! > {shlex.quote(str(command_pid))}; wait")
    terminal_pids = [await_file(shell_pid), await_file(command_pid)]
    # A shell replaced by exec is still a Ditch terminal, not an unknown job.
    for name, tail in [("exec", "exec sleep 300"), ("idle", ":"), ("stubborn", "trap '' HUP; exec sleep 300")]:
        folder = root / name; folder.mkdir()
        other = rpc({"CreateProject": dict(name=name, root=str(folder), git_policy="AllowOutsideGit")})["ProjectCreated"]
        marker = root / f"{name}.pid"
        open_terminal(other["id"], f"echo $$ > {shlex.quote(str(marker))}; {tail}")
        terminal_pids.append(await_file(marker))
    digest = hashlib.sha256(candidate.read_bytes()).hexdigest()
    command = [str(candidate), "upgrade-remote", target, digest, "automatic"]
    with sqlite3.connect(data / "state/ditch.sqlite3") as db:
        lease = json.dumps(dict(target="local", root=str(project), unconfined=True, coordinator_group=None))
        db.execute("INSERT INTO writer_reservations(agent_id,lease_json) VALUES(?,?)", (agent_id, lease))
    blocked = subprocess.run(command, env=env, capture_output=True, text=True, timeout=40)
    assert blocked.returncode != 0 and "runtime_busy:" in blocked.stderr, blocked.stderr
    assert ready()["pid"] == before["pid"]
    assert all(alive(pid) for pid in terminal_pids)
    assert external.poll() is None
    with sqlite3.connect(data / "state/ditch.sqlite3") as db:
        db.execute("DELETE FROM writer_reservations WHERE agent_id=?", (agent_id,))
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr
    child.wait(timeout=10)
    after = ready()
    assert after["pid"] != before["pid"]
    assert after["instance_id"] != before["instance_id"]
    assert after["build_identifier"] == candidate_metadata["build_identifier"]
    assert not any(alive(pid) for pid in terminal_pids), terminal_pids
    assert external.poll() is None
    snapshot = rpc("Snapshot")["Snapshot"]
    assert any(p["id"] == project_id for p in snapshot["projects"])
    assert any(a["id"] == agent_id and a["native_session_id"] == "saved-native-thread" for a in snapshot["agents"])
    with sqlite3.connect(data / "state/ditch.sqlite3") as db:
        saved = db.execute("SELECT message_json FROM agent_messages WHERE agent_id=?", (agent_id,)).fetchall()
    assert any(json.loads(row[0])["text"] == "Preserve this conversation" for row in saved)
    history = rpc({"ListAgentMessages": dict(agent_id=agent_id, before_sequence=None, limit=20)})
    assert any(m["message"]["text"] == "Preserve this conversation" for m in history["AgentMessages"]["messages"])
    assert (project / "KEEP-ME.txt").read_text() == "preserved"
    repeat = subprocess.run(command, env=env, capture_output=True, text=True, timeout=40)
    assert repeat.returncode == 0, repeat.stderr
    assert ready()["pid"] == after["pid"]
    # Current runtimes close registered terminals through atomic maintenance.
    marker = root / "modern.pid"
    open_terminal(project_id, f"trap '' HUP; echo $$ > {shlex.quote(str(marker))}; exec sleep 300")
    modern_pid = await_file(marker)
    token = str(uuid.uuid4())
    maintenance = dict(instance_id=after["instance_id"], token=token, action="begin")
    assert rpc({"RemoteRuntimeMaintenance": maintenance}) == "Accepted"
    assert not alive(modern_pid)
    assert external.poll() is None
    maintenance["action"] = "cancel"
    assert rpc({"RemoteRuntimeMaintenance": maintenance}) == "Accepted"
    print(json.dumps(dict(result="PASS", old_build=before["build_identifier"], new_build=after["build_identifier"],
                          reservation_protected=True, project_preserved=True, saved_conversation_preserved=True,
                          repeated_repair_keeps_pid=True, abandoned_pause_resumed=True,
                          ditch_terminals_closed=True, unrelated_terminal_preserved=True)))
finally:
    if external is not None:
        external.kill(); external.wait(timeout=5)
    if external_master is not None: os.close(external_master)
    try: rpc("Shutdown")
    except Exception: pass
    if child is not None:
        try: child.wait(timeout=5)
        except subprocess.TimeoutExpired: child.terminate(); child.wait(timeout=5)
    time.sleep(.3)
    log.close()
    shutil.rmtree(root)
