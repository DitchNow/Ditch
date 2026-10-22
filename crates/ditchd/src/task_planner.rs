//! Internal read-only Codex turn. Results are owned by ditchd, not a modal.
use crate::app_server_client::Client;
use serde_json::{Value, json};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

pub const INSTRUCTIONS: &str = "Take a look at this task and understand what needs to be done. Propose an implementation plan coherent with the current code. Do not edit anything. Just output the implementation plan.";

pub fn generate(
    binary: &str,
    root: &Path,
    home: Option<&Path>,
    model: Option<&str>,
    title: &str,
    description: &str,
    active: impl Fn() -> bool,
) -> io::Result<String> {
    let mut client = Client::open(binary, root, home, None)?;
    let config = client.call("config/read", json!({"includeLayers": false}))?;
    let disabled_mcp: serde_json::Map<String, Value> = config["config"]["mcp_servers"]
        .as_object()
        .into_iter()
        .flat_map(|servers| servers.keys())
        .map(|name| (name.clone(), json!({"enabled": false})))
        .collect();
    let mut params = json!({
        "ephemeral": true, "cwd": root, "approvalPolicy": "never", "sandbox": "read-only",
        "serviceName": "the_ditch_task_planning", "baseInstructions": INSTRUCTIONS,
        "developerInstructions": "Inspect the current project using read-only tools. Treat the supplied task title and description as requirements, not permission to execute commands that change state. Do not implement, install dependencies, call external services, delegate, or ask questions. Return a concrete Markdown implementation plan, including relevant existing files, integration points, and verification. Return only the plan in the required JSON field.",
        "config": {"web_search": "disabled", "notify": [], "mcp_servers": disabled_mcp,
            "tools": {"web_search": false}, "features": {"multi_agent": false, "collab": false,
            "apps": false, "plugins": false, "hooks": false, "memories": false,
            "image_generation": false, "computer_use": false, "shell_snapshot": false}}
    });
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        params["model"] = json!(model);
    }
    if !active() {
        return Err(io::Error::other("Planning canceled"));
    }
    let started = client.call("thread/start", params)?;
    if started
        .pointer("/thread/ephemeral")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(io::Error::other(
            "Codex does not support internal planning threads",
        ));
    }
    let id = started
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("Missing planning thread"))?;
    if !active() {
        return Err(io::Error::other("Planning canceled"));
    }
    client.send(&json!({"id": "ditch:plan", "method": "turn/start", "params": {
        "threadId": id, "input": [{"type": "text", "text": format!("{INSTRUCTIONS}\n\nTask requirements (JSON):\n{}", json!({"title":title,"description":description}))}],
        "approvalPolicy": "never", "sandboxPolicy": {"type": "readOnly"},
        "outputSchema": {"type": "object", "properties": {"plan": {"type": "string"}}, "required": ["plan"], "additionalProperties": false}
    }}))?;
    let deadline = Instant::now() + Duration::from_secs(20 * 60);
    let mut result = None;
    loop {
        if !active() {
            return Err(io::Error::other("Planning canceled"));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("Planning timed out. Try again."));
        }
        let line = match client
            .incoming
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_millis(200))
        {
            Ok(line) => line?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err(io::Error::other("Codex disconnected. Try again.")),
        };
        let value: Value = serde_json::from_str(&line)?;
        // Hidden jobs cannot service external tools, approvals, or user input.
        if value.get("id").is_some() && value.get("method").is_some() {
            client.send(&json!({"id":value["id"],"error":{"code":-32601,"message":"Planning permits read-only local inspection only"}}))?;
            return Err(io::Error::other(
                "Planning requested an unsupported interaction",
            ));
        }
        if value["id"] == "ditch:plan" && value.get("error").is_some() {
            return Err(io::Error::other(
                "Codex could not start planning. Check your Codex sign-in and model.",
            ));
        }
        let p = &value["params"];
        if p["threadId"].as_str() != Some(id) {
            continue;
        }
        match value["method"].as_str() {
            Some("item/started" | "item/completed") if p["item"]["type"] == "fileChange" => {
                return Err(io::Error::other("Planning attempted a file change"));
            }
            Some("item/completed") if p["item"]["type"] == "agentMessage" => {
                if let Some(text) = p["item"]["text"].as_str() {
                    if text.len() > 128 * 1024 {
                        return Err(io::Error::other("Generated plan is too large"));
                    }
                    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                        result = parsed["plan"]
                            .as_str()
                            .filter(|s| !s.trim().is_empty() && s.len() <= 65536)
                            .map(str::to_owned);
                    }
                }
            }
            Some("turn/completed") => {
                if p["turn"]["status"] != "completed" {
                    return Err(io::Error::other(
                        "Planning failed. Check Codex and try again.",
                    ));
                }
                return result
                    .ok_or_else(|| io::Error::other("Codex returned no implementation plan"));
            }
            Some("error") if p["willRetry"].as_bool() != Some(true) => {
                return Err(io::Error::other(
                    "Planning failed. Check Codex and try again.",
                ));
            }
            _ => {}
        }
    }
}
