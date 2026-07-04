use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const TRACE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictationTrace {
    pub version: u32,
    pub texts: BTreeMap<String, TraceText>,
    pub stages: Vec<TraceStage>,
    pub summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceText {
    pub hash: String,
    pub chars: usize,
    pub words: usize,
    pub redacted: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStage {
    pub index: usize,
    pub stage: String,
    pub component: String,
    pub function: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<String>,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Default)]
pub struct TraceStageInput<'a> {
    pub stage: &'a str,
    pub component: &'a str,
    pub function: &'a str,
    pub input: Option<&'a str>,
    pub output: Option<&'a str>,
    pub duration_ms: Option<i64>,
    pub reason: Option<&'a str>,
    pub risk: Option<&'a str>,
    pub metadata: Value,
}

impl Default for DictationTrace {
    fn default() -> Self {
        Self {
            version: TRACE_VERSION,
            texts: BTreeMap::new(),
            stages: Vec::new(),
            summary: json!({}),
        }
    }
}

impl DictationTrace {
    pub fn add_text(&mut self, text: &str) -> String {
        let (redacted_text, redacted) = redact_text(text);
        let hash = stable_hash(&redacted_text);
        let key = format!("t_{hash}");
        self.texts.entry(key.clone()).or_insert_with(|| TraceText {
            hash,
            chars: redacted_text.chars().count(),
            words: redacted_text.split_whitespace().count(),
            redacted,
            text: redacted_text,
        });
        key
    }

    pub fn add_stage(&mut self, input: TraceStageInput<'_>) {
        let input_ref = input.input.map(|text| self.add_text(text));
        let output_ref = input.output.map(|text| self.add_text(text));
        let changed = match (input.input, input.output) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        };
        let index = self.stages.len();
        self.stages.push(TraceStage {
            index,
            stage: input.stage.to_string(),
            component: input.component.to_string(),
            function: input.function.to_string(),
            input_ref,
            output_ref,
            changed,
            duration_ms: input.duration_ms,
            reason: input.reason.map(str::to_string),
            risk: input.risk.map(str::to_string),
            metadata: input.metadata,
        });
    }

    pub fn set_summary_field(&mut self, key: &str, value: Value) {
        if !self.summary.is_object() {
            self.summary = Value::Object(Map::new());
        }
        if let Some(obj) = self.summary.as_object_mut() {
            obj.insert(key.to_string(), value);
        }
    }

    pub fn merge(&mut self, other: DictationTrace) {
        for (key, text) in other.texts {
            self.texts.entry(key).or_insert(text);
        }
        for mut stage in other.stages {
            stage.index = self.stages.len();
            self.stages.push(stage);
        }
        merge_object_values(&mut self.summary, other.summary);
    }

    pub fn is_empty(&self) -> bool {
        self.texts.is_empty() && self.stages.is_empty() && is_empty_object(&self.summary)
    }

    pub fn into_value(self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

pub fn merge_trace_values(base: Option<&Value>, patch: Option<&Value>) -> Value {
    let mut trace = parse_trace_value(base).unwrap_or_default();
    if let Some(patch_trace) = parse_trace_value(patch) {
        trace.merge(patch_trace);
    }
    if trace.is_empty() {
        json!({})
    } else {
        trace.into_value()
    }
}

pub fn parse_trace_value(value: Option<&Value>) -> Option<DictationTrace> {
    let value = value?;
    if is_empty_object(value) {
        return None;
    }
    serde_json::from_value::<DictationTrace>(value.clone()).ok()
}

pub fn stage_text<'a>(trace: &'a DictationTrace, reference: &Option<String>) -> Option<&'a str> {
    reference
        .as_ref()
        .and_then(|key| trace.texts.get(key))
        .map(|entry| entry.text.as_str())
}

fn merge_object_values(base: &mut Value, patch: Value) {
    if !base.is_object() {
        *base = json!({});
    }
    let Some(base_obj) = base.as_object_mut() else {
        return;
    };
    let Some(patch_obj) = patch.as_object() else {
        return;
    };
    for (key, value) in patch_obj {
        base_obj.insert(key.clone(), value.clone());
    }
}

fn is_empty_object(value: &Value) -> bool {
    value.as_object().map(|o| o.is_empty()).unwrap_or(false)
}

fn redact_text(text: &str) -> (String, bool) {
    let mut redacted = false;
    let mut output = Vec::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("authorization: bearer ")
            || lower.contains("api_key=")
            || lower.contains("api key")
            || lower.contains("password=")
            || lower.contains("token=")
            || lower.contains("secret=")
        {
            redacted = true;
            output.push(redact_assignment_like_line(line));
        } else {
            let replaced = line
                .split_whitespace()
                .map(|token| {
                    if looks_like_secret_token(token) {
                        redacted = true;
                        "[REDACTED_SECRET]".to_string()
                    } else {
                        token.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            output.push(replaced);
        }
    }
    (output.join("\n"), redacted)
}

fn redact_assignment_like_line(line: &str) -> String {
    for needle in ["Authorization: Bearer ", "authorization: bearer "] {
        if let Some(pos) = line.find(needle) {
            return format!("{}{}[REDACTED_SECRET]", &line[..pos], needle);
        }
    }
    for sep in ['=', ':'] {
        if let Some(pos) = line.find(sep) {
            return format!("{}{} [REDACTED_SECRET]", &line[..pos], sep);
        }
    }
    "[REDACTED_SECRET]".to_string()
}

fn looks_like_secret_token(token: &str) -> bool {
    let trimmed = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
    if trimmed.len() < 20 {
        return false;
    }
    trimmed.starts_with("sk-")
        || trimmed.starts_with("ghp_")
        || trimmed.starts_with("xox")
        || trimmed.starts_with("eyJ")
        || (trimmed.len() >= 32
            && trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
}

fn stable_hash(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_dedupes_repeated_text() {
        let mut trace = DictationTrace::default();
        let a = trace.add_text("same text");
        let b = trace.add_text("same text");
        assert_eq!(a, b);
        assert_eq!(trace.texts.len(), 1);
    }

    #[test]
    fn trace_stage_marks_changed_only_when_text_changes() {
        let mut trace = DictationTrace::default();
        trace.add_stage(TraceStageInput {
            stage: "same",
            component: "test",
            function: "noop",
            input: Some("hello"),
            output: Some("hello"),
            ..Default::default()
        });
        trace.add_stage(TraceStageInput {
            stage: "changed",
            component: "test",
            function: "mutate",
            input: Some("hello"),
            output: Some("Hello"),
            ..Default::default()
        });
        assert!(!trace.stages[0].changed);
        assert!(trace.stages[1].changed);
    }

    #[test]
    fn trace_redacts_obvious_secrets() {
        let mut trace = DictationTrace::default();
        let key = trace.add_text("Authorization: Bearer sk-abcdefghijklmnopqrstuvwxyz");
        let text = trace.texts.get(&key).unwrap();
        assert!(text.redacted);
        assert!(!text.text.contains("abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn merge_appends_stages_and_summary() {
        let mut base = DictationTrace::default();
        base.set_summary_field("model", json!("a"));
        base.add_stage(TraceStageInput {
            stage: "one",
            component: "a",
            function: "f",
            ..Default::default()
        });
        let mut patch = DictationTrace::default();
        patch.set_summary_field("classify", json!("ok"));
        patch.add_stage(TraceStageInput {
            stage: "two",
            component: "b",
            function: "g",
            ..Default::default()
        });
        base.merge(patch);
        assert_eq!(base.stages.len(), 2);
        assert_eq!(base.stages[1].index, 1);
        assert_eq!(base.summary["model"], "a");
        assert_eq!(base.summary["classify"], "ok");
    }
}
