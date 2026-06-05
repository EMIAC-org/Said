//! Shared PII scrubbing for telemetry and diagnostics payloads.

/// Redact `$HOME` → `~` and `/<username>/` → `/~/` in place.
pub fn scrub_string(s: &mut String) {
    let home = dirs::home_dir().and_then(|p| p.to_str().map(|s| s.to_owned()));
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root");

    if let Some(ref h) = home {
        if !h.is_empty() && s.contains(h) {
            *s = s.replace(h, "~");
        }
    }
    if let Some(ref u) = user {
        let needle = format!("/{u}/");
        if s.contains(&needle) {
            *s = s.replace(&needle, "/~/");
        }
    }
}

/// Walk a JSON value and scrub every string leaf.
pub fn scrub_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => scrub_string(s),
        serde_json::Value::Array(items) => {
            for item in items {
                scrub_json_value(item);
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                scrub_json_value(v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scrub_string_leaves_unrelated_paths() {
        let mut s = "/tmp/some/path".to_string();
        scrub_string(&mut s);
        assert_eq!(s, "/tmp/some/path", "non-home paths must not be altered");
    }

    #[test]
    fn scrub_string_leaves_empty() {
        let mut s = String::new();
        scrub_string(&mut s);
        assert_eq!(s, "", "empty string must survive scrub unchanged");
    }

    #[test]
    fn scrub_json_walks_nested_object() {
        // The scrub only redacts paths matching $HOME or /<user>/.
        // A plain safe string in a nested object must survive.
        let mut v = json!({ "outer": { "inner": "safe-value" } });
        scrub_json_value(&mut v);
        assert_eq!(v["outer"]["inner"], "safe-value");
    }

    #[test]
    fn scrub_json_walks_array() {
        let mut v = json!(["safe-one", "safe-two"]);
        scrub_json_value(&mut v);
        assert_eq!(v[0], "safe-one");
        assert_eq!(v[1], "safe-two");
    }

    #[test]
    fn scrub_json_ignores_numbers_and_booleans() {
        let mut v = json!({ "port": 8080, "enabled": true, "ratio": 0.5 });
        scrub_json_value(&mut v);
        assert_eq!(v["port"], 8080);
        assert_eq!(v["enabled"], true);
    }

    #[test]
    fn scrub_json_handles_null() {
        let mut v = json!(null);
        scrub_json_value(&mut v);
        assert!(v.is_null());
    }
}
