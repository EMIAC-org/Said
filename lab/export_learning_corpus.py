#!/usr/bin/env python3
"""Export AirNote dictation learning corpus for offline lab iteration.

This is intentionally read-only. It pulls observed pairs such as:

    raw_stt/transcript -> polished_output -> user_kept

from local SQLite and, optionally, the dev/prod control-plane Postgres containers
over SSH. Outputs JSONL under lab/corpus/ by default; lab/corpus is gitignored.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sqlite3
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

LAB = Path(__file__).resolve().parent
DEFAULT_LOCAL_DB = Path.home() / "Library/Application Support/VoicePolish/db.sqlite"
DEFAULT_AUDIO_DIR = Path.home() / "Library/Application Support/VoicePolish/audio"
DEFAULT_OUT_DIR = LAB / "corpus"


def now_slug() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def text_hash(value: str | None) -> str | None:
    if not value:
        return None
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def user_hash(value: str | None) -> str | None:
    if not value:
        return None
    return hashlib.sha256(value.encode("utf-8")).hexdigest()[:16]


def nonempty(value: Any) -> str | None:
    if value is None:
        return None
    s = str(value).strip()
    return s or None


def word_count(text: str | None) -> int:
    return len((text or "").split())


def edit_bucket(polished: str | None, kept: str | None) -> str:
    p = (polished or "").strip()
    k = (kept or "").strip()
    if not k:
        return "missing_user_kept"
    if not p:
        return "missing_polished"
    if p == k:
        return "none"
    p_words = max(word_count(p), 1)
    k_words = word_count(k)
    if k_words > p_words * 1.5 or abs(k_words - p_words) > 12:
        return "large_rewrite"
    if k_words != p_words:
        return "medium"
    return "small_replace"


def normalize_sample(sample: dict[str, Any], *, include_text_hashes: bool = True) -> dict[str, Any]:
    raw_stt = nonempty(sample.get("raw_stt")) or nonempty(sample.get("transcript"))
    transcript = nonempty(sample.get("transcript")) or raw_stt
    polished = nonempty(sample.get("polished_output")) or nonempty(sample.get("ai_output"))
    kept = nonempty(sample.get("user_kept")) or nonempty(sample.get("final_text"))

    sample["raw_stt"] = raw_stt
    sample["transcript"] = transcript
    sample["polished_output"] = polished
    sample["user_kept"] = kept
    sample["has_user_kept"] = bool(kept)
    sample["word_count_raw"] = word_count(raw_stt)
    sample["word_count_kept"] = word_count(kept)
    sample["edit_bucket_lab"] = sample.get("edit_bucket") or edit_bucket(polished, kept)

    if include_text_hashes:
        sample["text_hashes"] = {
            "raw_stt": text_hash(raw_stt),
            "polished_output": text_hash(polished),
            "user_kept": text_hash(kept),
        }
    return sample


def write_jsonl(rows: Iterable[dict[str, Any]], output: Path) -> tuple[int, Path]:
    output.parent.mkdir(parents=True, exist_ok=True)
    count = 0
    with output.open("w", encoding="utf-8") as f:
        for row in rows:
            f.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")
            count += 1
    return count, output


def export_local_sqlite(
    *,
    db_path: Path,
    audio_dir: Path,
    include_identifiers: bool,
    limit: int,
) -> list[dict[str, Any]]:
    if not db_path.is_file():
        raise SystemExit(f"Local SQLite DB not found: {db_path}")

    con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row

    local_user = con.execute("SELECT id, email FROM local_user LIMIT 1").fetchone()
    local_user_id = local_user["id"] if local_user else None
    local_email = local_user["email"] if local_user else None

    rows: list[dict[str, Any]] = []

    recording_sql = """
        SELECT
            r.id,
            r.user_id,
            r.timestamp_ms,
            r.transcript,
            r.raw_transcript,
            r.local_corrected_transcript,
            r.polished_output,
            r.polished,
            r.final_text,
            r.model_used,
            r.word_count,
            r.recording_seconds,
            r.transcribe_ms,
            r.embed_ms,
            r.polish_ms,
            r.target_app,
            r.audio_id,
            e.id AS edit_event_id,
            e.user_kept AS edit_user_kept,
            e.ai_output AS edit_ai_output,
            e.edit_class,
            e.learning_kind
        FROM recordings r
        LEFT JOIN edit_events e
          ON e.id = (
            SELECT id FROM edit_events ee
             WHERE ee.recording_id = r.id
             ORDER BY ee.timestamp_ms DESC
             LIMIT 1
          )
        ORDER BY r.timestamp_ms DESC
        LIMIT ?
    """
    for r in con.execute(recording_sql, (limit,)):
        audio_id = nonempty(r["audio_id"])
        audio_path = str(audio_dir / f"{audio_id}.wav") if audio_id else None
        if audio_path and not Path(audio_path).is_file():
            audio_path = None
        sample = {
            "schema_version": 1,
            "source": "local_sqlite_recording",
            "sample_id": f"local-recording:{r['id']}",
            "recording_id": r["id"],
            "edit_event_id": r["edit_event_id"],
            "created_at_ms": r["timestamp_ms"],
            "account_hash": user_hash(r["user_id"] or local_user_id),
            "account_email": local_email if include_identifiers else None,
            "raw_stt": r["raw_transcript"] or r["transcript"],
            "transcript": r["local_corrected_transcript"] or r["transcript"],
            "polished_output": r["polished_output"] or r["edit_ai_output"] or r["polished"],
            "user_kept": r["edit_user_kept"] or r["final_text"],
            "model_used": r["model_used"],
            "target_app": r["target_app"],
            "audio_path": audio_path,
            "edit_class": r["edit_class"],
            "learning_kind": r["learning_kind"],
            "latency_ms": {
                "transcribe": r["transcribe_ms"],
                "embed": r["embed_ms"],
                "polish": r["polish_ms"],
            },
            "metadata": {
                "recording_seconds": r["recording_seconds"],
                "recording_word_count": r["word_count"],
            },
        }
        rows.append(normalize_sample(sample))

    edit_sql = """
        SELECT
            e.id,
            e.user_id,
            e.recording_id,
            e.timestamp_ms,
            e.transcript,
            e.ai_output,
            e.user_kept,
            e.target_app,
            e.edit_class,
            e.learning_kind
        FROM edit_events e
        ORDER BY e.timestamp_ms DESC
        LIMIT ?
    """
    for e in con.execute(edit_sql, (limit,)):
        sample = {
            "schema_version": 1,
            "source": "local_sqlite_edit_event",
            "sample_id": f"local-edit:{e['id']}",
            "recording_id": e["recording_id"],
            "edit_event_id": e["id"],
            "created_at_ms": e["timestamp_ms"],
            "account_hash": user_hash(e["user_id"] or local_user_id),
            "account_email": local_email if include_identifiers else None,
            "raw_stt": e["transcript"],
            "transcript": e["transcript"],
            "polished_output": e["ai_output"],
            "user_kept": e["user_kept"],
            "target_app": e["target_app"],
            "edit_class": e["edit_class"],
            "learning_kind": e["learning_kind"],
        }
        rows.append(normalize_sample(sample))

    return rows


def sql_literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def remote_export_sql(
    *,
    days: int,
    limit: int,
    email_like: str | None,
    include_identifiers: bool,
    source_label: str,
    sample_prefix: str,
) -> str:
    filters = [
        "h.deleted_at IS NULL",
        "h.created_at >= now() - make_interval(days => {days})".format(days=int(days)),
        "COALESCE(h.raw_transcript, h.transcript, h.polished_output, h.final_text, h.edit_feedback_json->>'user_kept') IS NOT NULL",
    ]
    if email_like:
        filters.append(f"a.email ILIKE '%' || {sql_literal(email_like)} || '%'")
    where = " AND ".join(filters)
    include = "true" if include_identifiers else "false"

    return f"""
WITH base AS (
  SELECT
    h.id,
    h.account_id,
    a.email,
    h.org_id,
    h.run_id,
    h.client_run_id,
    h.recording_id,
    h.device_id,
    h.platform,
    h.app_version,
    h.source,
    h.raw_transcript,
    h.transcript,
    h.local_corrected_transcript,
    h.polished_output,
    COALESCE(h.final_text, h.edit_feedback_json->>'user_kept') AS user_kept,
    h.model_used,
    h.word_count,
    h.recording_seconds,
    h.transcribe_ms,
    h.embed_ms,
    h.polish_ms,
    h.target_app,
    h.edit_feedback_json,
    h.created_at,
    h.updated_at,
    r.edit_bucket,
    r.edit_detected,
    r.total_ms,
    r.stt_provider,
    r.stt_model,
    r.stt_path,
    r.has_numbers,
    r.has_currency,
    r.has_percent,
    r.has_email,
    r.has_url,
    r.has_code_like_terms,
    r.mixed_language,
    r.protected_term_hit
  FROM runtime_history_items h
  JOIN accounts a ON a.id = h.account_id
  LEFT JOIN runtime_telemetry_runs r
    ON r.account_id = h.account_id
   AND (h.org_id IS NULL OR r.org_id = h.org_id)
   AND (
        (h.recording_id IS NOT NULL AND r.recording_id = h.recording_id)
        OR (h.client_run_id IS NOT NULL AND r.run_id = h.client_run_id)
   )
  WHERE {where}
  ORDER BY h.created_at DESC
  LIMIT {int(limit)}
)
SELECT encode(convert_to(jsonb_build_object(
  'schema_version', 1,
  'source', {sql_literal(source_label)},
  'sample_id', {sql_literal(sample_prefix)} || ':' || id::text,
  'account_hash', md5(account_id::text),
  'account_email', CASE WHEN {include} THEN email ELSE NULL END,
  'org_id', CASE WHEN {include} THEN org_id::text ELSE NULL END,
  'run_id', run_id::text,
  'client_run_id', client_run_id,
  'recording_id', recording_id,
  'created_at', created_at,
  'updated_at', updated_at,
  'raw_stt', COALESCE(raw_transcript, transcript),
  'transcript', COALESCE(local_corrected_transcript, transcript, raw_transcript),
  'polished_output', COALESCE(polished_output, edit_feedback_json->>'ai_output'),
  'user_kept', user_kept,
  'model_used', model_used,
  'target_app', target_app,
  'edit_bucket', edit_bucket,
  'has_user_kept', COALESCE(user_kept, '') <> '',
  'latency_ms', jsonb_build_object(
    'transcribe', transcribe_ms,
    'embed', embed_ms,
    'polish', polish_ms,
    'total', total_ms
  ),
  'stt', jsonb_build_object(
    'provider', stt_provider,
    'model', stt_model,
    'path', stt_path
  ),
  'content_flags', jsonb_build_object(
    'has_numbers', COALESCE(has_numbers, false),
    'has_currency', COALESCE(has_currency, false),
    'has_percent', COALESCE(has_percent, false),
    'has_email', COALESCE(has_email, false),
    'has_url', COALESCE(has_url, false),
    'has_code_like_terms', COALESCE(has_code_like_terms, false),
    'mixed_language', COALESCE(mixed_language, false),
    'protected_term_hit', COALESCE(protected_term_hit, false)
  ),
  'metadata', jsonb_build_object(
    'source', source,
    'platform', platform,
    'app_version', app_version,
    'device_id', device_id,
    'word_count', word_count,
    'recording_seconds', recording_seconds,
    'edit_detected', edit_detected
  ),
  'edit_feedback_json', edit_feedback_json
)::text, 'UTF8'), 'hex')
FROM base;
"""


def export_remote_postgres(
    *,
    ssh_host: str,
    ssh_user: str,
    pg_container: str,
    days: int,
    limit: int,
    email_like: str | None,
    include_identifiers: bool,
    source_label: str,
    sample_prefix: str,
) -> list[dict[str, Any]]:
    ssh_password = os.environ.get("AIRNOTE_SSH_PASSWORD") or os.environ.get("AIRNOTE_DEV_SSH_PASSWORD")
    if not ssh_password:
        raise SystemExit("Set AIRNOTE_SSH_PASSWORD or AIRNOTE_DEV_SSH_PASSWORD before remote export")

    env = os.environ.copy()
    env["SSHPASS"] = ssh_password
    sql = remote_export_sql(
        days=days,
        limit=limit,
        email_like=email_like,
        include_identifiers=include_identifiers,
        source_label=source_label,
        sample_prefix=sample_prefix,
    )
    cmd = [
        "sshpass",
        "-e",
        "ssh",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        f"{ssh_user}@{ssh_host}",
        (
            f"docker exec -i {pg_container} sh -lc "
            "'psql -U \"$POSTGRES_USER\" -d \"$POSTGRES_DB\" -At --no-align --tuples-only'"
        ),
    ]
    proc = subprocess.run(
        cmd,
        input=sql,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        check=False,
    )
    if proc.returncode != 0:
        raise SystemExit(
            "remote export failed\n"
            f"exit={proc.returncode}\n"
            f"stderr={proc.stderr.strip()}\n"
        )

    rows: list[dict[str, Any]] = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            raw_json = bytes.fromhex(line).decode("utf-8")
            rows.append(normalize_sample(json.loads(raw_json)))
        except json.JSONDecodeError as exc:
            raise SystemExit(f"bad JSON from remote psql: {exc}: {line[:300]}") from exc
        except ValueError as exc:
            raise SystemExit(f"bad hex from remote psql: {exc}: {line[:300]}") from exc
    return rows


def source_counts(rows: list[dict[str, Any]]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for row in rows:
        source = str(row.get("source") or "unknown")
        counts[source] = counts.get(source, 0) + 1
    return counts


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source",
        choices=["local", "remote-dev", "remote-prod", "remote-all", "all"],
        default="local",
        help="Corpus source to export.",
    )
    parser.add_argument("--local-db", type=Path, default=DEFAULT_LOCAL_DB)
    parser.add_argument("--audio-dir", type=Path, default=DEFAULT_AUDIO_DIR)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    parser.add_argument("--out", type=Path, help="Explicit JSONL output path.")
    parser.add_argument("--limit", type=int, default=500, help="Max rows per source query.")
    parser.add_argument("--days", type=int, default=90, help="Remote dev history window.")
    parser.add_argument("--ssh-host", default="103.180.163.41")
    parser.add_argument("--ssh-user", default="root")
    parser.add_argument("--pg-container", help="Override remote Postgres container.")
    parser.add_argument(
        "--remote-email-like",
        help="Filter remote dev account email, e.g. shivam or @emiactech.com.",
    )
    parser.add_argument(
        "--include-identifiers",
        action="store_true",
        help="Include account email/org ids in local gitignored corpus.",
    )
    args = parser.parse_args()

    rows: list[dict[str, Any]] = []
    if args.source in {"local", "all"}:
        rows.extend(
            export_local_sqlite(
                db_path=args.local_db,
                audio_dir=args.audio_dir,
                include_identifiers=args.include_identifiers,
                limit=args.limit,
            )
        )
    if args.source in {"remote-dev", "remote-all", "all"}:
        rows.extend(
            export_remote_postgres(
                ssh_host=args.ssh_host,
                ssh_user=args.ssh_user,
                pg_container=args.pg_container or "airnote-control-plane-dev-postgres-1",
                days=args.days,
                limit=args.limit,
                email_like=args.remote_email_like,
                include_identifiers=args.include_identifiers,
                source_label="remote_dev_runtime_history",
                sample_prefix="remote-dev",
            )
        )
    if args.source in {"remote-prod", "remote-all", "all"}:
        rows.extend(
            export_remote_postgres(
                ssh_host=args.ssh_host,
                ssh_user=args.ssh_user,
                pg_container=args.pg_container or "airnote-control-plane-postgres-1",
                days=args.days,
                limit=args.limit,
                email_like=args.remote_email_like,
                include_identifiers=args.include_identifiers,
                source_label="remote_prod_runtime_history",
                sample_prefix="remote-prod",
            )
        )

    if not rows:
        raise SystemExit("No corpus rows exported.")

    out = args.out or args.out_dir / f"learning_corpus_{args.source}_{now_slug()}.jsonl"
    count, path = write_jsonl(rows, out)
    summary = {
        "output": str(path),
        "rows": count,
        "sources": source_counts(rows),
        "with_user_kept": sum(1 for r in rows if r.get("has_user_kept")),
        "with_raw_stt": sum(1 for r in rows if r.get("raw_stt")),
        "created_at": datetime.now(timezone.utc).isoformat(),
    }
    summary_path = path.with_suffix(".summary.json")
    summary_path.write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
