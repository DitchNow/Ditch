//! Ditch's internal stdio MCP transport. Desktop authority lives in ditchd.
use ditch_core::tool_integrations::{
    AdapterRequest, MAX_REQUEST_BYTES, MAX_RESULT_BYTES, desktop_tool_schemas,
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
struct Adapter {
    socket: PathBuf,
    credential: String,
    requests: Arc<Mutex<HashMap<String, (uuid::Uuid, u64)>>>,
}

impl Adapter {
    fn operation_id(
        &self,
        rpc_id: String,
        name: &str,
        arguments: &Value,
    ) -> std::io::Result<uuid::Uuid> {
        let bytes = serde_json::to_vec(&(name, arguments))?;
        if rpc_id.len() > 256 || bytes.len() > MAX_REQUEST_BYTES {
            return Err(std::io::Error::other("request too large"));
        }
        // Keep only a fingerprint, never a second cache of typed/observed text.
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let fingerprint = hasher.finish();
        let mut requests = self.requests.lock().unwrap();
        if let Some((id, previous)) = requests.get(&rpc_id) {
            return if *previous == fingerprint {
                Ok(*id)
            } else {
                Err(std::io::Error::other(
                    "request ID reused with different arguments",
                ))
            };
        }
        if requests.len() >= 10000 {
            return Err(std::io::Error::other("turn tool-call limit reached"));
        }
        let id = uuid::Uuid::new_v4();
        requests.insert(rpc_id, (id, fingerprint));
        Ok(id)
    }

    async fn request(
        &self,
        operation_id: uuid::Uuid,
        name: String,
        arguments: Value,
    ) -> Result<Value, std::io::Error> {
        let request = AdapterRequest {
            credential: self.credential.clone(),
            operation_id,
            tool: name,
            arguments,
        };
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(std::io::Error::other("request too large"));
        }
        let mut stream = tokio::net::UnixStream::connect(&self.socket).await?;
        stream.write_all(&bytes).await?;
        let mut output = String::new();
        tokio::io::BufReader::new(stream.take((MAX_RESULT_BYTES + 1) as u64))
            .read_line(&mut output)
            .await?;
        if output.len() > MAX_RESULT_BYTES || !output.ends_with('\n') {
            return Err(std::io::Error::other("invalid result"));
        }
        serde_json::from_str(&output).map_err(std::io::Error::other)
    }
}

impl ServerHandler for Adapter {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("ditch_tools", env!("CARGO_PKG_VERSION")),
        )
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let connected = tokio::time::timeout(
            Duration::from_secs(3),
            self.request(
                uuid::Uuid::new_v4(),
                ditch_core::tool_integrations::ADAPTER_READY.into(),
                json!({}),
            ),
        )
        .await;
        match connected {
            Ok(Ok(ref value)) if value["ok"] == true => {}
            Ok(Ok(value)) => {
                let reason = match value["error"].as_str() {
                    Some("invalidCredential") => "invalidCredential",
                    Some("revoked") => "revoked",
                    Some("adapterChanged") => "adapterChanged",
                    _ => "handshake rejected",
                };
                return Err(ErrorData::internal_error(
                    format!("Ditch adapter authentication failed: {reason}"), None,
                ));
            }
            Err(_) => return Err(ErrorData::internal_error(
                "Ditch adapter authentication timed out", None,
            )),
            Ok(Err(_)) => return Err(ErrorData::internal_error(
                "Ditch runtime connection unavailable (transport or peer authentication)", None,
            )),
        }
        Ok(ListToolsResult {
            tools: desktop_tool_schemas()
                .into_iter()
                .map(|(name, description, schema)| {
                    Tool::new(name, description, schema.as_object().unwrap().clone())
                })
                .collect(),
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !desktop_tool_schemas()
            .iter()
            .any(|t| t.0 == request.name.as_ref())
        {
            return Ok(
                CallToolResult::error(vec![ContentBlock::text("Unknown Ditch tool")]).into(),
            );
        }
        let arguments = json!(request.arguments.unwrap_or_default());
        let operation_id = match self.operation_id(
            serde_json::to_string(&context.id).unwrap_or_default(),
            &request.name,
            &arguments,
        ) {
            Ok(id) => id,
            Err(_) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(
                    "Invalid, reused, or excessive tool request; start a fresh turn.",
                )])
                .into());
            }
        };
        let request = self.request(operation_id, request.name.to_string(), arguments);
        let response = tokio::select! {
            _=context.ct.cancelled()=>return Ok(CallToolResult::error(vec![ContentBlock::text("Desktop operation cancelled")]).into()),
            response=tokio::time::timeout(Duration::from_secs(14),request)=>response,
        };
        let value=match response {
            Ok(Ok(value))=>value,
            _=>return Ok(CallToolResult::error(vec![ContentBlock::text("Ditch computer use is unavailable or the operation timed out. Do not repeat an uncertain action; observe again in a fresh authorized turn.")]).into()),
        };
        if value["ok"] != true {
            return Ok(CallToolResult::error(vec![ContentBlock::text(value.to_string())]).into());
        }
        let mut result = value["result"].clone();
        let image = result.as_object_mut().and_then(|v| v.remove("image"));
        let mut content = vec![ContentBlock::text(result.to_string())];
        if let Some(image) = image {
            if let (Some(data), Some(mime)) = (image["data"].as_str(), image["mime_type"].as_str())
            {
                if mime == "image/png" && data.len() <= 2_800_000 {
                    content.push(ContentBlock::image(data, mime));
                } else {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "Invalid image result",
                    )])
                    .into());
                }
            }
        }
        Ok(CallToolResult::success(content).into())
    }
}

pub fn serve() -> Result<(), i32> {
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let socket =
            PathBuf::from(std::env::var_os("DITCH_TOOL_SOCKET").ok_or("missing scoped socket")?);
        let path = PathBuf::from(
            std::env::var_os("DITCH_TOOL_CREDENTIAL_FILE").ok_or("missing scoped credential")?,
        );
        if !socket.is_absolute() || !path.is_absolute() {
            return Err("invalid scoped paths".into());
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
            || meta.len() != 64
        {
            return Err("invalid credential file".into());
        }
        let mut credential = String::new();
        file.take(65).read_to_string(&mut credential)?;
        if credential.len() != 64 || !credential.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid credential".into());
        }
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?
            .block_on(async move {
                Adapter {
                    socket,
                    credential,
                    requests: Arc::new(Mutex::new(HashMap::new())),
                }
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
                Ok::<_, Box<dyn std::error::Error>>(())
            })
    };
    if run().is_err() {
        eprintln!("Ditch tool adapter unavailable; start it through an authorized Ditch session.");
        Err(1)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_rpc_ids_keep_the_operation_id_and_changed_arguments_are_rejected() {
        let adapter = Adapter {
            socket: PathBuf::new(),
            credential: String::new(),
            requests: Default::default(),
        };
        let args = json!({"action":"type","text":"private text"});
        let first = adapter
            .operation_id("1".into(), "desktop_act", &args)
            .unwrap();
        assert_eq!(
            adapter
                .operation_id("1".into(), "desktop_act", &args)
                .unwrap(),
            first
        );
        assert!(
            adapter
                .operation_id("1".into(), "desktop_act", &json!({"text":"different"}))
                .is_err()
        );
        assert_ne!(
            adapter
                .operation_id("2".into(), "desktop_act", &args)
                .unwrap(),
            first
        );
    }
}
