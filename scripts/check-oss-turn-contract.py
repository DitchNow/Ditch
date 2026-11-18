#!/usr/bin/env python3
"""Verify the actual Responses request for fresh/resumed Codex desktop turns.

Default: loopback scripted inference, fixture MCP, no desktop or external model.
--endpoint/--model: opt-in model comparison through a validating test-only relay.
The relay refuses requests exposing shell, filesystem or unrelated tools BEFORE
forwarding inference. Never uses the user's Codex home or OpenAI credentials.
--policy is exported by the production OssToolPolicy Rust regression test.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import sys
import threading
import time
import uuid
import base64
import hashlib
import struct
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.request import Request, build_opener, HTTPRedirectHandler
from urllib.error import HTTPError

TOOLS = {"desktop_status", "desktop_list_targets", "desktop_observe", "desktop_act"}


def fixture(policy_path, marker):
    schemas = json.loads(Path(policy_path).read_text())["tools"]
    for line in sys.stdin:
        req = json.loads(line)
        if "id" not in req:
            continue
        method = req.get("method")
        if method == "initialize":
            result = {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                      "serverInfo": {"name": "ditch-contract-fixture", "version": "1"}}
        elif method == "tools/list":
            result = {"tools": schemas}
        elif method == "tools/call":
            name = req["params"]["name"]
            data = ({"probe_marker": marker, "desktop_access": False} if name == "desktop_status"
                    else {"ok": False, "error": "fixtureOnly", "desktop_access": False})
            result = {"content": [{"type": "text", "text": json.dumps(data)}], "isError": False}
            if name == "desktop_observe" and os.environ.get("DITCH_CONTRACT_IMAGE"):
                result = {"content": [{"type": "text", "text": "Describe the image; no real desktop was accessed."},
                    {"type": "image", "mimeType": "image/png", "data": base64.b64encode(Path(os.environ["DITCH_CONTRACT_IMAGE"]).read_bytes()).decode()}], "isError": False}
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}), flush=True)


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        raise RuntimeError("Inference redirects are disabled")


def run(args):
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location("desktop_probe", Path(__file__).with_name("check-peekaboo-codex.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    policy = json.loads(args.policy.read_text())
    root = args.build_root.resolve() / ("turn-contract-" + uuid.uuid4().hex[:12])
    home = root / "home"
    home.mkdir(parents=True, mode=0o700)
    project = root / "fixture"
    project.mkdir()
    tmp = root / "tmp"
    tmp.mkdir()
    marker = "DESKTOP-" + uuid.uuid4().hex
    recall = "RECALL-" + uuid.uuid4().hex
    instructions = policy["thread_params"]["developerInstructions"]
    env = {k: v for k, v in os.environ.items() if not any(x in k.upper() for x in ["OPENAI", "CODEX", "PROXY"])}
    env.pop("DITCH_OLLAMA_SMOKE_TOKEN", None)
    env.update(CODEX_HOME=str(home), TMPDIR=str(tmp))
    image_data = None
    red_side = "LEFT" if uuid.uuid4().int % 2 else "RIGHT"
    if args.vision:
        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
        red, blue = bytes([255, 0, 0]), bytes([0, 0, 255])
        row = (red * 160 + blue * 160) if red_side == "LEFT" else (blue * 160 + red * 160)
        image_data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 320, 200, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress((b"\0" + row) * 200)) + chunk(b"IEND", b"")
        image_path = root / "vision.png"
        image_path.write_bytes(image_data)
        env["DITCH_CONTRACT_IMAGE"] = str(image_path)
    report = {"desktop_access": False, "live_inference": bool(args.endpoint), "cases": [], "wire": []}
    context = {"seed": None, "case": "initial", "error": None}
    opener = build_opener(NoRedirect())

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 8 * 1024 * 1024:
                    raise AssertionError("Invalid request size")
                body = self.rfile.read(length)
                payload = json.loads(body)
                if payload.get("model") != (args.model or "fixture"):
                    raise AssertionError("Unexpected inference model")
                flattened = []
                for tool in payload.get("tools", []):
                    if tool.get("type") == "namespace":
                        flattened.extend((tool.get("name", "") + "__" + child.get("name", ""), child.get("type"))
                                         for child in tool.get("tools", []))
                    else:
                        flattened.append((tool.get("name", ""), tool.get("type")))
                names = [name for name, _ in flattened]
                expected = {"mcp__ditch_tools__" + name for name in TOOLS}
                utilities = {"list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource",
                             "request_user_input", "get_goal", "create_goal", "update_goal"}
                # This assertion happens before ANY live inference request.
                if not expected.issubset(names) or set(names) - expected - utilities or any(kind != "function" for _, kind in flattened):
                    raise AssertionError("Unexpected model tools: " + ",".join(names))
                text = json.dumps(payload.get("input", []))
                # Compare text values, not JSON escaping or a model's echo.
                def contains(value, needle):
                    if isinstance(value, str):
                        return needle in value
                    if isinstance(value, dict):
                        return any(contains(v, needle) for v in value.values())
                    if isinstance(value, list):
                        return any(contains(v, needle) for v in value)
                    return False
                delivered = any(contains(i, instructions) for i in payload.get("input", [])
                                if isinstance(i, dict) and i.get("role") == "developer")
                row = {"case": context["case"], "tools": names, "instructions_delivered": delivered,
                       "recall_present": recall in text, "legacy_cua_present": "mcp__cua_repl.js" in text,
                       "model": payload.get("model")}
                def images(value):
                    if isinstance(value, dict):
                        for k, v in value.items():
                            if k == "image_url" and isinstance(v, str) and v.startswith("data:image/png;base64,"):
                                yield base64.b64decode(v.split(",", 1)[1])
                            else:
                                yield from images(v)
                    elif isinstance(value, list):
                        for v in value:
                            yield from images(v)
                row["image_delivered"] = image_data is not None and any(hashlib.sha256(i).digest() == hashlib.sha256(image_data).digest() for i in images(payload))
                report["wire"].append(row)
                if not delivered:
                    raise AssertionError("Current developer instructions absent at inference")
                if args.endpoint:
                    endpoint = args.endpoint.rstrip("/").removesuffix("/v1") + "/v1/responses"
                    headers = {"Content-Type": "application/json", "User-Agent": "Ditch-Contract-Probe/1"}
                    if os.environ.get("DITCH_OLLAMA_SMOKE_TOKEN"):
                        headers["Authorization"] = "Bearer " + os.environ["DITCH_OLLAMA_SMOKE_TOKEN"]
                    with opener.open(Request(endpoint, data=body, headers=headers), timeout=180) as response:
                        self.send_response(response.status)
                        self.send_header("Content-Type", response.headers.get("Content-Type", "text/event-stream"))
                        self.end_headers()
                        while chunk := response.read(4096):
                            self.wfile.write(chunk)
                            self.wfile.flush()
                    return
                message = ("Earlier workflow used mcp__cua_repl.js; Arc bridge was unavailable. " + recall
                           if context["seed"] == "legacy" else "Fixture response. " + recall)
                item = {"id": "msg_fixture", "type": "message", "role": "assistant",
                        "content": [{"type": "output_text", "text": message}]}
                if args.vision:
                    item = ({"id": "msg_fixture", "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": red_side}]} if row["image_delivered"] else
                        {"id": "fc_vision", "type": "function_call", "call_id": "call_vision", "name": "mcp__ditch_tools__desktop_observe", "arguments": json.dumps({"target_id":"fixture_window","mode":"visual"})})
                events = [
                    {"type": "response.created", "response": {"id": "resp_fixture", "status": "in_progress", "output": []}},
                    {"type": "response.output_item.done", "output_index": 0, "item": item},
                    {"type": "response.completed", "response": {"id": "resp_fixture", "status": "completed", "output": [item]}},
                ]
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                for event in events:
                    self.wfile.write(("data: " + json.dumps(event) + "\n\n").encode())
                self.wfile.flush()
            except Exception as error:
                # Do not serialize HTTP exception bodies, credentials or real prompts.
                context["error"] = (str(error) if isinstance(error, AssertionError) else
                                    f"HTTP {error.code}" if isinstance(error, HTTPError) else type(error).__name__)
                self.send_error(400, "Contract check failed")

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    config = {
        "model_provider": "ditch_contract", "model": args.model or "fixture",
        "model_providers.ditch_contract": {"name": "Ditch contract fixture", "base_url": f"http://127.0.0.1:{server.server_port}/v1",
            "wire_api": "responses", "requires_openai_auth": False, "supports_websockets": False,
            "request_max_retries": 0, "stream_max_retries": 0},
        "features.shell_tool": False, "features.apply_patch_freeform": False,
        "features.multi_agent": False, "features.collab": False, "features.apps": False,
        "web_search": "disabled", "model_context_window": 65536, "model_auto_compact_token_limit": 49152,
    }
    command = [args.codex]
    for key, value in config.items():
        if isinstance(value, dict):
            for field, setting in value.items():
                command += ["-c", key + "." + field + "=" + json.dumps(setting)]
        else:
            command += ["-c", key + "=" + json.dumps(value)]
    command += ["app-server"]

    def turn(case, thread=None, seed=None):
        started = time.monotonic()
        context.update(case=case, seed=seed, error=None)
        before = len(report["wire"])
        peer = module.Peer(command, env, project, root / "codex.stderr")
        try:
            peer.request("initialize", {"clientInfo": {"name": "ditch_contract", "version": "1"},
                                        "capabilities": {"experimentalApi": True}})
            peer.send({"method": "initialized", "params": {}})
            params = json.loads(json.dumps(policy["thread_params"]))
            params.update(cwd=str(project), model=args.model or "fixture", modelProvider="ditch_contract",
                          approvalPolicy="never", sandbox="danger-full-access")
            params.setdefault("config", {})["mcp_servers"] = {"ditch_tools": {
                "command": sys.executable, "args": [str(Path(__file__).resolve()), "--mcp-fixture", str(args.policy.resolve()), marker],
                "enabled": True, "enabled_tools": sorted(TOOLS)}}
            if args.vision:
                params["config"]["mcp_servers"]["ditch_tools"]["env"] = {"DITCH_CONTRACT_IMAGE": str(image_path)}
            # These flags must come from production apply_desktop_verification,
            # not a separate test-only configuration that could conceal drift.
            if params["config"].get("features", {}).get("shell_tool") is not False:
                raise AssertionError("Export policy with production desktop verification restrictions")
            if thread:
                params.update(threadId=thread, excludeTurns=True)
            result = peer.request("thread/resume" if thread else "thread/start", params)
            thread_id = result["thread"]["id"]
            if thread and thread_id != thread:
                raise AssertionError("Resume replaced thread")
            inventory = peer.request("mcpServerStatus/list", {"threadId": thread_id, "serverName": "ditch_tools", "detail": "toolsAndAuthOnly"})
            if not all(name in json.dumps(inventory) for name in TOOLS):
                raise AssertionError("Incomplete desktop catalogue")
            prompt = (f"Remember {recall}. This is fixture history. " +
                      ("The old workflow used mcp__cua_repl.js and said an Arc bridge was missing." if seed == "legacy" else "The workflow uses desktop_status.") +
                      " Reply OK without tools." if seed else
                      "Use the current desktop_status tool and report its exact probe_marker. " +
                      ("Repeat the RECALL marker from our earlier conversation. " if thread else "") +
                      "Do not use shell, CUA or another browser. State only what tools establish.")
            if args.vision:
                prompt = "This is an isolated image transport fixture, not a real desktop. Call desktop_observe with target_id=fixture_window, mode=visual. Inspect its returned image. Which half is red? Reply only LEFT or RIGHT. Do not infer the answer from text."
            peer.events.clear()
            peer.request("turn/start", {"threadId": thread_id, "input": [{"type": "text", "text": prompt}]})
            texts, calls = [], []
            deadline = time.monotonic() + 180
            while True:
                event = peer.events.pop(0) if peer.events else peer.receive(deadline)
                if event.get("method") == "item/completed":
                    item = event.get("params", {}).get("item", {})
                    if item.get("type") == "agentMessage":
                        texts.append(item.get("text", ""))
                    if item.get("type") == "mcpToolCall":
                        calls.append(item.get("tool"))
                    if item.get("type") in ["commandExecution", "fileChange"]:
                        raise AssertionError("Unexpected shell or file action")
                if "id" in event and event.get("method"):
                    peer.send({"id": event["id"], "error": {"code": -32601, "message": "Fixture declines requests"}})
                if event.get("method") == "turn/completed":
                    if context["error"]:
                        raise AssertionError(context["error"])
                    if event["params"]["turn"]["status"] != "completed":
                        raise AssertionError("Turn did not complete")
                    break
            wire = report["wire"][before:]
            if not wire:
                raise AssertionError("No inference request observed")
            if thread and not all(w["recall_present"] for w in wire):
                raise AssertionError("Resume lost history at inference")
            if case == "resumed_legacy" and not all(w["legacy_cua_present"] for w in wire):
                raise AssertionError("Legacy-history test did not exercise stale CUA references")
            output = "\n".join(texts)
            behavior = None if not args.endpoint or seed else ("desktop_status" in calls and marker in output and (not thread or recall in output))
            if args.vision:
                behavior = any(w["image_delivered"] for w in wire) and output.strip().upper() == red_side and "desktop_observe" in calls
            report["cases"].append({"case": case, "wire_verified": True, "tool_calls": calls,
                                   "marker_reported": marker in output,
                                   "recall_reported": recall in output,
                                   "inference_requests": len(wire),
                                   "elapsed_seconds": round(time.monotonic() - started, 3),
                                   "behavior_passed": behavior})
            return thread_id
        finally:
            peer.close()

    try:
        if args.vision:
            turn("vision")
        else:
            turn("fresh")
            normal = turn("seed_current", seed="current")
            turn("resumed_current", normal)
            legacy = turn("seed_legacy", seed="legacy")
            turn("resumed_legacy", legacy)
        report["passed"] = all(c["behavior_passed"] is not False for c in report["cases"])
    except Exception as error:
        report.update(passed=False, error=context["error"] or type(error).__name__)
    finally:
        server.shutdown()
        server.server_close()
        (root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        print("Evidence:", root)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--mcp-fixture":
        fixture(sys.argv[2], sys.argv[3])
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument("--policy", required=True, type=Path)
        parser.add_argument("--build-root", required=True, type=Path)
        parser.add_argument("--codex", required=True)
        parser.add_argument("--endpoint")
        parser.add_argument("--model")
        parser.add_argument("--vision", action="store_true", help="Verify an MCP screenshot reaches inference and the model identifies its contents")
        args = parser.parse_args()
        if bool(args.endpoint) != bool(args.model):
            parser.error("--endpoint and --model must be supplied together")
        raise SystemExit(run(args))
