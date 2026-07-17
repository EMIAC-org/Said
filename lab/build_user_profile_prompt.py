#!/usr/bin/env python3
"""Build a personal AirNote voice-polish prompt from local history.

The script is intentionally lab-only:
1. Read local AirNote SQLite history with a hard token budget.
2. Ask DeepSeek for a compact structured user profile.
3. Render a generic prompt template with that profile.
4. Optionally apply the rendered prompt to the local active prompt_templates row.
5. Optionally smoke-test the rendered prompt on the cached lab transcript.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import time
import urllib.error
import urllib.request
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import polish_lab
import run_prompt_matrix

LAB = Path(__file__).resolve().parent
REPO = LAB.parent
PROFILE_TEMPLATE = LAB / "profile_prompt_template.md"
OUT_ROOT = LAB / "user_profiles"
DEFAULT_DB = Path.home() / "Library/Application Support/VoicePolish/db.sqlite"
DEFAULT_USER_SEEDS = [
    "core finance",
    "SEO off-page",
    "SEO on-page",
    "Google Ads",
    "Meta Ads",
    "business operations",
    "inventory management",
    "latest AI development",
    "software engineering",
    "startup/product work",
]

CANONICAL_TEXT_REPLACEMENTS = [
    (re.compile(r"\bkaafka\b", re.I), "Kafka"),
    (re.compile(r"\bkaaf\s*ka\b", re.I), "Kafka"),
    (re.compile(r"\bzukeeper\b", re.I), "ZooKeeper"),
    (re.compile(r"\bzoo keeper\b", re.I), "ZooKeeper"),
    (re.compile(r"\bcqlite\b", re.I), "SQLite"),
    (re.compile(r"\bwebbook\b", re.I), "webhook"),
    (re.compile(r"\bdoctor rebuild\b", re.I), "Docker rebuild"),
    (re.compile(r"\bdeep infra\b", re.I), "DeepInfra"),
    (re.compile(r"\bdeep braahm\b", re.I), "Local speech"),
    (re.compile(r"\bcentury mein run ID\b", re.I), "Sentry mein run ID"),
]


def approx_tokens(text: str) -> int:
    return max(1, len(text) // 4)


def truncate(text: str | None, max_chars: int) -> str:
    text = (text or "").strip()
    if len(text) <= max_chars:
        return text
    return text[: max_chars - 1].rstrip() + "..."


def rows(conn: sqlite3.Connection, query: str, params: tuple[Any, ...] = ()) -> list[dict[str, Any]]:
    conn.row_factory = sqlite3.Row
    return [dict(row) for row in conn.execute(query, params).fetchall()]


def table_exists(conn: sqlite3.Connection, table: str) -> bool:
    return (
        conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
            (table,),
        ).fetchone()
        is not None
    )


def table_columns(conn: sqlite3.Connection, table: str) -> set[str]:
    return {str(row[1]) for row in conn.execute(f"PRAGMA table_info({table})").fetchall()}


def first_user_id(conn: sqlite3.Connection) -> str:
    row = conn.execute("SELECT id FROM local_user ORDER BY created_at LIMIT 1").fetchone()
    if not row:
        raise SystemExit("No local_user row found in AirNote DB.")
    return str(row[0])


def build_history_snapshot(
    *,
    db_path: Path,
    user_id: str | None,
    history_token_budget: int,
    max_recordings: int,
    max_edits: int,
    seed_focus: list[str],
) -> tuple[dict[str, Any], str]:
    conn = sqlite3.connect(str(db_path))
    try:
        user_id = user_id or first_user_id(conn)
        user = rows(conn, "SELECT id, email, license_tier, created_at FROM local_user WHERE id=?", (user_id,))
        prefs = rows(
            conn,
            "SELECT selected_model, tone_preset, output_language, learning_enabled, speech_model FROM preferences WHERE user_id=?",
            (user_id,),
        )
        edits = rows(
            conn,
            """
            SELECT timestamp_ms, transcript, ai_output, user_kept, target_app, edit_class, learning_kind
            FROM edit_events
            WHERE user_id=?
            ORDER BY timestamp_ms DESC
            LIMIT ?
            """,
            (user_id, max_edits),
        )
        recordings = rows(
            conn,
            """
            SELECT timestamp_ms, transcript, raw_transcript, local_corrected_transcript,
                   polished, polished_output, final_text, target_app, source, model_used,
                   word_count, edit_count
            FROM recordings
            WHERE user_id=?
            ORDER BY timestamp_ms DESC
            LIMIT ?
            """,
            (user_id, max_recordings),
        )
        vocab: list[dict[str, Any]] = []
        if table_exists(conn, "vocabulary"):
            vocab_cols = table_columns(conn, "vocabulary")
            vocab_order = "updated_at" if "updated_at" in vocab_cols else "last_used"
            vocab = rows(
                conn,
                f"SELECT term, term_type, meaning, example_context FROM vocabulary WHERE user_id=? ORDER BY {vocab_order} DESC LIMIT 200",
                (user_id,),
            )
        replacements: list[dict[str, Any]] = []
        if table_exists(conn, "stt_replacements"):
            repl_cols = table_columns(conn, "stt_replacements")
            repl_order = "updated_at" if "updated_at" in repl_cols else "last_used"
            if {"heard", "correct"}.issubset(repl_cols):
                replacements = rows(
                    conn,
                    f"SELECT heard, correct, context_hint, status FROM stt_replacements WHERE user_id=? ORDER BY {repl_order} DESC LIMIT 200",
                    (user_id,),
                )
            elif {"transcript_form", "correct_form"}.issubset(repl_cols):
                replacements = rows(
                    conn,
                    f"SELECT transcript_form AS heard, correct_form AS correct, review_reason AS context_hint, review_status AS status FROM stt_replacements WHERE user_id=? ORDER BY {repl_order} DESC LIMIT 200",
                    (user_id,),
                )

        snapshot: dict[str, Any] = {
            "source": "AirNote local SQLite history",
            "created_at": datetime.now(timezone.utc).isoformat(),
            "db_path": str(db_path),
            "user": user[0] if user else {"id": user_id},
            "preferences": prefs[0] if prefs else {},
            "seed_focus_from_user": seed_focus,
            "counts": {
                "recordings_total": conn.execute(
                    "SELECT COUNT(*) FROM recordings WHERE user_id=?", (user_id,)
                ).fetchone()[0],
                "edit_events_total": conn.execute(
                    "SELECT COUNT(*) FROM edit_events WHERE user_id=?", (user_id,)
                ).fetchone()[0],
                "vocabulary_total": conn.execute(
                    "SELECT COUNT(*) FROM vocabulary WHERE user_id=?", (user_id,)
                ).fetchone()[0]
                if table_exists(conn, "vocabulary")
                else 0,
                "stt_replacements_total": conn.execute(
                    "SELECT COUNT(*) FROM stt_replacements WHERE user_id=?", (user_id,)
                ).fetchone()[0]
                if table_exists(conn, "stt_replacements")
                else 0,
            },
            "edit_events": edits,
            "recordings": recordings,
            "vocabulary": vocab,
            "stt_replacements": replacements,
        }
    finally:
        conn.close()

    lines = [
        "# AirNote user history snapshot",
        "",
        "## User seed focus areas",
        ", ".join(seed_focus),
        "",
        "## Counts",
        json.dumps(snapshot["counts"], ensure_ascii=False),
        "",
    ]

    budget = history_token_budget

    def add_block(title: str, body: str) -> bool:
        nonlocal budget
        token_cost = approx_tokens(body)
        if token_cost > budget:
            return False
        lines.extend([title, body, ""])
        budget -= token_cost
        return True

    if edits:
        add_block("## Explicit edit events (highest signal)", "")
        for idx, edit in enumerate(edits, start=1):
            body = "\n".join(
                [
                    f"Edit {idx} target_app={edit.get('target_app') or '-'} class={edit.get('edit_class') or '-'}",
                    f"Raw transcript: {truncate(edit.get('transcript'), 700)}",
                    f"AI output: {truncate(edit.get('ai_output'), 700)}",
                    f"User kept: {truncate(edit.get('user_kept'), 900)}",
                ]
            )
            if not add_block("", body):
                break

    if recordings:
        add_block("## Recent recordings", "")
        for idx, rec in enumerate(recordings, start=1):
            raw = rec.get("raw_transcript") or rec.get("local_corrected_transcript") or rec.get("transcript")
            final = rec.get("final_text") or rec.get("polished_output") or rec.get("polished")
            body = "\n".join(
                [
                    f"Recording {idx} target_app={rec.get('target_app') or '-'} source={rec.get('source') or '-'} edits={rec.get('edit_count')}",
                    f"Raw: {truncate(raw, 600)}",
                    f"Final: {truncate(final, 800)}",
                ]
            )
            if not add_block("", body):
                break

    if vocab:
        add_block("## Existing vocabulary rows", "")
        for item in vocab:
            body = json.dumps(item, ensure_ascii=False)
            if not add_block("", body):
                break

    if replacements:
        add_block("## Existing STT replacements", "")
        for item in replacements:
            body = json.dumps(item, ensure_ascii=False)
            if not add_block("", body):
                break

    return snapshot, "\n".join(lines).strip() + "\n"


PROFILE_SYSTEM_PROMPT = """You build compact user profiles for a voice dictation polish system.

Return JSON only. Do not include markdown fences.

Goal: infer a reusable profile that helps an LLM recover likely intended terms from noisy STT for this specific user, without adding facts or over-rewriting.

The profile must be useful for future STT cleanup, not just the provided examples.

Rules:
- Use the user's explicit seed focus areas as priors, but verify/augment from history.
- Separate domains, terms, phrase recoveries, and style rules.
- Favor high-signal terms and realistic STT garble patterns.
- Include finance, SEO, ads, business, inventory, and latest-AI/dev areas if supported or explicitly seeded.
- Keep it compact. The rendered prompt focus block should fit under roughly 900 tokens.
- Do not include secrets, API keys, private tokens, long personal data, or full transcript dumps.
- Use Roman Hinglish examples where helpful.
- Hard output caps: max 6 domains, max 35 term_bank entries, max 16 phrase_recoveries, max 8 style_rules, max 8 guardrails.
- Keep prompt_focus_block under 1500 characters and the full JSON response under 4500 characters.
- Prefer fewer, higher-confidence entries over a long noisy list.
- Normalize obvious technical STT garbles to canonical spellings when context supports them:
  SQLite not CQLite, Sentry not century/centuries, Caps Lock not app slot/cabslog,
  Docker not doctor, webhook not webbook, DeepInfra not deep infra, Local speech not deep braahm,
  ZooKeeper not zuki/zukeeper, Kafka not kaaf/kaafka.
- Phrase recoveries must map noisy heard text to a cleaner intended phrase. Do not include no-op mappings.

JSON schema:
{
  "version": "airnote-user-profile-v1",
  "profile_name": "short name",
  "summary": "2-3 sentences",
  "domains": [
    {"name": "domain", "priority": 1-5, "cues": ["words"], "preferred_terms": ["terms"]}
  ],
  "term_bank": [
    {"term": "canonical term", "domain": "domain", "aliases_or_garbles": ["possible garbles"], "priority": 1-5}
  ],
  "phrase_recoveries": [
    {"heard": "likely STT phrase", "intended": "canonical phrase", "domain": "domain", "confidence": "high|medium|low"}
  ],
  "style_rules": ["short rules"],
  "guardrails": ["short rules"],
  "prompt_focus_block": "compact plain-text block to embed in a system prompt"
}
"""

PROFILE_REPAIR_PROMPT = """Repair or compress the provided profile into valid JSON only.

Use the same schema as requested. Do not include markdown fences.
Hard caps: max 6 domains, max 35 term_bank entries, max 16 phrase_recoveries, prompt_focus_block under 1500 characters.
Drop incomplete, duplicate, low-confidence, or noisy entries. Preserve the strongest profile facts.
"""


def load_dotenv_no_override() -> None:
    polish_lab.load_dotenv()


def call_deepseek_profile(history_text: str, max_tokens: int) -> dict[str, Any]:
    api_key = os.getenv("DEEPSEEK_API_KEY", "").strip()
    if not api_key:
        raise SystemExit("DEEPSEEK_API_KEY is not set in .env or environment.")
    model = os.getenv("DEEPSEEK_PROFILE_UPDATE_MODEL", "").strip() or "deepseek-v4-flash"
    base = os.getenv("DEEPSEEK_BASE_URL", "").strip() or "https://api.deepseek.com"
    url = f"{base.rstrip('/')}/v1/chat/completions"

    def request_profile(messages: list[dict[str, str]], token_cap: int) -> str:
        body = {
            "model": model,
            "temperature": 0.0,
            "max_tokens": token_cap,
            "stream": False,
            "thinking": {"type": "disabled"},
            "response_format": {"type": "json_object"},
            "messages": messages,
        }
        req = urllib.request.Request(
            url,
            data=json.dumps(body).encode("utf-8"),
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=120) as resp:
                payload = json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"DeepSeek profile request failed HTTP {exc.code}: {detail}") from exc
        content = (
            payload.get("choices", [{}])[0]
            .get("message", {})
            .get("content", "")
            .strip()
        )
        if not content:
            raise RuntimeError(f"DeepSeek profile response was empty: {payload}")
        return content

    content = request_profile(
        [
            {"role": "system", "content": PROFILE_SYSTEM_PROMPT},
            {"role": "user", "content": history_text},
        ],
        max_tokens,
    )
    try:
        return parse_profile_json(content)
    except json.JSONDecodeError:
        repaired = request_profile(
            [
                {"role": "system", "content": PROFILE_REPAIR_PROMPT},
                {"role": "user", "content": content},
            ],
            min(max(max_tokens, 3000), 5000),
        )
        return parse_profile_json(repaired)


def parse_profile_json(content: str) -> dict[str, Any]:
    try:
        return json.loads(content)
    except json.JSONDecodeError:
        match = re.search(r"\{.*\}", content, flags=re.S)
        if not match:
            raise
        return json.loads(match.group(0))


def clamp_profile(profile: dict[str, Any]) -> dict[str, Any]:
    profile["domains"] = (profile.get("domains") or [])[:6]
    profile["term_bank"] = (profile.get("term_bank") or [])[:35]
    profile["phrase_recoveries"] = clean_phrase_recoveries(profile.get("phrase_recoveries") or [])[:16]
    profile["style_rules"] = (profile.get("style_rules") or [])[:8]
    profile["guardrails"] = (profile.get("guardrails") or [])[:8]
    focus = str(profile.get("prompt_focus_block") or "").strip()
    if focus:
        profile["prompt_focus_block"] = truncate(focus, 1500)
    return profile


def normalize_canonical_text(text: str) -> str:
    normalized = text
    for pattern, replacement in CANONICAL_TEXT_REPLACEMENTS:
        normalized = pattern.sub(replacement, normalized)
    return normalized


def clean_phrase_recoveries(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
    cleaned: list[dict[str, Any]] = []
    seen: set[tuple[str, str]] = set()
    for item in items:
        heard = str(item.get("heard") or "").strip()
        intended = normalize_canonical_text(str(item.get("intended") or "").strip())
        if not heard or not intended:
            continue
        if heard.casefold() == intended.casefold():
            continue
        key = (heard.casefold(), intended.casefold())
        if key in seen:
            continue
        seen.add(key)
        next_item = dict(item)
        next_item["heard"] = heard
        next_item["intended"] = intended
        cleaned.append(next_item)
    return cleaned


def profile_to_block(profile: dict[str, Any], max_chars: int = 4200) -> str:
    explicit = str(profile.get("prompt_focus_block") or "").strip()
    if explicit:
        return truncate(explicit, max_chars)

    lines = [str(profile.get("summary") or "").strip()]
    domains = profile.get("domains") or []
    if domains:
        lines.append("Focus domains:")
        for item in domains[:8]:
            terms = ", ".join((item.get("preferred_terms") or [])[:8])
            lines.append(f"- {item.get('name')}: {terms}")
    terms = profile.get("term_bank") or []
    if terms:
        lines.append("High-signal terms:")
        lines.append(", ".join(str(item.get("term")) for item in terms[:40] if item.get("term")))
    recoveries = profile.get("phrase_recoveries") or []
    if recoveries:
        lines.append("Likely phrase recoveries:")
        for item in recoveries[:16]:
            lines.append(f"- {item.get('heard')} -> {item.get('intended')}")
    rules = profile.get("style_rules") or []
    if rules:
        lines.append("Style:")
        for rule in rules[:8]:
            lines.append(f"- {rule}")
    return truncate("\n".join(line for line in lines if line), max_chars)


def render_personal_prompt(profile: dict[str, Any], template_path: Path) -> str:
    template = template_path.read_text(encoding="utf-8")
    return template.replace("{{user_profile_block}}", profile_to_block(profile))


def profile_markdown(profile: dict[str, Any]) -> str:
    lines = [
        "# AirNote User Profile",
        "",
        f"- Version: `{profile.get('version', '-')}`",
        f"- Profile: `{profile.get('profile_name', '-')}`",
        "",
        "## Summary",
        "",
        str(profile.get("summary") or ""),
        "",
        "## Domains",
        "",
    ]
    for item in profile.get("domains") or []:
        terms = ", ".join(item.get("preferred_terms") or [])
        cues = ", ".join(item.get("cues") or [])
        lines.append(f"- **{item.get('name')}** priority={item.get('priority')}: {terms} | cues: {cues}")
    lines.extend(["", "## Phrase Recoveries", ""])
    for item in profile.get("phrase_recoveries") or []:
        lines.append(
            f"- `{item.get('heard')}` -> `{item.get('intended')}` ({item.get('domain')}, {item.get('confidence')})"
        )
    lines.extend(["", "## Prompt Focus Block", "", "```text", profile_to_block(profile), "```", ""])
    return "\n".join(lines)


def apply_prompt_to_local_db(db_path: Path, user_id: str, prompt_body: str) -> None:
    now = int(time.time() * 1000)
    event_id = str(uuid.uuid4())
    conn = sqlite3.connect(str(db_path))
    try:
        conn.execute(
            """
            UPDATE prompt_templates
               SET title='Voice cleaning system prompt - Abhishek profile test',
                   base_version='2026-06-23.personal-profile-v1',
                   active_body=?,
                   draft_body=NULL,
                   updated_at=?,
                   applied_at=?
             WHERE user_id=? AND kind='voice_system'
            """,
            (prompt_body, now, now, user_id),
        )
        if conn.total_changes == 0:
            conn.execute(
                """
                INSERT INTO prompt_templates
                (user_id, kind, title, base_version, active_body, draft_body, updated_at, applied_at)
                VALUES (?, 'voice_system', 'Voice cleaning system prompt - Abhishek profile test',
                        '2026-06-23.personal-profile-v1', ?, NULL, ?, ?)
                """,
                (user_id, prompt_body, now, now),
            )
        conn.execute(
            """
            INSERT INTO prompt_template_events
            (id, user_id, kind, event_type, body_snapshot, created_at)
            VALUES (?, ?, 'voice_system', 'apply_personal_profile_lab', ?, ?)
            """,
            (event_id, user_id, prompt_body, now),
        )
        conn.commit()
    finally:
        conn.close()


def smoke_test_prompt(prompt_body: str) -> list[dict[str, Any]]:
    cache = polish_lab.load_cache()
    if not cache or not cache.get("transcript"):
        return []
    transcript = cache["transcript"]
    results = []
    for route in polish_lab.resolve_polish_routes():
        polished, polish_s, actual_route = polish_lab.polish_transcript(
            transcript, prompt_body, route
        )
        results.append(
            {
                "provider": actual_route["provider"],
                "model": actual_route["model"],
                "polish_s": polish_s,
                "metrics": run_prompt_matrix.score_output(polished),
                "raw_transcript": transcript,
                "polished": polished,
            }
        )
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, default=DEFAULT_DB)
    parser.add_argument("--user-id", default="")
    parser.add_argument("--history-token-budget", type=int, default=12000)
    parser.add_argument("--profile-max-tokens", type=int, default=5000)
    parser.add_argument("--max-recordings", type=int, default=250)
    parser.add_argument("--max-edits", type=int, default=200)
    parser.add_argument("--template", type=Path, default=PROFILE_TEMPLATE)
    parser.add_argument("--seed-focus", action="append", default=[])
    parser.add_argument("--apply-local-prompt", action="store_true")
    parser.add_argument("--smoke-test", action="store_true")
    args = parser.parse_args()

    load_dotenv_no_override()
    if not args.db.is_file():
        raise SystemExit(f"AirNote DB not found: {args.db}")

    seed_focus = DEFAULT_USER_SEEDS + [s for s in args.seed_focus if s.strip()]
    conn = sqlite3.connect(str(args.db))
    try:
        user_id = args.user_id.strip() or first_user_id(conn)
    finally:
        conn.close()

    snapshot, history_text = build_history_snapshot(
        db_path=args.db,
        user_id=user_id,
        history_token_budget=args.history_token_budget,
        max_recordings=args.max_recordings,
        max_edits=args.max_edits,
        seed_focus=seed_focus,
    )
    profile = clamp_profile(call_deepseek_profile(history_text, args.profile_max_tokens))
    prompt_body = render_personal_prompt(profile, args.template)

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out_dir = OUT_ROOT / stamp
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "history_snapshot.json").write_text(
        json.dumps(snapshot, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    (out_dir / "history_prompt.txt").write_text(history_text, encoding="utf-8")
    (out_dir / "user_profile.json").write_text(
        json.dumps(profile, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    (out_dir / "user_profile.md").write_text(profile_markdown(profile), encoding="utf-8")
    (out_dir / "personal_voice_prompt.md").write_text(prompt_body + "\n", encoding="utf-8")

    smoke_results: list[dict[str, Any]] = []
    if args.smoke_test:
        smoke_results = smoke_test_prompt(prompt_body)
        (out_dir / "smoke_results.json").write_text(
            json.dumps(smoke_results, indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )
        lines = ["# Personal Prompt Smoke Test", ""]
        for item in smoke_results:
            lines.extend(
                [
                    f"## {item['provider']} / {item['model']}",
                    "",
                    f"- Polish: {item['polish_s']:.2f}s",
                    f"- Score: `{item['metrics']['score']}`",
                    f"- Expected hits: {', '.join(item['metrics']['expected_hits']) or '-'}",
                    f"- Missing: {', '.join(item['metrics']['missing_terms']) or '-'}",
                    f"- Bad garbles: {', '.join(item['metrics']['bad_hits']) or '-'}",
                    "",
                    "### Raw",
                    "",
                    item["raw_transcript"],
                    "",
                    "### Polished",
                    "",
                    item["polished"],
                    "",
                ]
            )
        (out_dir / "smoke_results.md").write_text("\n".join(lines), encoding="utf-8")

    if args.apply_local_prompt:
        apply_prompt_to_local_db(args.db, user_id, prompt_body)

    print(f"Wrote profile prompt run -> {out_dir.relative_to(REPO)}")
    print(f"Profile domains: {', '.join(str(d.get('name')) for d in (profile.get('domains') or [])[:8])}")
    print(f"Rendered prompt chars: {len(prompt_body)}")
    if args.apply_local_prompt:
        print("Applied rendered prompt to local prompt_templates voice_system row.")
    if smoke_results:
        for item in smoke_results:
            print(f"{item['provider']} / {item['model']} -> {item['polish_s']:.2f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
