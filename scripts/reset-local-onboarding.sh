#!/usr/bin/env bash
set -euo pipefail

# Reset only local setup/onboarding state for the macOS desktop app.
# Keeps recordings, audio, vocabulary, meetings, and downloaded local STT models.

APP_SUPPORT="${HOME}/Library/Application Support/VoicePolish"
DB_PATH="${APP_SUPPORT}/db.sqlite"
WEBKIT_ROOT="${HOME}/Library/WebKit"
WEBKIT_IDS=(
  "com.emiac.airnote.desktop"
  "said-desktop"
)

echo "Resetting AirNote local onboarding state..."

if pgrep -x "AirNote" >/dev/null 2>&1; then
  echo "Quitting AirNote..."
  osascript -e 'tell application "AirNote" to quit' >/dev/null 2>&1 || true
  sleep 1
fi

pkill -x "AirNote" >/dev/null 2>&1 || true
pkill -f "airnote-backend" >/dev/null 2>&1 || true
pkill -f "said-backend" >/dev/null 2>&1 || true

python3 - "$DB_PATH" "$WEBKIT_ROOT" "${WEBKIT_IDS[@]}" <<'PY'
import os
import sqlite3
import sys
from pathlib import Path

db_path = Path(sys.argv[1]).expanduser()
webkit_root = Path(sys.argv[2]).expanduser()
webkit_ids = sys.argv[3:]

LOCAL_STORAGE_KEYS = {
    "said:onboarding-complete",
    # Per-step progress — without this the flow resumes mid-way and SKIPS steps
    # whose stale status is still "done" (e.g. jumps engine → test, past hotkey).
    "said:onboarding-progress",
    "said:onboarding-auth-mode",
    "said:enterprise",
    "said:enterprise-pending-url",
    "said:enterprise-recent-urls",
    "said:enterprise-device-id",
    # Server-URL override (Settings → Workspace server) so reset returns to the
    # env/default backend.
    "said:server-url-mode",
    "said:server-url-override",
}

PREFERENCE_RESETS = {
    "gateway_api_key": None,
    "deepgram_api_key": None,
    "gemini_api_key": None,
    "groq_api_key": None,
    "cerebras_api_key": None,
    "deepinfra_api_key": None,
    "sarvam_api_key": None,
    "llm_provider": "groq",
    "selected_model": "smart",
    "tone_preset": "neutral",
    "custom_prompt": None,
    "language": "auto",
    "output_language": "hinglish",
    "record_hotkey": "caps_lock",
    "stt_provider": "deepgram",
    "server_runtime_enabled": 0,
}


def table_exists(conn: sqlite3.Connection, table: str) -> bool:
    row = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
        (table,),
    ).fetchone()
    return row is not None


def columns(conn: sqlite3.Connection, table: str) -> set[str]:
    return {row[1] for row in conn.execute(f"PRAGMA table_info({table})")}


def reset_app_db(path: Path) -> None:
    if not path.exists():
        print(f"SQLite DB not found, skipping: {path}")
        return

    conn = sqlite3.connect(path)
    try:
        conn.execute("PRAGMA foreign_keys = ON")

        if table_exists(conn, "local_user"):
            cols = columns(conn, "local_user")
            assignments: list[str] = []
            values: list[object] = []
            for col, value in {
                "email": "local@voicepolish.app",
                "cloud_token": None,
                "license_tier": "free",
                "enterprise_server_url": None,
                "enterprise_org_name": None,
                "active_org_id": None,
            }.items():
                if col in cols:
                    assignments.append(f"{col} = ?")
                    values.append(value)
            if assignments:
                conn.execute(f"UPDATE local_user SET {', '.join(assignments)}", values)

        if table_exists(conn, "preferences"):
            cols = columns(conn, "preferences")
            assignments = []
            values = []
            for col, value in PREFERENCE_RESETS.items():
                if col in cols:
                    assignments.append(f"{col} = ?")
                    values.append(value)
            if "updated_at" in cols:
                assignments.append("updated_at = CAST(strftime('%s','now') AS INTEGER) * 1000")
            if assignments:
                conn.execute(f"UPDATE preferences SET {', '.join(assignments)}", values)

        for table in (
            "openai_oauth",
            "server_migration_state",
            "company_bucket_state",
            "company_vocab_upload_state",
            "company_vocabulary",
            "company_stt_replacements",
            "company_vocab_tombstones",
        ):
            if table_exists(conn, table):
                conn.execute(f"DELETE FROM {table}")

        conn.commit()
        try:
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        except sqlite3.DatabaseError:
            pass
        print(f"Reset local setup/auth/API-key state in: {path}")
    finally:
        conn.close()


def reset_local_storage(root: Path, app_id: str) -> None:
    base = root / app_id
    if not base.exists():
        return

    dbs = list(base.glob("WebsiteData/Default/*/*/LocalStorage/localstorage.sqlite3"))
    for db in dbs:
        conn = sqlite3.connect(db)
        try:
            if not table_exists(conn, "ItemTable"):
                continue
            before = conn.execute("SELECT COUNT(*) FROM ItemTable").fetchone()[0]
            conn.executemany(
                "DELETE FROM ItemTable WHERE key = ?",
                [(key,) for key in LOCAL_STORAGE_KEYS],
            )
            conn.commit()
            try:
                conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            except sqlite3.DatabaseError:
                pass
            after = conn.execute("SELECT COUNT(*) FROM ItemTable").fetchone()[0]
            print(f"Cleared {before - after} localStorage key(s) in: {db}")
        finally:
            conn.close()


reset_app_db(db_path)
for app_id in webkit_ids:
    reset_local_storage(webkit_root, app_id)
PY

echo "Done. Reopen AirNote to see the onboarding flow again."
