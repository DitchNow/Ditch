use serde_json::Value;
use std::hash::{Hash, Hasher};

/// Per-turn limits survive model retries and cannot be reset by status/discovery.
#[derive(Default)]
pub(super) struct InteractionPolicy {
    operations: usize,
    captures: usize,
    observations_without_action: usize,
    failures: usize,
    last_image: Option<u64>,
    unchanged_images: usize,
}

impl InteractionPolicy {
    pub fn before(&mut self, tool: &str) -> Result<(), &'static str> {
        if tool == "desktop_status" {
            return Ok(());
        }
        if self.unchanged_images >= 3 {
            return Err("desktopNoProgress");
        }
        if self.operations >= 120 || self.captures >= 40 || self.failures >= 3 {
            return Err("desktopRecoveryLimit");
        }
        if tool == "desktop_observe" && self.observations_without_action >= 3 {
            return Err("observationLimit");
        }
        self.operations += 1;
        if tool == "desktop_observe" {
            self.captures += 1;
            self.observations_without_action += 1;
        }
        Ok(())
    }

    pub fn after(&mut self, tool: &str, result: &Value) {
        if tool == "desktop_list_targets" {
            // Transport success can still yield no usable target (no match,
            // no visible windows, or every candidate unavailable). Count it as
            // failed recovery; listing/status must never reset the budget.
            if result["ok"] != true
                || result["result"]["targets"].as_array().is_none_or(|targets| targets.is_empty())
            {
                self.failures += 1;
            }
            return;
        }
        if !matches!(tool, "desktop_observe" | "desktop_act") {
            return;
        }
        if result["ok"] == true {
            self.failures = 0;
            if tool == "desktop_act" {
                self.observations_without_action = 0;
            }
            if tool == "desktop_observe" {
                if let Some(image) = result["result"]["image"]["data"].as_str() {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    result["result"]["target_id"].as_str().hash(&mut hasher);
                    image.hash(&mut hasher);
                    let hash = hasher.finish();
                    self.unchanged_images = if self.last_image == Some(hash) {
                        self.unchanged_images + 1
                    } else {
                        0
                    };
                    self.last_image = Some(hash);
                }
            }
        } else {
            self.failures += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn failed_or_empty_discovery_stops_and_status_cannot_erase_it() {
        for result in [json!({"ok":false,"error":"applicationGone"}),
            json!({"ok":true,"result":{"status":"unavailable","targets":[],"unavailable":[{"error":"processIdentityUnavailable"}]}}),
            json!({"ok":true,"result":{"status":"noMatches","targets":[]}}),
            json!({"ok":true,"result":{"status":"noVisibleWindows","targets":[]}})] {
            let mut policy = InteractionPolicy::default();
            for _ in 0..3 {
                assert!(policy.before("desktop_list_targets").is_ok());
                policy.after("desktop_list_targets", &result);
                policy.after("desktop_status", &json!({"ok":true}));
            }
            assert_eq!(policy.before("desktop_list_targets"), Err("desktopRecoveryLimit"));
            assert!(policy.before("desktop_status").is_ok());
        }
    }

    #[test]
    fn stale_reference_allows_rediscovery_and_observation_restores_progress() {
        let mut policy = InteractionPolicy::default();
        policy.after("desktop_observe", &json!({"ok":false,"error":"targetNotDiscovered"}));
        for _ in 0..2 {
            assert!(policy.before("desktop_list_targets").is_ok());
            policy.after("desktop_list_targets", &json!({"ok":true,"result":{"status":"partial","targets":[{"target_id":"t_valid"}],"unavailable":[{"error":"processIdentityUnavailable"}]}}));
        }
        assert_eq!(policy.failures, 1);
        assert!(policy.before("desktop_observe").is_ok());
        policy.after("desktop_observe", &json!({"ok":true,"result":{"snapshot_id":"s_valid"}}));
        assert_eq!(policy.failures, 0);
        assert!(policy.before("desktop_act").is_ok());
    }
    #[test]
    fn captures_stop_until_an_action_succeeds() {
        let mut policy = InteractionPolicy::default();
        for _ in 0..3 {
            assert!(policy.before("desktop_observe").is_ok());
        }
        assert_eq!(policy.before("desktop_observe"), Err("observationLimit"));
        policy.after("desktop_list_targets", &json!({"ok":true,"result":{"targets":[{"target_id":"t_test"}]}}));
        assert!(policy.before("desktop_observe").is_err());
        policy.after("desktop_act", &json!({"ok":true}));
        assert!(policy.before("desktop_observe").is_ok());
    }
    #[test]
    fn discovery_does_not_erase_failures_and_new_turn_resets() {
        let mut policy = InteractionPolicy::default();
        for _ in 0..3 {
            assert!(policy.before("desktop_act").is_ok());
            policy.after("desktop_act", &json!({"ok":false}));
            policy.after("desktop_list_targets", &json!({"ok":true,"result":{"targets":[{"target_id":"t_test"}]}}));
        }
        assert_eq!(policy.before("desktop_act"), Err("desktopRecoveryLimit"));
        assert!(policy.before("desktop_status").is_ok());
        assert!(InteractionPolicy::default().before("desktop_act").is_ok());
    }
    #[test]
    fn alternating_actions_cannot_bypass_total_capture_budget() {
        let mut policy = InteractionPolicy::default();
        for _ in 0..40 {
            assert!(policy.before("desktop_observe").is_ok());
            policy.after("desktop_act", &json!({"ok":true}));
        }
        assert_eq!(
            policy.before("desktop_observe"),
            Err("desktopRecoveryLimit")
        );
    }
    #[test]
    fn successful_dispatch_cannot_hide_repeated_unchanged_images() {
        let mut policy = InteractionPolicy::default();
        for _ in 0..4 {
            assert!(policy.before("desktop_observe").is_ok());
            policy.after(
                "desktop_observe",
                &serde_json::json!({"ok":true,"result":{"image":{"data":"same-image"}}}),
            );
            policy.after("desktop_act", &serde_json::json!({"ok":true}));
        }
        assert_eq!(policy.before("desktop_act"), Err("desktopNoProgress"));
        assert!(policy.before("desktop_status").is_ok());
    }
}
