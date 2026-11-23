//! Releases 1.0.28 to 1.0.31 saved a lane's file-edit choice as `"edits"` next to its
//! profile. Permission modes replaced it; a lane that chose to accept edits keeps doing so.
use serde_json::{Map, Value};

/// Moves every lane's `"edits": "accept_edits"` into its profile's mode, unless the profile
/// already has one. Lanes sit in templates, drafts and runs, so the whole section is walked;
/// only a lane has an `edits` field. The field itself stays and is ignored when parsed.
pub(super) fn fold(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(fold),
        Value::Object(object) => {
            if object.get("edits").and_then(Value::as_str) == Some("accept_edits")
                && let Value::Object(profile) = object
                    .entry("profile")
                    .or_insert_with(|| Value::Object(Map::new()))
            {
                profile
                    .entry("mode")
                    .or_insert_with(|| "accept_edits".into());
            }
            object.values_mut().for_each(fold);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::fold;
    use serde_json::json;

    #[test]
    fn a_lane_that_accepted_edits_keeps_doing_so_as_its_mode() {
        let mut saved = json!({
            "templates": [{"lanes": [
                {"name": "api", "profile": {"agent": "Codex"}, "edits": "accept_edits"},
                {"name": "web", "profile": {"agent": "Codex"}, "edits": "cli_default"},
            ]}],
            "runs": [{"lanes": [
                {"name": "api", "edits": "accept_edits"},
                {"name": "web", "profile": {"mode": "plan"}, "edits": "accept_edits"},
                {"name": "odd", "profile": "unreadable", "edits": "accept_edits"},
            ]}],
            "drafts": [{"lanes": [{"preset": {"profile": {}, "edits": "accept_edits"}}]}],
        });
        fold(&mut saved);
        let templates = &saved["templates"][0]["lanes"];
        assert_eq!(templates[0]["profile"]["mode"], "accept_edits");
        assert!(templates[1]["profile"].get("mode").is_none());
        let runs = &saved["runs"][0]["lanes"];
        // A lane saved without a profile still gets the mode; the rest takes its defaults.
        assert_eq!(runs[0]["profile"], json!({"mode": "accept_edits"}));
        // A mode already chosen wins, and a damaged profile is left for the parser to refuse.
        assert_eq!(runs[1]["profile"]["mode"], "plan");
        assert_eq!(runs[2]["profile"], "unreadable");
        assert_eq!(
            saved["drafts"][0]["lanes"][0]["preset"]["profile"]["mode"],
            "accept_edits"
        );
    }
}
