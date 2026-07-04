#!/usr/bin/env bash
set -euo pipefail

# Reset local onboarding / post-update-gate state for the macOS desktop app, so
# both first-run onboarding AND the "you just updated" forced gate can be
# replayed on demand while iterating.
#
# Usage:
#   reset-local-onboarding.sh            # FULL: replay fresh onboarding from scratch
#   reset-local-onboarding.sh update     # UPDATE: replay ONLY the post-update gate
#                                         #   (keeps you onboarded + your API keys /
#                                         #    model / prefs; just re-arms the
#                                         #    ModelMigrationGate as if you launched a
#                                         #    freshly-updated build)
#   reset-local-onboarding.sh update 1   # arm the gate as if you had satisfied
#                                         #   migration v1 — you then see only the
#                                         #   step(s) added after v1 (e.g. hotkey)
#
# Keeps recordings, audio, vocabulary, meetings, and downloaded local STT models.

MODE="${1:-full}"
FROM_VERSION="${2:-0}"

case "$MODE" in
  full|update) ;;
  *)
    echo "Unknown mode '$MODE'. Use 'full' (default) or 'update' [from_version]." >&2
    exit 2
    ;;
esac

APP_SUPPORT="${HOME}/Library/Application Support/VoicePolish"
DB_PATH="${APP_SUPPORT}/db.sqlite"
WEBKIT_ROOT="${HOME}/Library/WebKit"
WEBKIT_IDS=(
  "com.emiac.airnote.desktop"
  "said-desktop"
)

if [ "$MODE" = "update" ]; then
  echo "Re-arming AirNote post-update gate (from migration v${FROM_VERSION})..."
else
  echo "Resetting AirNote local onboarding state..."
fi

if pgrep -x "AirNote" >/dev/null 2>&1; then
  echo "Quitting AirNote..."
  osascript -e 'tell application "AirNote" to quit' >/dev/null 2>&1 || true
  sleep 1
fi

pkill -x "AirNote" >/dev/null 2>&1 || true
pkill -f "airnote-backend" >/dev/null 2>&1 || true
pkill -f "said-backend" >/dev/null 2>&1 || true

python3 - "$MODE" "$FROM_VERSION" "$DB_PATH" "$WEBKIT_ROOT" "${WEBKIT_IDS[@]}" <<'PY'
import sqlite3
import sys
from pathlib import Path

mode = sys.argv[1]
from_version = int(sys.argv[2])
db_path = Path(sys.argv[3]).expanduser()
webkit_root = Path(sys.argv[4]).expanduser()
webkit_ids = sys.argv[5:]

# localStorage key that arms the post-update forced gate (see lib/migration.ts).
MIGRATION_KEY = "said:migration-done"

# Keys cleared for a FULL fresh-onboarding reset.
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
    # Post-update gate + legacy model-setup flag — clear so a fresh user also sees
    # the current migration flow (onboarding re-stamps the version on completion,
    # so the gate never double-shows for the fresh path).
    MIGRATION_KEY,
    "said:local-model-setup-v1-done",
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


def ls_dbs(root: Path, app_id: str) -> list[Path]:
    base = root / app_id
    if not base.exists():
        return []
    return list(base.glob("WebsiteData/Default/*/*/LocalStorage/localstorage.sqlite3"))


def _checkpoint(conn: sqlite3.Connection) -> None:
    try:
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    except sqlite3.DatabaseError:
        pass


def clear_local_storage(root: Path, app_id: str, keys: set[str]) -> None:
    for db in ls_dbs(root, app_id):
        conn = sqlite3.connect(db)
        try:
            if not table_exists(conn, "ItemTable"):
                continue
            before = conn.execute("SELECT COUNT(*) FROM ItemTable").fetchone()[0]
            conn.executemany(
                "DELETE FROM ItemTable WHERE key = ?",
                [(key,) for key in keys],
            )
            conn.commit()
            _checkpoint(conn)
            after = conn.execute("SELECT COUNT(*) FROM ItemTable").fetchone()[0]
            print(f"Cleared {before - after} localStorage key(s) in: {db}")
        finally:
            conn.close()


def _encode(value: str) -> bytes:
    # WebKit stores localStorage strings as UTF-16LE BLOBs (no BOM).
    return value.encode("utf-16-le")


def rearm_update_gate(root: Path, app_id: str, from_version: int) -> None:
    """Keep the user onboarded but re-arm the post-update ModelMigrationGate.

    Writes `said:onboarding-complete = "true"` (so onboarding is skipped) and sets
    `said:migration-done` to `from_version` — deleting it entirely when that is 0 —
    so App.tsx's `migrationDone < MIGRATION_VERSION` check fires the gate again.
    """
    for db in ls_dbs(root, app_id):
        conn = sqlite3.connect(db)
        try:
            if not table_exists(conn, "ItemTable"):
                continue
            conn.execute(
                "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
                ("said:onboarding-complete", sqlite3.Binary(_encode("true"))),
            )
            if from_version > 0:
                conn.execute(
                    "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
                    (MIGRATION_KEY, sqlite3.Binary(_encode(str(from_version)))),
                )
            else:
                conn.execute("DELETE FROM ItemTable WHERE key = ?", (MIGRATION_KEY,))
            conn.commit()
            _checkpoint(conn)
            state = f"= {from_version}" if from_version > 0 else "cleared (→ 0)"
            print(f"Armed update gate (onboarding-complete=true, {MIGRATION_KEY} {state}) in: {db}")
        finally:
            conn.close()


if mode == "update":
    # Existing, fully set-up user who just updated — leave the app DB untouched.
    for app_id in webkit_ids:
        rearm_update_gate(webkit_root, app_id, from_version)
else:
    reset_app_db(db_path)
    for app_id in webkit_ids:
        clear_local_storage(webkit_root, app_id, LOCAL_STORAGE_KEYS)
PY

if [ "$MODE" = "update" ]; then
  echo "Done. Reopen AirNote to see the post-update gate (Meet the new model → hotkey)."
else
  echo "Done. Reopen AirNote to see the onboarding flow again."
fi
