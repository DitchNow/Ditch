/// Relay validates string length in UTF-16 code units, as JavaScript does.
pub const SESSION_TITLE_MAX_UTF16: usize = 240;
pub const SESSION_TITLE_PENDING: &str = "Naming session…";
pub const SESSION_TITLE_FAILED: &str = "Unnamed session";

impl crate::AgentRun {
    pub fn display_name(&self) -> String {
        self.user_title
            .as_deref()
            .and_then(normalize_session_title)
            .or_else(|| {
                self.codex_title
                    .as_deref()
                    .and_then(normalize_session_title)
            })
            .unwrap_or_else(|| {
                if self.naming_failed {
                    SESSION_TITLE_FAILED
                } else {
                    SESSION_TITLE_PENDING
                }
                .to_owned()
            })
    }
}

pub fn normalize_session_title(value: &str) -> Option<String> {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut units = 0;
    let title: String = compact
        .chars()
        .take_while(|ch| {
            units += ch.len_utf16();
            units <= SESSION_TITLE_MAX_UTF16
        })
        .collect();
    let title = title.trim_end();
    (!title.is_empty()).then(|| title.to_owned())
}

pub fn session_display_name(user_title: Option<&str>, codex_name: Option<&str>) -> String {
    user_title
        .and_then(normalize_session_title)
        .or_else(|| codex_name.and_then(normalize_session_title))
        .unwrap_or_else(|| SESSION_TITLE_PENDING.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_title_waits_for_name_and_preserves_manual_rename() {
        assert_eq!(session_display_name(None, None), SESSION_TITLE_PENDING);
        assert_eq!(
            session_display_name(Some(" "), Some(" Fix\n login ")),
            "Fix login"
        );
        assert_eq!(
            session_display_name(Some("My name"), Some("Codex name")),
            "My name"
        );
    }

    #[test]
    fn session_title_obeys_relay_limit_without_splitting_unicode() {
        for source in [
            "a".repeat(300),
            "🙂".repeat(200),
            format!("{}🙂", "a".repeat(239)),
        ] {
            let title = normalize_session_title(&source).unwrap();
            assert!(title.encode_utf16().count() <= 240);
            assert!(source.starts_with(&title));
        }
        assert_eq!(normalize_session_title(" \n\t "), None);
    }
}
