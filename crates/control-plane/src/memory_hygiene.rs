//! Profile-level memory hygiene — DeepSeek batch review of personal vocab/aliases.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HygieneActionKind {
    CommonBlock,
    Penalize,
    Demote,
    MergeCluster,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HygieneAction {
    pub action: HygieneActionKind,
    #[serde(default)]
    pub heard: Option<String>,
    #[serde(default)]
    pub correct: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub cluster_id: Option<String>,
}

pub async fn mark_memory_dirty(db: &PgPool, account_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO personal_memory_hygiene_state (account_id, memory_dirty_at, updated_at)
         VALUES ($1, now(), now())
         ON CONFLICT (account_id) DO UPDATE SET
             memory_dirty_at = now(),
             updated_at = now()",
    )
    .bind(account_id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn fetch_dirty_accounts(db: &PgPool, limit: i64) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT account_id
           FROM personal_memory_hygiene_state
          WHERE memory_dirty_at IS NOT NULL
            AND memory_dirty_at < now() - INTERVAL '30 minutes'
          ORDER BY memory_dirty_at ASC
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn build_profile_snapshot(db: &PgPool, account_id: Uuid) -> Result<Value, sqlx::Error> {
    let vocab: Vec<(String, String, f64, i32, i32, String)> = sqlx::query_as(
        "SELECT term, term_type, weight, positive_count, negative_count, status
           FROM personal_vocab_terms
          WHERE account_id = $1 AND status = 'active'
          ORDER BY positive_count DESC, weight DESC
          LIMIT 200",
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;

    let aliases: Vec<(String, String, f64, i32, String, Option<String>)> = sqlx::query_as(
        "SELECT transcript_form, correct_form, weight, positive_count, safety_status,
                learned_stt_provider
           FROM personal_stt_replacements
          WHERE account_id = $1 AND status = 'active'
          ORDER BY positive_count DESC, weight DESC
          LIMIT 200",
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;

    let policies: Vec<(String, String, String, i32, i32, String)> = sqlx::query_as(
        "SELECT variant_form, correct_form, edit_type, positive_count, negative_count, status
           FROM personal_edit_policy_rules
          WHERE account_id = $1 AND status IN ('active', 'candidate')
          ORDER BY positive_count DESC
          LIMIT 200",
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;

    Ok(json!({
        "vocab_terms": vocab.iter().map(|(term, term_type, weight, pos, neg, status)| {
            json!({
                "term": term,
                "term_type": term_type,
                "weight": weight,
                "positive_count": pos,
                "negative_count": neg,
                "status": status,
            })
        }).collect::<Vec<_>>(),
        "aliases": aliases.iter().map(|(heard, correct, weight, pos, safety, stt)| {
            json!({
                "heard": heard,
                "correct": correct,
                "weight": weight,
                "positive_count": pos,
                "safety_status": safety,
                "learned_stt_provider": stt,
            })
        }).collect::<Vec<_>>(),
        "edit_policies": policies.iter().map(|(variant, correct, edit_type, pos, neg, status)| {
            json!({
                "variant": variant,
                "correct": correct,
                "edit_type": edit_type,
                "positive_count": pos,
                "negative_count": neg,
                "status": status,
            })
        }).collect::<Vec<_>>(),
    }))
}

const HYGIENE_SYSTEM_PROMPT: &str = r#"You are a memory hygiene reviewer for a voice dictation app.
You receive a user's learned vocab terms, STT aliases (heard→correct), and edit-policy rules.
Identify false-positive aliases (common English words misheard as jargon), phonetic clusters that should merge,
over-weighted vocab, and edit policies that should be penalized.

Return ONLY valid JSON:
{"actions":[{"action":"common_block","heard":"mec","correct":"EMIAC","reason":"common substring"},
{"action":"penalize","heard":"mea","correct":"EMIAC","reason":"low confidence cluster"},
{"action":"demote","heard":null,"correct":null,"reason":"stale term","cluster_id":null},
{"action":"merge_cluster","heard":"mia","correct":"EMIAC","reason":"phonetic cluster","cluster_id":"EMIAC"}]}

Allowed actions: common_block, penalize, demote, merge_cluster.
For demote include "heard" as the vocab term to demote.
Do not invent pairs not present in the snapshot. Be conservative."#;

pub fn parse_hygiene_actions(raw: &str) -> Vec<HygieneAction> {
    let trimmed = raw.trim();
    let json_str = if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            &trimmed[start..=end]
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    let value: Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            warn!("[memory-hygiene] parse failed: {e}");
            return Vec::new();
        }
    };

    value
        .get("actions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

pub async fn call_deepseek_memory_hygiene(
    snapshot: &Value,
) -> Result<(String, Vec<HygieneAction>), String> {
    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .map_err(|_| "DEEPSEEK_API_KEY is not configured".to_string())?;
    let model =
        std::env::var("DEEPSEEK_HYGIENE_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".to_string());
    let base = std::env::var("DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string());
    let url = format!("{}/v1/chat/completions", base.trim_end_matches('/'));
    let user_message = serde_json::to_string_pretty(snapshot)
        .map_err(|e| format!("snapshot serialize failed: {e}"))?;

    let body = json!({
        "model": model,
        "temperature": 0.0,
        "max_tokens": 2048,
        "stream": false,
        "thinking": { "type": "disabled" },
        "messages": [
            { "role": "system", "content": HYGIENE_SYSTEM_PROMPT },
            { "role": "user", "content": user_message }
        ]
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .timeout(std::time::Duration::from_secs(90))
        .send()
        .await
        .map_err(|e| format!("DeepSeek request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let preview = resp.text().await.unwrap_or_default();
        return Err(format!(
            "DeepSeek HTTP {status}: {}",
            &preview[..preview.len().min(200)]
        ));
    }

    let value: Value = resp
        .json()
        .await
        .map_err(|e| format!("DeepSeek response parse failed: {e}"))?;
    let output = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    let actions = parse_hygiene_actions(&output);
    Ok((model, actions))
}

fn norm_key(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

pub async fn apply_hygiene_action(
    db: &PgPool,
    account_id: Uuid,
    org_id: Option<Uuid>,
    model: &str,
    action: &HygieneAction,
) -> Result<(), sqlx::Error> {
    let verdict = format!("{:?}", action.action);
    match action.action {
        HygieneActionKind::CommonBlock => {
            let heard = action.heard.as_deref().unwrap_or_default();
            let correct = action.correct.as_deref().unwrap_or_default();
            if heard.is_empty() || correct.is_empty() {
                return Ok(());
            }
            let hn = norm_key(heard);
            let cn = norm_key(correct);
            sqlx::query(
                "UPDATE personal_stt_replacements
                    SET safety_status = 'common_block', status = 'blocked', updated_at = now()
                  WHERE account_id = $1 AND transcript_norm = $2 AND correct_norm = $3",
            )
            .bind(account_id)
            .bind(&hn)
            .bind(&cn)
            .execute(db)
            .await?;
            insert_audit(
                db,
                account_id,
                org_id,
                "common_block",
                heard,
                correct,
                &verdict,
                &action.reason,
                model,
            )
            .await?;
        }
        HygieneActionKind::Penalize => {
            let heard = action.heard.as_deref().unwrap_or_default();
            let correct = action.correct.as_deref().unwrap_or_default();
            if heard.is_empty() || correct.is_empty() {
                return Ok(());
            }
            let vn = norm_key(heard);
            let cn = norm_key(correct);
            sqlx::query(
                "UPDATE personal_edit_policy_rules
                    SET negative_count = negative_count + 1,
                        status = CASE WHEN negative_count + 1 >= 3 THEN 'candidate' ELSE status END,
                        updated_at = now()
                  WHERE account_id = $1 AND variant_norm = $2 AND correct_norm = $3",
            )
            .bind(account_id)
            .bind(&vn)
            .bind(&cn)
            .execute(db)
            .await?;
            insert_audit(
                db,
                account_id,
                org_id,
                "penalize",
                heard,
                correct,
                &verdict,
                &action.reason,
                model,
            )
            .await?;
        }
        HygieneActionKind::Demote => {
            let term = action
                .heard
                .as_deref()
                .or(action.correct.as_deref())
                .unwrap_or_default();
            if term.is_empty() {
                return Ok(());
            }
            let tn = norm_key(term);
            sqlx::query(
                "UPDATE personal_vocab_terms
                    SET weight = GREATEST(0.1, weight * 0.5), updated_at = now()
                  WHERE account_id = $1 AND term_norm = $2",
            )
            .bind(account_id)
            .bind(&tn)
            .execute(db)
            .await?;
            insert_audit(
                db,
                account_id,
                org_id,
                "demote",
                term,
                "",
                &verdict,
                &action.reason,
                model,
            )
            .await?;
        }
        HygieneActionKind::MergeCluster => {
            let heard = action.heard.as_deref().unwrap_or_default();
            let correct = action.correct.as_deref().unwrap_or_default();
            if heard.is_empty() || correct.is_empty() {
                return Ok(());
            }
            let hn = norm_key(heard);
            let cn = norm_key(correct);
            sqlx::query(
                "UPDATE personal_stt_replacements
                    SET safety_status = 'common_block', status = 'blocked', updated_at = now()
                  WHERE account_id = $1 AND transcript_norm = $2 AND correct_norm = $3",
            )
            .bind(account_id)
            .bind(&hn)
            .bind(&cn)
            .execute(db)
            .await?;
            insert_audit(
                db,
                account_id,
                org_id,
                "merge_cluster",
                heard,
                correct,
                &verdict,
                &action.reason,
                model,
            )
            .await?;
        }
    }
    Ok(())
}

async fn insert_audit(
    db: &PgPool,
    account_id: Uuid,
    org_id: Option<Uuid>,
    action: &str,
    heard: &str,
    correct: &str,
    verdict: &str,
    reason: &str,
    model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO alias_safety_audit
             (account_id, org_id, action, target_type, heard, correct, verdict, reason, model)
         VALUES ($1, $2, $3, 'alias', $4, $5, $6, $7, $8)",
    )
    .bind(account_id)
    .bind(org_id)
    .bind(action)
    .bind(heard)
    .bind(correct)
    .bind(verdict)
    .bind(reason)
    .bind(model)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn clear_dirty_flag(db: &PgPool, account_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE personal_memory_hygiene_state
            SET memory_dirty_at = NULL, last_hygiene_at = now(), updated_at = now()
          WHERE account_id = $1",
    )
    .bind(account_id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn process_account_hygiene(db: &PgPool, account_id: Uuid) -> Result<usize, String> {
    let org_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT org_id FROM org_members WHERE account_id = $1 ORDER BY joined_at ASC LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(db)
    .await
    .map_err(|e| e.to_string())?;

    let snapshot = build_profile_snapshot(db, account_id)
        .await
        .map_err(|e| e.to_string())?;
    if snapshot
        .get("aliases")
        .and_then(Value::as_array)
        .is_none_or(|a| a.is_empty())
        && snapshot
            .get("vocab_terms")
            .and_then(Value::as_array)
            .is_none_or(|a| a.is_empty())
    {
        clear_dirty_flag(db, account_id)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(0);
    }

    let (model, actions) = call_deepseek_memory_hygiene(&snapshot).await?;
    let mut applied = 0usize;
    for action in &actions {
        if apply_hygiene_action(db, account_id, org_id, &model, action)
            .await
            .is_ok()
        {
            applied += 1;
        }
    }
    clear_dirty_flag(db, account_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(applied)
}

#[derive(sqlx::FromRow)]
pub struct HygieneStateRow {
    pub memory_dirty_at: Option<DateTime<Utc>>,
    pub last_hygiene_at: Option<DateTime<Utc>>,
    pub hygiene_version: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_actions_array() {
        let raw = r#"{"actions":[{"action":"common_block","heard":"mec","correct":"EMIAC","reason":"test"}]}"#;
        let actions = parse_hygiene_actions(raw);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].action, HygieneActionKind::CommonBlock);
        assert_eq!(actions[0].heard.as_deref(), Some("mec"));
    }
}
