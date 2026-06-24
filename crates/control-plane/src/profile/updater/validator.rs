//! Rust validator — merge DeepSeek patch into profile_json, promote aliases, cap sizes.

use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::profile::alias::{ProfileAlias, ProfileAliasStatus, validate_alias_candidate};
use crate::profile::alias_safety::normalize_alias_phrase;
use crate::profile::store::{self, PROFILE_MARKDOWN_MAX_BYTES};
use crate::profile::updater::types::{
    AliasChangeRecord, DeepSeekAliasProposal, DeepSeekClassification,
    DeepSeekProfileUpdateResponse, LearnAuditPayload,
};

const RECENT_CONTEXT_MAX: usize = 5;
const RECENT_CONTEXT_ENTRY_MAX: usize = 200;
const ALLOWED_ALIAS_TERM_TYPES: &[&str] = &[
    "brand",
    "acronym",
    "code_identifier",
    "proper_noun",
    "phrase",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatorDecision {
    Applied,
    Shadow,
    Rejected,
}

pub struct ValidatorInput {
    pub current_json: Value,
    pub current_markdown: String,
    pub current_version: i64,
    pub deepseek: DeepSeekProfileUpdateResponse,
    pub update_mode_apply: bool,
    pub request_id: Uuid,
    pub edit_event_id: String,
    pub recording_id: Option<String>,
    pub client_run_id: Option<String>,
    pub run_id: Option<Uuid>,
    pub latency_ms: u64,
}

pub struct ValidatorOutput {
    pub decision: ValidatorDecision,
    pub reasons: Vec<String>,
    pub merged_json: Value,
    pub merged_markdown: String,
    pub alias_changes: Vec<AliasChangeRecord>,
    pub delta_summary: Value,
    pub review_required: bool,
    pub audit_payload: LearnAuditPayload,
}

pub fn validate_and_merge(input: ValidatorInput) -> ValidatorOutput {
    let mut reasons = Vec::new();
    let ds = &input.deepseek;

    if matches!(
        ds.classification,
        DeepSeekClassification::NoLearning | DeepSeekClassification::UserRewrite
    ) {
        reasons.push(format!(
            "classification {:?} skips profile write",
            ds.classification
        ));
        return rejected_output(input, reasons);
    }

    if ds.profile_patch.user_background.is_none()
        && ds.profile_patch.add_focus_areas.is_empty()
        && ds.profile_patch.add_speech_patterns.is_empty()
        && ds.profile_patch.add_recent_context.is_empty()
        && ds.profile_patch.add_domains.is_empty()
        && ds.profile_patch.add_stable_terms.is_empty()
        && ds.profile_patch.add_stt_confusions.is_empty()
        && ds.profile_patch.add_negative_rules.is_empty()
        && ds.profile_patch.style_updates.is_empty()
        && ds.alias_proposals.is_empty()
        && ds.profile_markdown_patch.mode.is_none()
    {
        reasons.push("empty patch from DeepSeek".to_string());
        return rejected_output(input, reasons);
    }

    let mut merged = input.current_json.clone();
    if !merged.is_object() {
        merged = json!({});
    }

    let mut terms_added = 0usize;
    let mut aliases_updated = 0usize;
    let mut profile_sections_updated = 0usize;

    profile_sections_updated +=
        merge_user_background(&mut merged, &ds.profile_patch.user_background);
    profile_sections_updated += merge_focus_areas(&mut merged, &ds.profile_patch.add_focus_areas);
    profile_sections_updated +=
        merge_speech_patterns(&mut merged, &ds.profile_patch.add_speech_patterns);
    profile_sections_updated +=
        merge_recent_context_notes(&mut merged, &ds.profile_patch.add_recent_context);
    merge_domains(&mut merged, &ds.profile_patch.add_domains);
    terms_added += merge_stable_terms(&mut merged, &ds.profile_patch.add_stable_terms);
    merge_stt_confusions(&mut merged, &ds.profile_patch.add_stt_confusions);
    merge_negative_rules(&mut merged, &ds.profile_patch.add_negative_rules);
    merge_style_updates(&mut merged, &ds.profile_patch.style_updates);

    let (alias_changes, alias_count) =
        merge_alias_proposals(&mut merged, &ds.alias_proposals, input.current_version + 1);
    aliases_updated += alias_count;

    if terms_added > 0 || aliases_updated > 0 || profile_sections_updated > 0 {
        append_recent_context(&mut merged, &ds.reason);
    }

    let merged_markdown = resolve_markdown(
        &input.current_markdown,
        &merged,
        &ds.profile_markdown_patch,
        &mut reasons,
    );

    if let Err(e) = store::validate_profile_sizes(&merged, &merged_markdown) {
        reasons.push(e);
        return rejected_output(input, reasons);
    }

    if instruction_like_markdown(&merged_markdown) {
        reasons.push("markdown contains instruction-like content".to_string());
        return rejected_output(input, reasons);
    }

    if terms_added == 0
        && aliases_updated == 0
        && profile_sections_updated == 0
        && merged == input.current_json
        && merged_markdown == input.current_markdown
    {
        reasons.push("no effective profile changes after validation".to_string());
        return rejected_output(input, reasons);
    }

    let delta_summary = json!({
        "terms_added": terms_added,
        "aliases_updated": aliases_updated,
        "profile_sections_updated": profile_sections_updated,
    });

    let review_required = ds.review_required
        || alias_changes
            .iter()
            .any(|c| c.to_status == "candidate" && ds.confidence >= 0.82);

    let decision = if input.update_mode_apply {
        ValidatorDecision::Applied
    } else {
        ValidatorDecision::Shadow
    };

    let validator_decision = match decision {
        ValidatorDecision::Applied => "applied",
        ValidatorDecision::Shadow => "shadow",
        ValidatorDecision::Rejected => "rejected",
    };

    let audit_payload = LearnAuditPayload {
        edit_event_id: input.edit_event_id.clone(),
        recording_id: input.recording_id.clone(),
        client_run_id: input.client_run_id.clone(),
        run_id: input.run_id,
        job_id: None,
        deepseek_classification: Some(format!("{:?}", ds.classification).to_ascii_lowercase()),
        deepseek_confidence: Some(ds.confidence),
        deepseek_reason: Some(ds.reason.clone()),
        validator_decision: Some(validator_decision.to_string()),
        validator_reasons: if reasons.is_empty() {
            None
        } else {
            Some(reasons.clone())
        },
        alias_changes: if alias_changes.is_empty() {
            None
        } else {
            Some(alias_changes.clone())
        },
        profile_json_delta_summary: Some(delta_summary.clone()),
        deepseek_request_id: Some(input.request_id),
        latency_ms: Some(input.latency_ms),
        shadow_would_apply: Some(
            decision == ValidatorDecision::Applied || decision == ValidatorDecision::Shadow,
        ),
    };

    ValidatorOutput {
        decision,
        reasons,
        merged_json: merged,
        merged_markdown,
        alias_changes,
        delta_summary,
        review_required,
        audit_payload,
    }
}

fn rejected_output(input: ValidatorInput, reasons: Vec<String>) -> ValidatorOutput {
    let audit_payload = LearnAuditPayload {
        edit_event_id: input.edit_event_id.clone(),
        recording_id: input.recording_id.clone(),
        client_run_id: input.client_run_id.clone(),
        run_id: input.run_id,
        job_id: None,
        deepseek_classification: Some(
            format!("{:?}", input.deepseek.classification).to_ascii_lowercase(),
        ),
        deepseek_confidence: Some(input.deepseek.confidence),
        deepseek_reason: Some(input.deepseek.reason.clone()),
        validator_decision: Some("rejected".to_string()),
        validator_reasons: Some(reasons.clone()),
        alias_changes: None,
        profile_json_delta_summary: None,
        deepseek_request_id: Some(input.request_id),
        latency_ms: Some(input.latency_ms),
        shadow_would_apply: Some(false),
    };

    ValidatorOutput {
        decision: ValidatorDecision::Rejected,
        reasons,
        merged_json: input.current_json,
        merged_markdown: input.current_markdown,
        alias_changes: Vec::new(),
        delta_summary: json!({}),
        review_required: input.deepseek.review_required,
        audit_payload,
    }
}

fn merge_user_background(
    profile: &mut Value,
    background: &Option<crate::profile::updater::types::PatchUserBackground>,
) -> usize {
    let Some(background) = background else {
        return 0;
    };
    let summary = background.summary.trim();
    if summary.is_empty() {
        return 0;
    }
    if let Some(obj) = profile.as_object_mut() {
        obj.insert(
            "user_background".into(),
            json!({
                "summary": summary.chars().take(260).collect::<String>(),
                "evidence": background.evidence.chars().take(200).collect::<String>(),
                "updated_at": Utc::now().to_rfc3339(),
            }),
        );
        return 1;
    }
    0
}

fn merge_focus_areas(
    profile: &mut Value,
    areas: &[crate::profile::updater::types::PatchFocusArea],
) -> usize {
    let mut added = 0usize;
    for area in areas {
        let name = area.area.trim();
        if name.is_empty() {
            continue;
        }
        let key = name.to_ascii_lowercase();
        let arr = profile.as_object_mut().and_then(|o| {
            if !o.contains_key("focus_areas") {
                o.insert("focus_areas".into(), json!([]));
            }
            o.get_mut("focus_areas").and_then(|v| v.as_array_mut())
        });
        let Some(arr) = arr else {
            continue;
        };
        if arr.iter().any(|e| {
            e.get("area")
                .and_then(|v| v.as_str())
                .map(|v| v.to_ascii_lowercase() == key)
                .unwrap_or(false)
        }) {
            continue;
        }
        arr.push(json!({
            "area": name.chars().take(90).collect::<String>(),
            "weight": area.weight.clamp(0.0, 1.0),
            "evidence": area.evidence.chars().take(200).collect::<String>(),
        }));
        added += 1;
    }
    added
}

fn merge_speech_patterns(
    profile: &mut Value,
    patterns: &[crate::profile::updater::types::PatchSpeechPattern],
) -> usize {
    let mut added = 0usize;
    for p in patterns {
        let pattern = p.pattern.trim();
        if pattern.is_empty() {
            continue;
        }
        let key = pattern.to_ascii_lowercase();
        let arr = profile.as_object_mut().and_then(|o| {
            if !o.contains_key("speech_patterns") {
                o.insert("speech_patterns".into(), json!([]));
            }
            o.get_mut("speech_patterns").and_then(|v| v.as_array_mut())
        });
        let Some(arr) = arr else {
            continue;
        };
        if arr.iter().any(|e| {
            e.get("pattern")
                .and_then(|v| v.as_str())
                .map(|v| v.to_ascii_lowercase() == key)
                .unwrap_or(false)
        }) {
            continue;
        }
        arr.push(json!({
            "pattern": pattern.chars().take(180).collect::<String>(),
            "evidence": p.evidence.chars().take(200).collect::<String>(),
        }));
        added += 1;
    }
    added
}

fn merge_recent_context_notes(
    profile: &mut Value,
    notes: &[crate::profile::updater::types::PatchRecentContext],
) -> usize {
    let mut added = 0usize;
    for n in notes {
        let note = n.note.trim();
        if note.is_empty() {
            continue;
        }
        let entry = format!(
            "{} (evidence: {})",
            note.chars().take(150).collect::<String>(),
            n.evidence.chars().take(120).collect::<String>()
        );
        append_recent_context(profile, &entry);
        added += 1;
    }
    added
}

fn merge_domains(profile: &mut Value, domains: &[crate::profile::updater::types::PatchDomain]) {
    for d in domains {
        let name = d.name.trim();
        if name.is_empty() {
            continue;
        }
        let key = name.to_ascii_lowercase();
        let arr = profile.as_object_mut().and_then(|o| {
            if !o.contains_key("domains") {
                o.insert("domains".into(), json!([]));
            }
            o.get_mut("domains").and_then(|v| v.as_array_mut())
        });
        let Some(arr) = arr else {
            continue;
        };
        if arr.iter().any(|e| {
            e.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.to_ascii_lowercase() == key)
                .unwrap_or(false)
        }) {
            continue;
        }
        arr.push(json!({
            "name": name,
            "weight": d.weight.clamp(0.0, 1.0),
            "evidence": d.evidence.chars().take(200).collect::<String>(),
        }));
    }
}

fn merge_stable_terms(
    profile: &mut Value,
    terms: &[crate::profile::updater::types::PatchStableTerm],
) -> usize {
    let mut added = 0;
    for t in terms {
        let term = t.term.trim();
        if term.is_empty() {
            continue;
        }
        let key = term.to_ascii_lowercase();
        let arr = profile.as_object_mut().and_then(|o| {
            if !o.contains_key("stable_terms") {
                o.insert("stable_terms".into(), json!([]));
            }
            o.get_mut("stable_terms").and_then(|v| v.as_array_mut())
        });
        let Some(arr) = arr else {
            continue;
        };
        if arr.iter().any(|e| {
            e.get("term")
                .and_then(|n| n.as_str())
                .map(|n| n.to_ascii_lowercase() == key)
                .unwrap_or(false)
        }) {
            continue;
        }
        arr.push(json!({
            "term": term,
            "term_type": t.term_type,
            "evidence": t.evidence.chars().take(200).collect::<String>(),
        }));
        added += 1;
    }
    added
}

fn merge_stt_confusions(
    profile: &mut Value,
    confusions: &[crate::profile::updater::types::PatchSttConfusion],
) {
    for c in confusions {
        let heard = normalize_alias_phrase(&c.heard);
        let intended = c.intended.trim();
        if heard.is_empty() || intended.is_empty() {
            continue;
        }
        let arr = profile.as_object_mut().and_then(|o| {
            if !o.contains_key("stt_confusions") {
                o.insert("stt_confusions".into(), json!([]));
            }
            o.get_mut("stt_confusions").and_then(|v| v.as_array_mut())
        });
        let Some(arr) = arr else {
            continue;
        };
        if arr.iter().any(|e| {
            e.get("heard")
                .and_then(|h| h.as_str())
                .map(|h| normalize_alias_phrase(h) == heard)
                .unwrap_or(false)
        }) {
            continue;
        }
        arr.push(json!({
            "heard": heard,
            "intended": intended,
            "evidence": c.evidence.chars().take(200).collect::<String>(),
        }));
    }
}

fn merge_negative_rules(
    profile: &mut Value,
    rules: &[crate::profile::updater::types::PatchNegativeRule],
) {
    for r in rules {
        let rule = r.rule.trim();
        if rule.is_empty() {
            continue;
        }
        let arr = profile.as_object_mut().and_then(|o| {
            if !o.contains_key("negative_rules") {
                o.insert("negative_rules".into(), json!([]));
            }
            o.get_mut("negative_rules").and_then(|v| v.as_array_mut())
        });
        let Some(arr) = arr else {
            continue;
        };
        if arr
            .iter()
            .any(|e| e.get("rule").and_then(|x| x.as_str()) == Some(rule))
        {
            continue;
        }
        arr.push(json!({
            "rule": rule,
            "evidence": r.evidence.chars().take(200).collect::<String>(),
        }));
    }
}

fn merge_style_updates(
    profile: &mut Value,
    updates: &[crate::profile::updater::types::PatchStyleUpdate],
) {
    for u in updates {
        let category = u.category.trim();
        if category.is_empty() {
            continue;
        }
        let key = category.to_ascii_lowercase();
        let arr = profile.as_object_mut().and_then(|o| {
            if !o.contains_key("style") {
                o.insert("style".into(), json!([]));
            }
            o.get_mut("style").and_then(|v| v.as_array_mut())
        });
        let Some(arr) = arr else {
            continue;
        };
        if arr.iter().any(|e| {
            e.get("category")
                .and_then(|c| c.as_str())
                .map(|c| c.to_ascii_lowercase() == key)
                .unwrap_or(false)
        }) {
            continue;
        }
        arr.push(json!({
            "category": category,
            "preference": u.preference,
            "evidence": u.evidence.chars().take(200).collect::<String>(),
        }));
    }
}

fn append_recent_context(profile: &mut Value, reason: &str) {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return;
    }
    let entry = trimmed
        .chars()
        .take(RECENT_CONTEXT_ENTRY_MAX)
        .collect::<String>();
    let arr = profile.as_object_mut().and_then(|o| {
        if !o.contains_key("recent_context") {
            o.insert("recent_context".into(), json!([]));
        }
        o.get_mut("recent_context").and_then(|v| v.as_array_mut())
    });
    let Some(arr) = arr else {
        return;
    };
    arr.push(json!(entry));
    while arr.len() > RECENT_CONTEXT_MAX {
        arr.remove(0);
    }
}

fn merge_alias_proposals(
    profile: &mut Value,
    proposals: &[DeepSeekAliasProposal],
    new_version: i64,
) -> (Vec<AliasChangeRecord>, usize) {
    let mut changes = Vec::new();
    let mut updated = 0usize;

    for proposal in proposals {
        if proposal.proposal_status.eq_ignore_ascii_case("blocked") {
            continue;
        }
        if !ALLOWED_ALIAS_TERM_TYPES
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&proposal.term_type))
        {
            continue;
        }

        let source = proposal.source_phrase.trim();
        let canonical = proposal.canonical_phrase.trim();
        if source.is_empty() || canonical.is_empty() {
            continue;
        }

        let source_norm = normalize_alias_phrase(source);
        let candidate = ProfileAlias {
            source_phrase: source.to_string(),
            canonical_phrase: canonical.to_string(),
            status: ProfileAliasStatus::Candidate,
            confidence: proposal.confidence.clamp(0.0, 1.0),
            evidence_count: proposal.evidence_count_delta.max(0).min(1),
            reason: proposal.reason.chars().take(200).collect(),
            last_seen_at: Some(Utc::now()),
            profile_version: new_version,
        };
        if validate_alias_candidate(&candidate).is_err() {
            continue;
        }

        let aliases_arr = profile.as_object_mut().and_then(|o| {
            if !o.contains_key("aliases") {
                o.insert("aliases".into(), json!([]));
            }
            o.get_mut("aliases").and_then(|v| v.as_array_mut())
        });
        let Some(aliases_arr) = aliases_arr else {
            continue;
        };

        let idx = aliases_arr.iter().position(|e| {
            e.get("source_phrase")
                .and_then(|s| s.as_str())
                .map(normalize_alias_phrase)
                .unwrap_or_default()
                == source_norm
        });

        let from_status = if let Some(i) = idx {
            aliases_arr[i]
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("none")
                .to_string()
        } else {
            "none".to_string()
        };

        let prior_evidence = idx
            .and_then(|i| aliases_arr[i].get("evidence_count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;
        let evidence_count = prior_evidence + proposal.evidence_count_delta.max(0).min(1);

        let to_status = if proposal.proposal_status.eq_ignore_ascii_case("active")
            && proposal.confidence >= 0.82
            && evidence_count >= 2
        {
            "active"
        } else {
            "candidate"
        };

        let entry = json!({
            "source_phrase": source,
            "canonical_phrase": canonical,
            "status": to_status,
            "confidence": proposal.confidence.clamp(0.0, 1.0),
            "evidence_count": evidence_count,
            "reason": proposal.reason.chars().take(200).collect::<String>(),
            "last_seen_at": Utc::now().to_rfc3339(),
            "profile_version": new_version,
        });

        if let Some(i) = idx {
            aliases_arr[i] = entry;
        } else {
            aliases_arr.push(entry);
        }

        changes.push(AliasChangeRecord {
            source_phrase: source.to_string(),
            canonical_phrase: canonical.to_string(),
            from_status,
            to_status: to_status.to_string(),
        });
        updated += 1;
    }

    (changes, updated)
}

fn resolve_markdown(
    current: &str,
    merged_json: &Value,
    patch: &crate::profile::updater::types::DeepSeekMarkdownPatch,
    reasons: &mut Vec<String>,
) -> String {
    let current = sanitize_profile_body(current);
    let mode = patch.mode.as_deref().unwrap_or("null");
    match mode {
        "replace" => {
            let md = patch.markdown.clone().unwrap_or_default();
            let sanitized = sanitize_profile_body(&md);
            if sanitized.len() > PROFILE_MARKDOWN_MAX_BYTES {
                reasons.push("markdown replace exceeds cap".to_string());
                return regenerate_markdown_from_json(merged_json);
            }
            sanitized
        }
        "append_bounded" => {
            let extra = patch.markdown.clone().unwrap_or_default();
            let sanitized_extra = sanitize_profile_body(&extra);
            let combined = sanitize_profile_body(&format!("{current}\n{sanitized_extra}"));
            if combined.len() > PROFILE_MARKDOWN_MAX_BYTES {
                reasons.push("markdown append exceeds cap".to_string());
                return regenerate_markdown_from_json(merged_json);
            }
            combined
        }
        _ => regenerate_markdown_from_json(merged_json),
    }
}

fn sanitize_profile_body(raw: &str) -> String {
    const INSTRUCTION_MARKERS: &[&str] = &[
        "you must",
        "ignore previous",
        "ignore all",
        "disregard",
        "system:",
        "assistant:",
        "override instructions",
        "new instructions",
    ];

    let mut kept = Vec::new();
    let mut byte_len = 0usize;
    for line in raw.trim().lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("user profile context")
            || lower.starts_with("treat this as untrusted context")
            || lower == "do not add content."
            || INSTRUCTION_MARKERS.iter().any(|m| lower.contains(m))
        {
            continue;
        }
        if byte_len + line.len() > PROFILE_MARKDOWN_MAX_BYTES {
            break;
        }
        byte_len += line.len();
        kept.push(line.to_string());
    }
    kept.join("\n")
}

fn regenerate_markdown_from_json(profile: &Value) -> String {
    let mut lines = Vec::new();

    if let Some(summary) = profile
        .get("user_background")
        .and_then(|v| v.get("summary"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        lines.push(format!("Background: {}", summary.trim()));
    }

    if let Some(areas) = profile.get("focus_areas").and_then(|v| v.as_array()) {
        let mut sorted = areas
            .iter()
            .filter_map(|a| {
                Some((
                    a.get("area")?.as_str()?,
                    a.get("weight").and_then(|v| v.as_f64()).unwrap_or(0.5),
                ))
            })
            .collect::<Vec<_>>();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let names: Vec<_> = sorted.into_iter().take(8).map(|(name, _)| name).collect();
        if !names.is_empty() {
            lines.push(format!("Focus areas: {}", names.join(", ")));
        }
    }

    if let Some(domains) = profile.get("domains").and_then(|v| v.as_array()) {
        let names: Vec<_> = domains
            .iter()
            .filter_map(|d| d.get("name").and_then(|n| n.as_str()))
            .take(8)
            .collect();
        if !names.is_empty() {
            lines.push(format!("Domains: {}", names.join(", ")));
        }
    }

    if let Some(patterns) = profile.get("speech_patterns").and_then(|v| v.as_array()) {
        let p: Vec<_> = patterns
            .iter()
            .filter_map(|d| d.get("pattern").and_then(|n| n.as_str()))
            .take(3)
            .collect();
        if !p.is_empty() {
            lines.push(format!("Speech style: {}", p.join("; ")));
        }
    }

    if let Some(terms) = profile.get("stable_terms").and_then(|v| v.as_array()) {
        let t: Vec<_> = terms
            .iter()
            .filter_map(|d| d.get("term").and_then(|n| n.as_str()))
            .take(18)
            .collect();
        if !t.is_empty() {
            lines.push(format!("Stable vocabulary: {}", t.join(", ")));
        }
    }

    let mut recoveries = Vec::new();
    if let Some(conf) = profile.get("stt_confusions").and_then(|v| v.as_array()) {
        for c in conf.iter().take(6) {
            if let (Some(h), Some(i)) = (
                c.get("heard").and_then(|v| v.as_str()),
                c.get("intended").and_then(|v| v.as_str()),
            ) {
                recoveries.push(format!("{h} → {i}"));
            }
        }
    }
    if let Some(aliases) = profile.get("aliases").and_then(|v| v.as_array()) {
        for a in aliases.iter().take(8) {
            if let (Some(h), Some(i)) = (
                a.get("source_phrase").and_then(|v| v.as_str()),
                a.get("canonical_phrase").and_then(|v| v.as_str()),
            ) {
                let pair = format!("{h} → {i}");
                if !recoveries.iter().any(|r| r == &pair) {
                    recoveries.push(pair);
                }
            }
        }
    }
    if !recoveries.is_empty() {
        lines.push(format!(
            "STT recovery: {}",
            recoveries
                .into_iter()
                .take(10)
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    if let Some(style) = profile.get("style").and_then(|v| v.as_array()) {
        let s: Vec<_> = style
            .iter()
            .filter_map(|d| {
                Some(format!(
                    "{}: {}",
                    d.get("category")?.as_str()?,
                    d.get("preference")?.as_str()?
                ))
            })
            .take(4)
            .collect();
        if !s.is_empty() {
            lines.push(format!("Style preferences: {}", s.join("; ")));
        }
    }

    if let Some(recent) = profile.get("recent_context").and_then(|v| v.as_array()) {
        let r: Vec<_> = recent
            .iter()
            .filter_map(|v| v.as_str())
            .rev()
            .take(3)
            .collect();
        if !r.is_empty() {
            lines.push(format!("Recent context: {}", r.join("; ")));
        }
    }

    let md = lines.join("\n");
    sanitize_profile_body(&md)
        .chars()
        .take(PROFILE_MARKDOWN_MAX_BYTES)
        .collect()
}

fn instruction_like_markdown(md: &str) -> bool {
    let lower = md.to_lowercase();
    [
        "you must",
        "ignore previous",
        "ignore all",
        "system:",
        "assistant:",
        "disregard",
        "override",
    ]
    .iter()
    .any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::updater::types::{
        DeepSeekClassification, DeepSeekMarkdownPatch, DeepSeekProfilePatch,
        DeepSeekProfileUpdateResponse, PatchFocusArea, PatchSpeechPattern, PatchUserBackground,
    };

    fn base_input(ds: DeepSeekProfileUpdateResponse, apply: bool) -> ValidatorInput {
        ValidatorInput {
            current_json: json!({}),
            current_markdown: String::new(),
            current_version: 1,
            deepseek: ds,
            update_mode_apply: apply,
            request_id: Uuid::new_v4(),
            edit_event_id: "evt-1".into(),
            recording_id: None,
            client_run_id: None,
            run_id: None,
            latency_ms: 10,
        }
    }

    #[test]
    fn rejects_no_learning_classification() {
        let ds = DeepSeekProfileUpdateResponse {
            schema_version: 1,
            classification: DeepSeekClassification::NoLearning,
            confidence: 0.1,
            profile_patch: DeepSeekProfilePatch::default(),
            alias_proposals: vec![],
            profile_markdown_patch: DeepSeekMarkdownPatch::default(),
            review_required: false,
            reason: "ambiguous".into(),
        };
        let out = validate_and_merge(base_input(ds, true));
        assert_eq!(out.decision, ValidatorDecision::Rejected);
    }

    #[test]
    fn shadow_mode_does_not_mark_applied() {
        let ds = DeepSeekProfileUpdateResponse {
            schema_version: 1,
            classification: DeepSeekClassification::SttError,
            confidence: 0.9,
            profile_patch: DeepSeekProfilePatch {
                add_stable_terms: vec![crate::profile::updater::types::PatchStableTerm {
                    term: "n8n".into(),
                    term_type: "brand".into(),
                    evidence: "user corrected".into(),
                }],
                ..Default::default()
            },
            alias_proposals: vec![DeepSeekAliasProposal {
                source_phrase: "n 10".into(),
                canonical_phrase: "n8n".into(),
                term_type: "code_identifier".into(),
                proposal_status: "candidate".into(),
                confidence: 0.9,
                evidence_count_delta: 1,
                reason: "automation context".into(),
            }],
            profile_markdown_patch: DeepSeekMarkdownPatch::default(),
            review_required: false,
            reason: "stt fix".into(),
        };
        let out = validate_and_merge(base_input(ds, false));
        assert_eq!(out.decision, ValidatorDecision::Shadow);
        assert!(out.merged_json.get("aliases").is_some());
    }

    #[test]
    fn rejects_common_word_alias() {
        let ds = DeepSeekProfileUpdateResponse {
            schema_version: 1,
            classification: DeepSeekClassification::SttError,
            confidence: 0.9,
            profile_patch: DeepSeekProfilePatch::default(),
            alias_proposals: vec![DeepSeekAliasProposal {
                source_phrase: "kaam".into(),
                canonical_phrase: "Kafka".into(),
                term_type: "brand".into(),
                proposal_status: "candidate".into(),
                confidence: 0.95,
                evidence_count_delta: 1,
                reason: "bad".into(),
            }],
            profile_markdown_patch: DeepSeekMarkdownPatch::default(),
            review_required: false,
            reason: "stt".into(),
        };
        let out = validate_and_merge(base_input(ds, true));
        assert_eq!(out.decision, ValidatorDecision::Rejected);
    }

    #[test]
    fn rich_profile_sections_render_into_prompt_markdown() {
        let ds = DeepSeekProfileUpdateResponse {
            schema_version: 1,
            classification: DeepSeekClassification::DomainTerm,
            confidence: 0.9,
            profile_patch: DeepSeekProfilePatch {
                user_background: Some(PatchUserBackground {
                    summary:
                        "User appears to be a developer-business operator shipping software and client automations."
                            .into(),
                    evidence: "mentions PR, Docker, client update".into(),
                }),
                add_focus_areas: vec![PatchFocusArea {
                    area: "software releases and business operations".into(),
                    weight: 0.9,
                    evidence: "PR plus client update context".into(),
                }],
                add_speech_patterns: vec![PatchSpeechPattern {
                    pattern:
                        "Mixes Hinglish with developer and business English; wants natural work-ready text."
                            .into(),
                    evidence: "Hinglish dictation".into(),
                }],
                add_stable_terms: vec![crate::profile::updater::types::PatchStableTerm {
                    term: "Docker".into(),
                    term_type: "brand".into(),
                    evidence: "user kept Docker".into(),
                }],
                add_stt_confusions: vec![crate::profile::updater::types::PatchSttConfusion {
                    heard: "doctor rebuild".into(),
                    intended: "Docker rebuild".into(),
                    evidence: "user corrected Doctor rebuild to Docker rebuild".into(),
                }],
                ..Default::default()
            },
            alias_proposals: vec![],
            profile_markdown_patch: DeepSeekMarkdownPatch::default(),
            review_required: true,
            reason: "developer/business profile update".into(),
        };
        let out = validate_and_merge(base_input(ds, true));
        assert_eq!(out.decision, ValidatorDecision::Applied);
        assert!(out.merged_markdown.contains("Background:"));
        assert!(out.merged_markdown.contains("Focus areas:"));
        assert!(out.merged_markdown.contains("Speech style:"));
        assert!(out.merged_markdown.contains("Stable vocabulary: Docker"));
        assert!(
            out.merged_markdown
                .contains("doctor rebuild → Docker rebuild")
        );
        assert!(!out.merged_markdown.contains("USER PROFILE CONTEXT"));
    }
}
