use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edition {
    Community,
    Commercial,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityId {
    RemoteControl,
    Rag,
    Grok,
    AgenticBots,
    PremiumIntegrations,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityTier {
    Community,
    Commercial,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityMetadata {
    pub id: CapabilityId,
    pub tier: CapabilityTier,
    pub title: &'static str,
    pub summary: &'static str,
}

pub const CAPABILITIES: &[CapabilityMetadata] = &[
    CapabilityMetadata {
        id: CapabilityId::RemoteControl,
        tier: CapabilityTier::Commercial,
        title: "Remote Control",
        summary: "Control Ditch from the proprietary iPhone app through the Ditch Relay.",
    },
    CapabilityMetadata {
        id: CapabilityId::Rag,
        tier: CapabilityTier::Commercial,
        title: "RAG",
        summary: "Commercial retrieval capabilities.",
    },
    CapabilityMetadata {
        id: CapabilityId::Grok,
        tier: CapabilityTier::Commercial,
        title: "Grok",
        summary: "Commercial Grok integration.",
    },
    CapabilityMetadata {
        id: CapabilityId::AgenticBots,
        tier: CapabilityTier::Commercial,
        title: "Agentic bots",
        summary: "Commercial autonomous integrations.",
    },
    CapabilityMetadata {
        id: CapabilityId::PremiumIntegrations,
        tier: CapabilityTier::Commercial,
        title: "Premium integrations",
        summary: "Commercial third-party integrations.",
    },
];

pub fn capability(id: CapabilityId) -> &'static CapabilityMetadata {
    CAPABILITIES
        .iter()
        .find(|metadata| metadata.id == id)
        .expect("every CapabilityId must have metadata")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_capability_has_unique_metadata() {
        let mut ids = std::collections::HashSet::new();
        for metadata in CAPABILITIES {
            assert!(ids.insert(metadata.id));
        }
        assert_eq!(
            capability(CapabilityId::RemoteControl).tier,
            CapabilityTier::Commercial
        );
    }
}
