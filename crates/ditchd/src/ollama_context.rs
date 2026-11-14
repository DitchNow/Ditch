//! Verify serving limits and Responses semantics before sending a user's turn.
use super::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Api {
    agent: ureq::Agent,
    root: String,
    token: Option<EndpointToken>,
    deadline: std::time::Instant,
}
impl Api {
    fn new(endpoint: &OllamaEndpoint, token: Option<&EndpointToken>) -> io::Result<Self> {
        validate_token(token)?;
        let url = normalize_url(&endpoint.api_root_url, endpoint.allow_private_http)?;
        let addresses = validate_destination(&url)?;
        Ok(Self {
            agent: ureq::AgentBuilder::new()
                .redirects(0)
                .timeout(Duration::from_secs(120))
                .resolver(move |_: &str| Ok(addresses.clone()))
                .build(),
            root: url.as_str().trim_end_matches('/').into(),
            token: token.cloned(),
            deadline: std::time::Instant::now() + Duration::from_secs(75),
        })
    }
    fn request(&self, path: &str, payload: Option<Value>) -> Result<Value, Box<ureq::Error>> {
        let mut request = self.agent.request(
            if payload.is_some() { "POST" } else { "GET" },
            &format!("{}{path}", self.root),
        );
        // The entire preflight must finish before the UI's 100-second RPC timeout.
        request = request.timeout(
            self.deadline
                .saturating_duration_since(std::time::Instant::now())
                .max(Duration::from_millis(1)),
        );
        if let Some(token) = &self.token {
            request = request.set("Authorization", &format!("Bearer {}", token.0));
        }
        let response = match payload {
            Some(value) => request.send_json(value)?,
            None => request.call()?,
        };
        // Convert parsing failures without including a URL, credential, or response body.
        Ok(body(response)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null))
    }
    fn get(&self, path: &str, payload: Option<Value>) -> io::Result<Value> {
        self.request(path, payload).map_err(|_| {
            invalid(&format!(
                "Ollama {path} check failed. Check endpoint access and server logs."
            ))
        })
    }
}

fn configured_context(show: &Value) -> io::Result<u32> {
    // /api/show's model_info context_length is a training maximum, not allocation.
    let context = show["parameters"]
        .as_str()
        .unwrap_or("")
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next() == Some("num_ctx"))
                .then(|| fields.next()?.parse::<u32>().ok())
                .flatten()
        });
    match context {
        Some(value @ 8192..=262144) => Ok(value),
        _ => Err(invalid(
            "Configure this Ollama model with an explicit num_ctx between 8192 and 262144 that fits the server, then retry. Ditch needs a persistent serving budget; the model's advertised maximum is not its allocated context.",
        )),
    }
}

fn output_text(response: &Value) -> String {
    response["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter_map(|item| item["text"].as_str())
        .collect::<String>()
}

fn overflow_rejected(error: ureq::Error) -> bool {
    let ureq::Error::Status(400, response) = error else {
        return false;
    };
    let text = body(response).unwrap_or_default().to_lowercase();
    // A generic validation/authentication error does not prove overflow handling.
    text.contains("context") && (text.contains("exceed") || text.contains("too long"))
}

pub fn verify_model(
    endpoint: &OllamaEndpoint,
    token: Option<&EndpointToken>,
    model: &str,
) -> io::Result<OllamaModelStatus> {
    if !valid_model(model) {
        return Err(invalid("Enter an exact model ID"));
    }
    let api = Api::new(endpoint, token)?;
    let version = api.get("/api/version", None)?["version"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| invalid("Ollama version is unavailable"))?
        .to_owned();
    let show = api.get("/api/show", Some(json!({"model":model})))?;
    let context = configured_context(&show)?;
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&show)?));
    // Each launch/resume rechecks instructions, history, and actual allocation.
    // Only the expensive overflow probe is cached, bound to model config/version.
    let prefix = format!("D{}", Uuid::new_v4().simple());
    let marker = format!("H{}", Uuid::new_v4().simple());
    let response = api.get("/v1/responses", Some(json!({
        "model":model,"stream":false,"store":false,"temperature":0,
        "max_output_tokens":128,"reasoning":{"effort":"none"},"truncation":"disabled",
        "input":[
            {"role":"developer","content":format!("For this diagnostic, answer with exactly {prefix}| followed by the user's remembered marker. No other text.")},
            {"role":"user","content":format!("Remember this marker: {marker}")},
            {"role":"assistant","content":"OK."},
            {"role":"user","content":"Return the marker I asked you to remember, following your instructions."}
        ]
    })))?;
    if output_text(&response).trim() != format!("{prefix}|{marker}") {
        return Err(invalid(
            "Ollama instruction/history check failed. This model/server must preserve developer instructions and conversation history. Update or patch its renderer before using it with Codex.",
        ));
    }
    let ps = api.get("/api/ps", None)?;
    let loaded =
        ps["models"].as_array().into_iter().flatten().find(|item| {
            item["name"].as_str() == Some(model) || item["model"].as_str() == Some(model)
        });
    if loaded.and_then(|item| item["context_length"].as_u64()) != Some(u64::from(context)) {
        return Err(invalid(
            "Ollama's loaded context differs from this model's num_ctx. Reload the model with its configured context and retry.",
        ));
    }
    let cached = endpoint.model_status.get(model).is_some_and(|status| {
        status.context_window == context
            && status.model_fingerprint == fingerprint
            && status.server_version == version
            && status.rejects_overflow
            && (chrono::Utc::now() - status.verified_at).num_seconds() >= 0
            && (chrono::Utc::now() - status.verified_at).num_seconds() < 86400
    });
    if !cached {
        let overflow = api.request(
            "/v1/responses",
            Some(json!({
                "model":model,"stream":false,"store":false,"max_output_tokens":1,
                "reasoning":{"effort":"none"},"truncation":"disabled",
                "input":format!("{}\nReply OK.", " diagnostic".repeat(context as usize * 2))
            })),
        );
        if !overflow
            .err()
            .is_some_and(|error| overflow_rejected(*error))
        {
            return Err(invalid(
                "Ollama silently accepts oversized requests despite truncation=disabled. Install a server with enforced context-limit errors (upstream fix: 0.40.16) before using this endpoint.",
            ));
        }
    }
    Ok(OllamaModelStatus {
        tools_declared: show["capabilities"].as_array().is_some_and(|v| v.iter().any(|c| c == "tools")),
        vision_declared: show["capabilities"].as_array().is_some_and(|v| v.iter().any(|c| c == "vision")),
        context_window: context,
        model_fingerprint: fingerprint,
        server_version: version,
        // Do not extend the overflow-cache lifetime on every successful short probe.
        verified_at: if cached {
            endpoint.model_status[model].verified_at
        } else {
            chrono::Utc::now()
        },
        developer_instructions: true,
        conversation_recall: true,
        rejects_overflow: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn verification_rejects_role_loss_wrong_allocation_and_silent_truncation() {
        for failure in ["", "role", "allocation", "overflow", "cached", "version"] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let count = match failure {
                "role" => 3,
                "allocation" => 4,
                "cached" => 9,
                "version" => 10,
                _ => 5,
            };
            let server = std::thread::spawn(move || {
                for index in 0..count {
                    let phase = index / 5;
                    let index = index % 5;
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(10)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut headers = String::new();
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" {
                            break;
                        }
                        headers.push_str(&line);
                    }
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    let mut bytes = vec![0; length];
                    reader.read_exact(&mut bytes).unwrap();
                    let request: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    let (status, response) = match index {
                        0 => (
                            200,
                            json!({"version":if failure == "version" && phase > 0 {"changed"} else {"0.40.16-test"}}),
                        ),
                        1 => (200, json!({"parameters":"num_ctx 65536"})),
                        2 => {
                            assert_eq!(request["truncation"], "disabled");
                            let prefix = request["input"][0]["content"]
                                .as_str()
                                .unwrap()
                                .split("exactly ")
                                .nth(1)
                                .unwrap()
                                .split('|')
                                .next()
                                .unwrap();
                            let marker = request["input"][1]["content"]
                                .as_str()
                                .unwrap()
                                .split_whitespace()
                                .last()
                                .unwrap();
                            let text = if failure == "role" {
                                "Hello".to_owned()
                            } else {
                                format!("{prefix}|{marker}")
                            };
                            (
                                200,
                                json!({"output":[{"type":"message","content":[{"text":text}]}]}),
                            )
                        }
                        3 => (
                            200,
                            json!({"models":[{"name":"test:9b","context_length":if failure == "allocation" {32768} else {65536}}]}),
                        ),
                        _ if failure == "overflow" => (200, json!({"output":[]})),
                        _ => {
                            assert_eq!(request["truncation"], "disabled");
                            assert!(request["input"].as_str().unwrap().len() > 65536);
                            (
                                400,
                                json!({"error":{"message":"input exceeds context length"}}),
                            )
                        }
                    };
                    let body = response.to_string();
                    write!(stream,"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                }
            });
            let mut endpoint = OllamaEndpoint {
                id: Uuid::new_v4(),
                display_name: "Test".into(),
                api_root_url: format!("http://{address}"),
                auth_mode: "none".into(),
                has_credentials: false,
                allow_private_http: true,
                enabled: true,
                created_at: chrono::Utc::now(),
                models: vec![],
                reachable: None,
                error: None,
                model_status: Default::default(),
            };
            let mut result = verify_model(&endpoint, None, "test:9b");
            if matches!(failure, "cached" | "version") {
                endpoint
                    .model_status
                    .insert("test:9b".into(), result.unwrap());
                result = verify_model(&endpoint, None, "test:9b");
            }
            server.join().unwrap();
            assert_eq!(
                result.is_ok(),
                matches!(failure, "" | "cached" | "version"),
                "{failure}: {result:?}"
            );
            if let Ok(status) = result {
                assert_eq!(status.context_window, 65536);
                assert!(
                    status.developer_instructions
                        && status.conversation_recall
                        && status.rejects_overflow
                );
            }
        }
    }
    #[test]
    fn serving_budget_requires_explicit_model_configuration() {
        assert!(
            configured_context(&json!({"model_info":{"qwen35.context_length":262144}})).is_err()
        );
        assert_eq!(
            configured_context(&json!({"parameters":"temperature 1\nnum_ctx 65536\n"})).unwrap(),
            65536
        );
        for value in ["0", "4096", "262145", "unknown"] {
            assert!(configured_context(&json!({"parameters":format!("num_ctx {value}")})).is_err());
        }
    }
}
