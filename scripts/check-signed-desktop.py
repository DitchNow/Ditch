#!/usr/bin/env python3
"""Opt-in native verification through the signed Staging runtime.

Creates one diagnostic agent in the requested existing project and opens one Arc
tab to example.com (Arc), or creates one uniquely titled Backlog task through
Ditch Staging's UI (DitchStaging). Never uses GitHub or edits files through the agent.
Requires desktop_verification_v1 and desktop_automation_v2: old builds are rejected.
Reads receipts/transcript locally; reports booleans/IDs, not screen contents.
"""
import argparse
import datetime
import json
from pathlib import Path
import socket
import sqlite3
import time
import uuid


def rpc(path, body):
    envelope = {"protocol_version": 1, "id": str(uuid.uuid4()),
                "sent_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "body": body}
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(90)
        stream.connect(str(path))
        stream.sendall((json.dumps(envelope) + "\n").encode())
        with stream.makefile("rb") as reader:
            response = json.loads(reader.readline(8 * 1024 * 1024))
    return response["body"]


def tool_results(value):
    if isinstance(value, dict):
        if "snapshot_id" in value or "error" in value or "outcome" in value:
            yield value
        for key in ["result", "content", "text", "output"]:
            if key in value:
                yield from tool_results(value[key])
    elif isinstance(value, list):
        for item in value:
            yield from tool_results(item)
    elif isinstance(value, str):
        try:
            yield from tool_results(json.loads(value))
        except (ValueError, RecursionError):
            pass


def verified_backlog_task(db, project_id, marker):
    # Read-only independent verification: assistant prose cannot satisfy this.
    rows = db.execute("SELECT project_id,state,task_json FROM tasks WHERE title=?", (marker,)).fetchall()
    if len(rows) != 1:
        return False
    project, state, serialized = rows[0]
    task = json.loads(serialized or "{}")
    return (project == project_id and state.strip('"') == "Backlog"
            and task.get("state") == "Backlog" and task.get("github_source") is None)


def main(args):
    support = args.support.resolve()
    status = rpc(support / "ditchd.sock", "RuntimeStatus").get("RuntimeStatus", {})
    if status.get("deployment_environment") != "staging":
        raise RuntimeError("This probe only runs against Staging")
    if args.expected_build and status.get("build_identifier") != args.expected_build:
        raise RuntimeError("Running Staging does not match --expected-build; rebuild and launch it first")
    if "desktop_verification_v1" not in status.get("capabilities", []):
        raise RuntimeError("Rebuild signed Staging: desktop_verification_v1 is required")
    if "desktop_automation_v2" not in status.get("capabilities", []):
        raise RuntimeError("Rebuild signed Staging: desktop_automation_v2 is required")
    db = sqlite3.connect((support / "ditch.sqlite3").as_uri() + "?mode=ro", uri=True)
    db.execute("PRAGMA query_only=ON")
    project_row = db.execute("SELECT name,root,execution_target FROM projects WHERE id=? AND archived_at IS NULL", (args.project_id,)).fetchone()
    if not project_row:
        raise RuntimeError("Project not found")
    project = {"name": project_row[0], "root": project_row[1]}
    if json.loads(project_row[2]).get("kind") != "local":
        raise RuntimeError("A local project is required for native desktop verification")
    endpoint_row = db.execute("SELECT endpoint_json FROM ollama_endpoints WHERE id=?", (args.endpoint_id,)).fetchone()
    if not endpoint_row or not json.loads(endpoint_row[0]).get("enabled"):
        raise RuntimeError("Enabled custom inference endpoint required")
    marker = "ditch-probe-" + uuid.uuid4().hex
    prompt = ("Diagnostic Arc test. Use only the provided desktop tools. List Arc and its exact windows, "
              "using current-turn target_id references, focus a window, and observe in mode=visual. "
              "Perform one harmless move action to a visible point in the image to exercise the real pointer. "
              "Observe again. Open one NEW tab and navigate to "
              f"https://example.com/?{marker} using native keyboard/mouse actions. "
              "Observe freshly after every action in mode=visual; snapshots are single-use. Finally observe in "
              "mode=accessibility with include_image=false and check the address bar for the exact URL. "
              "Do not use shell, CUA, another app, or edit files. "
              "Stop and report the exact error if a tool fails. Do not claim success without verification.")
    if args.target == "DitchStaging":
        prompt = ("Diagnostic Ditch Staging test. Use only the provided desktop tools. "
                  "Discover Ditch Staging (ai.theditch.app.staging), list its windows using the returned target_id, "
                  "focus its main window and observe in mode=visual. Perform a harmless foreground click "
                  "on the requested project's navigation and observe the changed UI. Then use the native Kanban UI "
                  f"for project {project['name']!r} to create exactly ONE task titled {marker!r} in Backlog only. "
                  "Leave GitHub integration off; do not start task execution. Use foreground mouse/keyboard actions "
                  "with fresh visual snapshots after each action. Finally observe the Backlog to verify the new task. "
                  "Do not use shell, CUA, another app, a web interface, or edit files. Never substitute production Ditch. "
                  "Current tool results override stale claims that Ditch blocks automation. Rediscover once after a "
                  "stale target; otherwise stop and report the actual error. Do not claim success without verification.")
    params = {
        "project_id": args.project_id, "project_name": project["name"], "project_root": project["root"],
        "prompt": prompt, "mode": "Exec",
        "execution_profile": {"inference_endpoint_id": args.endpoint_id, "model": args.model,
                              "transport": "AppServer", "approval": "FullAccess", "skills": []},
        "turn_options": {"computer_use_visualizer": True, "desktop_verification_target": args.target}}
    transcript_offsets = {}
    method = "StartCodexSession"
    if args.resume_thread:
        previous = db.execute("SELECT project_id,run_json,codex_home,state FROM agents WHERE native_session_id=? ORDER BY updated_at DESC LIMIT 1",
                              (args.resume_thread,)).fetchone()
        profile = json.loads(previous[1]).get("execution_profile", {}) if previous else {}
        if (not previous or previous[0] != args.project_id
                or profile.get("inference_endpoint_id") != args.endpoint_id or profile.get("model") != args.model
                or previous[3].strip('"') not in ["Completed", "Failed", "Interrupted", "Idle"]):
            raise RuntimeError("Resume requires an inactive thread from this project and the exact selected OSS endpoint/model")
        prior_paths = list(Path(previous[2] or status["codex_home"]).joinpath("sessions").rglob("*" + args.resume_thread + ".jsonl"))
        if len(prior_paths) != 1:
            raise RuntimeError("Cannot identify prior transcript; refusing to mix old evidence")
        transcript_offsets[str(prior_paths[0])] = prior_paths[0].stat().st_size
        method = "ResumeCodexSession"
        params.pop("mode")
        params["thread_id"] = args.resume_thread
    receipt_floor = db.execute("SELECT COALESCE(MAX(rowid),0) FROM tool_operation_receipts").fetchone()[0]
    response = rpc(support / "ditchd.sock", {method: params})
    agent = response.get("AgentStarted", {}).get("id")
    if not agent:
        raise RuntimeError("Staging refused diagnostic launch; inspect its setup diagnostic")
    print("Diagnostic agent:", agent, flush=True)
    report = {"agent_id": agent, "runtime_build": status.get("build_identifier"), "target": args.target,
              "resumed": bool(args.resume_thread), "passed": False}
    finished = False
    try:
        deadline = time.monotonic() + args.timeout
        row = None
        while time.monotonic() < deadline:
            row = db.execute("SELECT state,native_session_id,codex_home FROM agents WHERE id=?", (agent,)).fetchone()
            if row and row[0].strip('"') in ["Completed", "Failed", "Interrupted"]:
                finished = True
                break
            time.sleep(1)
        else:
            rpc(support / "ditchd.sock", {"StopAgent": {"agent_id": agent}})
            raise RuntimeError("Timed out; stop requested")
        receipts = db.execute("SELECT tool,state FROM tool_operation_receipts WHERE agent_id=? AND rowid>? ORDER BY updated_at, rowid", (agent, receipt_floor)).fetchall()
        report["native_receipts"] = [{"tool": t, "state": s} for t, s in receipts]
        report["thread_id"] = row[1]
        paths = list(Path(row[2] or status["codex_home"]).joinpath("sessions").rglob("*" + row[1] + ".jsonl")) if row[1] else []
        if len(paths) != 1:
            raise RuntimeError("Cannot identify diagnostic transcript")
        calls, snapshots = {}, set()
        action_accepted = False
        click_accepted = False
        observed_after_click = False
        foreground_action = False
        visual_observed = False
        verified = False
        unexpected = []
        with paths[0].open("rb") as transcript:
            transcript.seek(transcript_offsets.get(str(paths[0]), 0))
            lines = transcript.readlines()
        for line in lines:
            event = json.loads(line).get("payload", {})
            if event.get("type") == "function_call":
                name = event.get("name", "").split("__")[-1]
                arguments = event.get("arguments", {})
                if isinstance(arguments, str):
                    arguments = json.loads(arguments)
                calls[event.get("call_id")] = (name, arguments)
                if name not in ["desktop_status", "desktop_list_targets", "desktop_observe", "desktop_act"]:
                    unexpected.append(name)
            if event.get("type") == "function_call_output":
                name, arguments = calls.get(event.get("call_id"), (None, {}))
                for result in tool_results(event.get("output")):
                    if name == "desktop_act" and result.get("verification_required") is True and "outcome" in result:
                        action_accepted = True
                        foreground_action |= result.get("delivery") == "foreground"
                        click_accepted |= (result.get("delivery") == "foreground"
                                           and arguments.get("action") in ["click", "double_click", "right_click"])
                    if name == "desktop_observe" and "snapshot_id" in result:
                        snapshots.add(result["snapshot_id"])
                        visual_observed |= result.get("mode") == "visual"
                        observed_after_click |= click_accepted and result.get("mode") == "visual"
                        if action_accepted and marker in json.dumps(result.get("elements", [])):
                            verified = True
        if args.target == "DitchStaging":
            verified = verified_backlog_task(db, args.project_id, marker)
        report.update(fresh_snapshots=len(snapshots), outcome_verified=verified, unexpected_tools=unexpected,
                      foreground_click=click_accepted, visual_observation_after_click=observed_after_click,
                      foreground_action=foreground_action, visual_observed=visual_observed)
        report["passed"] = (row[0].strip('"') == "Completed" and verified and foreground_action and visual_observed and len(snapshots) >= 2
                            and any(t == "desktop_act" and s == "completed" for t, s in receipts)
                            and not unexpected and all(s == "completed" for _, s in receipts)
                            and (not args.resume_thread or row[1] == args.resume_thread)
                            and (args.target != "DitchStaging" or (click_accepted and observed_after_click)))
    finally:
        if not finished:
            try:
                rpc(support / "ditchd.sock", {"StopAgent": {"agent_id": agent}})
            except (OSError, ValueError):
                report["stop_request_failed"] = True
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
        db.close()
        print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--support", type=Path, required=True)
    parser.add_argument("--project-id", required=True)
    parser.add_argument("--endpoint-id", required=True)
    parser.add_argument("--model", default="qwen3.5:9b")
    parser.add_argument("--target", choices=["Arc", "DitchStaging"], default="Arc")
    parser.add_argument("--resume-thread", help="Repeat against an inactive existing thread with the same project/OSS profile")
    parser.add_argument("--expected-build", help="Refuse a runtime whose build identifier differs from this value")
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--timeout", type=int, default=180)
    raise SystemExit(main(parser.parse_args()))
