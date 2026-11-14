#!/usr/bin/env python3
"""Phase 1 fixture: real SDK/Codex MCP discovery, optional Ollama resume/vision.

No desktop observation/input and no installed Ditch or user Codex configuration
is touched. All evidence goes to a fresh directory below --build-root. The Rust
fixture is deliberately separate from the production CLI and must not ship.
Use --live --endpoint URL --model TAG to perform inference. Optional endpoint
credential: DITCH_OLLAMA_SMOKE_TOKEN (never written to reports).
"""
import argparse
import base64
import json
import os
from pathlib import Path
import queue
import secrets
import signal
import struct
import subprocess
import threading
import time
import uuid
import zlib


def check_adapter_startup(args):
    """Real Codex + production CLI, controlled runtime transport, no native calls.

    Rust tests exercise the production authorization helper. This separate probe
    checks Codex's process lifecycle against the same owner/discovery contract.
    """
    import hmac
    import socket
    import sys
    if sys.platform != "darwin":
        raise RuntimeError("Adapter PID probe requires macOS LOCAL_PEERPID")
    base = args.build_root.resolve()
    base.mkdir(parents=True, exist_ok=True)
    root = base / ("adapter-startup-" + uuid.uuid4().hex[:12])
    home = root / "home"
    home.mkdir(parents=True, mode=0o700)
    # No external inference: starting/interruption of an offline turn only makes
    # Codex persist the thread so the next process can genuinely resume it.
    (home / "config.toml").write_text('''model_provider = "offline_fixture"
model = "fixture"
[model_providers.offline_fixture]
name = "Offline fixture"
base_url = "http://127.0.0.1:1/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
''')
    cli = args.server.resolve(strict=True)
    expected = {"desktop_status", "desktop_list_targets", "desktop_observe", "desktop_act"}
    report = {"desktop_access": False, "external_inference": False, "cases": []}
    env = dict(os.environ, CODEX_HOME=str(home))
    for key in ("OPENAI_API_KEY", "CODEX_API_KEY", "DITCH_SSH_ASKPASS_SOCKET"):
        env.pop(key, None)

    def run(thread_id, strict):
        token = secrets.token_hex(32)
        token_path = root / (uuid.uuid4().hex + ".token")
        token_path.touch(mode=0o600)
        token_path.write_text(token)
        path = base / ("s-" + uuid.uuid4().hex[:12])
        if len(os.fsencode(path)) >= 104:
            raise RuntimeError("Use a shorter --build-root for the Unix socket")
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(path))
        path.chmod(0o600)
        listener.listen()
        listener.settimeout(0.1)
        done = threading.Event()
        state = {"owner": None, "requests": [], "revoked": False}

        def serve():
            while not done.is_set():
                try:
                    conn, _ = listener.accept()
                except socket.timeout:
                    continue
                with conn:
                    conn.settimeout(3)
                    pid = conn.getsockopt(0, 0x002)
                    try:
                        request = json.loads(conn.makefile().readline())
                        tool = request["tool"]
                        if not hmac.compare_digest(request["credential"], token):
                            result = {"ok": False, "error": "invalidCredential"}
                        elif state["revoked"]:
                            result = {"ok": False, "error": "revoked"}
                        elif tool == "_ditch_adapter_ready":
                            if state["owner"] is None:
                                state["owner"] = pid
                            result = ({"ok": False, "error": "adapterChanged"}
                                      if strict and pid != state["owner"] else {"ok": True})
                        elif pid != state["owner"]:
                            result = {"ok": False, "error": "adapterChanged"}
                        else:
                            # Fixture responses only. Never forward to Ditch/macOS.
                            result = {"ok": True, "result": {"fixture": True}}
                        state["requests"].append({"pid": pid, "tool": tool, "accepted": result["ok"]})
                        conn.sendall((json.dumps(result) + "\n").encode())
                    except (OSError, ValueError, KeyError):
                        pass
        worker = threading.Thread(target=serve)
        worker.start()
        peer = None
        secondary = None
        try:
            peer = Peer([args.codex, "app-server"], env, root, root / "codex.stderr")
            peer.request("initialize", {"clientInfo": {"name": "the_ditch", "version": "probe"}, "capabilities": {"experimentalApi": True}})
            peer.send({"method": "initialized", "params": {}})
            adapter_env = {"DITCH_TOOL_SOCKET": str(path), "DITCH_TOOL_CREDENTIAL_FILE": str(token_path)}
            params = {"cwd": str(root), "approvalPolicy": "never", "sandbox": "danger-full-access", "config": {"features": {"apps": False}, "mcp_servers": {"ditch_tools": {"command": str(cli), "args": ["tools", "serve", "--stdio"], "env": adapter_env, "enabled_tools": sorted(expected), "startup_timeout_sec": 5}}}}
            if thread_id:
                params["threadId"] = thread_id
                params["excludeTurns"] = True
            result = peer.request("thread/resume" if thread_id else "thread/start", params)
            native_id = result["thread"]["id"]
            if thread_id:
                assert native_id == thread_id, "resume replaced the conversation"
            primary = peer.request("mcpServerStatus/list", {"threadId": native_id, "serverName": "ditch_tools", "detail": "toolsAndAuthOnly"})["data"]
            assert len(primary) == 1 and set(primary[0]["tools"]) == expected, primary
            owner = state["owner"]
            assert owner is not None
            broad = peer.request("mcpServerStatus/list", {"threadId": native_id})["data"]
            desktop = next(s for s in broad if s["name"] == "ditch_tools")
            if strict:
                assert desktop.get("toolsError") and not desktop["tools"], "old PID rule did not reproduce the failure"
            else:
                assert desktop["runtimeStatus"] == "connected" and not desktop.get("toolsError") and set(desktop["tools"]) == expected, desktop
            assert state["owner"] == owner, "inventory changed execution ownership"
            assert any(r["pid"] != owner for r in state["requests"]), "Codex did not create the expected discovery helper"
            if not strict:
                # A separate real CLI can list schemas but cannot invoke even
                # desktop_status, and cannot transfer authority to itself.
                secondary = Peer([str(cli), "tools", "serve", "--stdio"], dict(env, **adapter_env), root, root / "adapter.stderr")
                secondary.request("initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "probe", "version": "1"}})
                secondary.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
                assert {t["name"] for t in secondary.request("tools/list", {})["tools"]} == expected
                rejected = secondary.request("tools/call", {"name": "desktop_status", "arguments": {}})
                assert rejected["isError"] and "adapterChanged" in json.dumps(rejected), rejected
                assert state["owner"] == owner
                state["revoked"] = True
                rejected = secondary.request("tools/call", {"name": "desktop_status", "arguments": {}})
                assert rejected["isError"] and "revoked" in json.dumps(rejected), rejected
            turn = peer.request("turn/start", {"threadId": native_id, "input": [{"type": "text", "text": "Offline adapter lifecycle fixture"}]})
            try:
                peer.request("turn/interrupt", {"threadId": native_id, "turnId": turn["turn"]["id"]})
            except RuntimeError:
                pass  # Offline request may already have failed.
            for _ in range(40):
                try:
                    peer.request("thread/read", {"threadId": native_id, "includeTurns": False})
                    break
                except RuntimeError:
                    time.sleep(0.05)
            else:
                raise AssertionError("fixture rollout was not persisted")
            report["cases"].append({"resume": bool(thread_id), "old_guard": strict, "owner_unchanged": True, "second_helper_observed": True, "catalogue_verified": not strict, "regression_reproduced": strict})
            return native_id
        finally:
            if secondary:
                secondary.close()
            if peer:
                peer.close()
            done.set()
            worker.join(timeout=5)
            listener.close()
            path.unlink(missing_ok=True)
            token_path.unlink(missing_ok=True)
    try:
        thread = run(None, True)
        run(thread, True)
        thread = run(None, False)
        run(thread, False)
    finally:
        (root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        print("Evidence:", root)


class Peer:
    def __init__(self, command, env, cwd, log):
        self.log = log.open("a")
        self.child = subprocess.Popen(command, env=env, cwd=cwd, stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=self.log, text=True,
                                      start_new_session=True)
        self.messages = queue.Queue()
        self.next_id = 0
        self.events = []
        def reader():
            try:
                for line in self.child.stdout:
                    self.messages.put(json.loads(line))
            except Exception as error:
                self.messages.put(error)
            finally:
                self.messages.put(None)
        threading.Thread(target=reader, daemon=True).start()

    def send(self, message):
        self.child.stdin.write(json.dumps(message) + "\n")
        self.child.stdin.flush()

    def receive(self, deadline):
        message = self.messages.get(timeout=max(0.001, deadline - time.monotonic()))
        if message is None:
            raise RuntimeError("Child exited; inspect the isolated log")
        if isinstance(message, Exception):
            raise message
        return message

    def request(self, method, params, timeout=30):
        self.next_id += 1
        request_id = self.next_id
        self.send(dict(id=request_id, method=method, params=params, jsonrpc="2.0"))
        deadline = time.monotonic() + timeout
        while True:
            message = self.receive(deadline)
            if message.get("id") == request_id:
                if "error" in message:
                    raise RuntimeError(f"{method}: {message['error']}")
                return message["result"]
            self.events.append(message)

    def close(self):
        self.child.stdin.close()
        try:
            self.child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(self.child.pid, signal.SIGTERM)
            try:
                self.child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(self.child.pid, signal.SIGKILL)
                self.child.wait()
        self.child.stdout.close()
        self.log.close()


def fixture_image():
    """Synthetic PNG, with random colors known only to the test assertion."""
    palette = {"red": (255, 0, 0), "green": (0, 190, 0), "blue": (0, 0, 255),
               "yellow": (255, 255, 0)}
    colors = list(palette)
    secrets.SystemRandom().shuffle(colors)
    raw = bytearray()
    for y in range(256):
        raw.append(0)
        for x in range(256):
            raw.extend(palette[colors[(y // 128) * 2 + x // 128]])
    def chunk(kind, payload):
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload)))
    png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 256, 256, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))
    return base64.b64encode(png).decode(), colors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", required=True, type=Path)
    parser.add_argument("--codex", default="codex")
    parser.add_argument("--build-root", required=True, type=Path)
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--endpoint")
    parser.add_argument("--model")
    parser.add_argument("--vision", action="store_true")
    parser.add_argument("--adapter-startup", action="store_true", help="Use --server as the production ditch_cli; test owner/discovery lifecycle without desktop access")
    args = parser.parse_args()
    if args.adapter_startup:
        if args.live or args.vision:
            parser.error("--adapter-startup never performs inference or desktop actions")
        return check_adapter_startup(args)
    if args.live and not (args.endpoint and args.model):
        parser.error("--live requires --endpoint and --model")
    if args.vision and not args.live:
        parser.error("--vision requires --live")
    root = args.build_root.resolve() / ("codex-probe-" + uuid.uuid4().hex)
    project, home = root / "fixture", root / "codex-home"
    project.mkdir(parents=True)
    home.mkdir(mode=0o700)
    (root / "tmp").mkdir()
    server = args.server.resolve(strict=True)
    marker = "MCP-" + uuid.uuid4().hex
    recall = "RECALL-" + uuid.uuid4().hex
    image, colors = fixture_image()
    env = dict(os.environ, CODEX_HOME=str(home), TMPDIR=str(root / "tmp"))
    for key in ("OPENAI_API_KEY", "CODEX_API_KEY"):
        env.pop(key, None)
    config = {"web_search": "disabled", "features.apps": False,
              "features.multi_agent": False, "skills.max_context_tokens": 1024}
    if args.live:
        endpoint = args.endpoint.rstrip("/").removesuffix("/v1")
        config.update({"model_provider": "ditch_probe_ollama", "model": args.model,
                       "model_context_window": 65536, "model_auto_compact_token_limit": 49152})
        for key, value in dict(name="Ditch isolated probe", base_url=endpoint + "/v1",
                               wire_api="responses", requires_openai_auth=False,
                               supports_websockets=False).items():
            config["model_providers.ditch_probe_ollama." + key] = value
        if env.get("DITCH_OLLAMA_SMOKE_TOKEN"):
            config["model_providers.ditch_probe_ollama.env_key"] = "DITCH_OLLAMA_SMOKE_TOKEN"
    command = [args.codex]
    for key, value in config.items():
        command.extend(["-c", key + "=" + json.dumps(value)])
    command.append("app-server")
    adapter_env = {"DITCH_PROBE_MARKER": marker}
    if args.vision:
        adapter_env["DITCH_PROBE_IMAGE"] = image
    adapter = dict(command=str(server), env=adapter_env, enabled_tools=["desktop_status"],
                   startup_timeout_sec=5, tool_timeout_sec=20)
    if args.vision:
        adapter["enabled_tools"].append("desktop_observe")
    report = {"sdk": "rmcp 3.10.0", "desktop_access": False,
              "sdk_stdio_verified": False, "new_thread_discovery_verified": False,
              "resume_discovery_verified": False, "recall_verified": False,
              "tool_call_verified": False, "vision_verified": False}

    def run(thread_id=None, inject=True):
        peer = Peer(command, env, project, root / "codex.stderr")
        try:
            peer.request("initialize", {"clientInfo": {"name": "ditch_desktop_probe", "version": "0.1"},
                                        "capabilities": {"experimentalApi": True}})
            peer.send({"method": "initialized", "params": {}})
            params = dict(cwd=str(project), approvalPolicy="never", sandbox="danger-full-access",
                          config={"mcp_servers": {"ditch_tools": adapter} if inject else {}})
            if args.live:
                params.update(model=args.model, modelProvider="ditch_probe_ollama")
            if thread_id:
                params["threadId"] = thread_id
            result = peer.request("thread/resume" if thread_id else "thread/start", params)
            native_id = result["thread"]["id"]
            if thread_id and native_id != thread_id:
                raise AssertionError("Codex replaced the existing thread")
            if inject:
                status = peer.request("mcpServerStatus/list", {"threadId": native_id,
                                      "serverName": "ditch_tools", "detail": "toolsAndAuthOnly"})
                if "desktop_status" not in json.dumps(status):
                    raise AssertionError("Thread catalog is missing desktop_status")
                report["resume_discovery_verified" if thread_id else "new_thread_discovery_verified"] = True
            if not args.live:
                return native_id
            prompt = (f"Remember {recall} in conversation only. Do not write it to files or use tools. Reply OK."
                      if not thread_id else
                      "Call the desktop_status MCP tool and report its exact probe_marker. Also repeat the "
                      "RECALL marker I gave you earlier using conversation history. Use no shell or file tools.")
            if thread_id and args.vision:
                prompt += (" Also call desktop_observe. Report the image's four panel colors in order: "
                           "top-left, top-right, bottom-left, bottom-right. Use exactly COLOR_ORDER=a,b,c,d.")
            peer.events.clear()
            peer.request("turn/start", {"threadId": native_id, "input": [{"type": "text", "text": prompt}]})
            deadline = time.monotonic() + 180
            texts, completed_tools = [], []
            while True:
                event = peer.events.pop(0) if peer.events else peer.receive(deadline)
                method = event.get("method", "")
                if method == "item/completed":
                    item = event.get("params", {}).get("item", {})
                    if item.get("type") == "agentMessage":
                        texts.append(item.get("text", ""))
                    elif item.get("type") == "mcpToolCall":
                        completed_tools.append(item)
                    elif item.get("type") in ("commandExecution", "fileChange"):
                        raise AssertionError("Model used shell/files; memory/vision test is invalid")
                if "id" in event and method:
                    peer.send({"id": event["id"], "error": {"code": -32601, "message": "Probe declines requests"}})
                if method == "turn/completed":
                    if event["params"]["turn"]["status"] != "completed":
                        raise AssertionError("Model turn did not complete")
                    break
            if thread_id:
                response = "\n".join(texts)
                if recall not in response:
                    raise AssertionError("Recall marker missing after restart/resume")
                report["recall_verified"] = True
                if marker not in response or not any(t.get("tool") == "desktop_status" for t in completed_tools):
                    raise AssertionError("Missing real MCP call/marker")
                report["tool_call_verified"] = True
                if args.vision:
                    expected = "color_order=" + ",".join(colors)
                    if expected not in response.lower().replace(" ", ""):
                        raise AssertionError("Image interpretation incorrect")
                    report["vision_verified"] = True
            return native_id
        finally:
            peer.close()

    try:
        sdk_env = dict(env, **adapter_env)
        sdk_env.pop("DITCH_OLLAMA_SMOKE_TOKEN", None)
        peer = Peer([str(server)], sdk_env, project, root / "sdk.stderr")
        try:
            peer.request("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                        "clientInfo": {"name": "ditch_probe", "version": "0.1"}})
            peer.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            peer.request("tools/list", {})
            result = peer.request("tools/call", {"name": "desktop_status", "arguments": {}})
            if marker not in json.dumps(result):
                raise AssertionError("SDK round-trip failed")
            report["sdk_stdio_verified"] = True
        finally:
            peer.close()
        run()  # New-thread adapter discovery, independent of the resume test.
        if args.live:
            native_id = run(inject=False)  # Existing conversation predates the adapter.
            run(native_id, inject=True)
        report["status"] = "passed"
    except Exception as error:
        report.update(status="failed", error=str(error))
        raise
    finally:
        token = env.get("DITCH_OLLAMA_SMOKE_TOKEN")
        for log in root.glob("*.stderr"):
            if token:
                log.write_text(log.read_text().replace(token, "[redacted]"))
        if token:
            report = json.loads(json.dumps(report).replace(token, "[redacted]"))
        (root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        print("Evidence:", root)


if __name__ == "__main__":
    main()
