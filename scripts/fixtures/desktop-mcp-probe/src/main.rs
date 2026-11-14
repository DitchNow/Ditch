//! Harmless MCP fixture for testing Codex discovery/resume with the official SDK.
//! No desktop APIs, runtime socket, model credentials, or filesystem tools.
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use serde_json::json;

#[derive(Clone)]
struct Probe {
    marker: String,
    image: Option<String>,
}

impl ServerHandler for Probe {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("ditch-desktop-probe", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let schema = json!({"type":"object", "properties":{}, "additionalProperties":false});
        let mut tools = vec![Tool::new(
            "desktop_status",
            "Return the isolated Ditch feasibility marker. Does not observe or control the desktop.",
            schema.as_object().unwrap().clone(),
        )];
        if self.image.is_some() {
            tools.push(Tool::new("desktop_observe",
                "Return a synthetic four-panel image. No screenshot, accessibility text, or desktop access.",
                schema.as_object().unwrap().clone()));
        }
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name == "desktop_observe" {
            if let Some(image) = &self.image {
                return Ok(
                    CallToolResult::success(vec![ContentBlock::image(image, "image/png")]).into(),
                );
            }
        }
        if request.name != "desktop_status" {
            return Ok(
                CallToolResult::error(vec![ContentBlock::text("Unknown probe tool")]).into(),
            );
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(
            json!({"probe_marker":self.marker, "desktop_access":false}).to_string(),
        )])
        .into())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let marker = std::env::var("DITCH_PROBE_MARKER")?;
    if marker.is_empty() || marker.len() > 128 {
        return Err("invalid probe marker".into());
    }
    let image = std::env::var("DITCH_PROBE_IMAGE").ok();
    Probe { marker, image }
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
