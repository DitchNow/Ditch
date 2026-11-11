use crate::ProjectId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Only request payloads carry credentials. Debug output must never expose them.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EndpointToken(pub String);
impl std::fmt::Debug for EndpointToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OllamaEndpoint {
    pub id: Uuid,
    pub display_name: String,
    pub api_root_url: String,
    pub auth_mode: String,
    pub has_credentials: bool,
    pub allow_private_http: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub models: Vec<String>,
    pub reachable: Option<bool>,
    pub error: Option<String>,
}
impl OllamaEndpoint {
    pub fn provider_key(&self) -> String {
        format!("ditch_ollama_{}", self.id.simple())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OllamaRequest {
    pub project_id: ProjectId,
    pub operation: OllamaOperation,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OllamaOperation {
    List,
    Test {
        api_url: String,
        auth: Option<EndpointToken>,
        #[serde(default)]
        allow_private_http: bool,
        model: Option<String>,
    },
    Add {
        display_name: String,
        api_url: String,
        auth: Option<EndpointToken>,
        #[serde(default)]
        allow_private_http: bool,
        model: String,
    },
    Refresh {
        endpoint_id: Uuid,
    },
    Disable {
        endpoint_id: Uuid,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OllamaProbe {
    pub models: Vec<String>,
    pub reachable: bool,
    pub error: Option<String>,
    /// A bounded Responses/tool probe, not a claim of end-to-end coding quality.
    pub responses_tool_check: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OllamaResponse {
    Endpoints(Vec<OllamaEndpoint>),
    Endpoint(OllamaEndpoint),
    Probe(OllamaProbe),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_are_backward_compatible_and_credentials_are_redacted() {
        let old = r#"{"model":"gpt-test","reasoning_effort":null}"#;
        let mut profile: crate::AgentExecutionProfile = serde_json::from_str(old).unwrap();
        assert_eq!(profile.inference_endpoint_id, None);
        profile.inference_endpoint_id = Some(Uuid::new_v4());
        assert_eq!(
            profile,
            serde_json::from_str(&serde_json::to_string(&profile).unwrap()).unwrap()
        );
        assert!(!format!("{:?}", EndpointToken("secret-token".into())).contains("secret-token"));
    }
}
