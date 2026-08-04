#!/usr/bin/env bash
set -u

# AirNote local meeting diagnostics collector for macOS.
#
# What it does:
# - Scans local meeting artifact folders under VoicePolish/meetings.
# - Copies recent AirNote logs into a Desktop diagnostics folder.
# - Writes an inventory CSV/JSON and a concise summary.
# - Copies the concise summary to the clipboard with pbcopy.
#
# What it does NOT do:
# - It does not modify AirNote data.
# - It does not delete, move, transcribe, summarize, upload, or call any API.
# - It does not copy meeting audio or full transcript text into the bundle.

SINCE_DATE="${1:-2026-07-31}"
STAMP="$(date '+%Y-%m-%d_%H-%M-%S')"
OUT_DIR="${HOME}/Desktop/AirNote_Local_Meeting_Diagnostics_${STAMP}"
DATA_DIR="${HOME}/Library/Application Support/VoicePolish"
MEETINGS_ROOT="${DATA_DIR}/meetings"
LOG_DIR="${HOME}/Library/Logs/AirNote"
SUMMARY_FILE="${OUT_DIR}/AIRNOTE_DIAGNOSTIC_SUMMARY.txt"
ZIP_PATH="${OUT_DIR}.zip"

mkdir -p "${OUT_DIR}/logs" "${OUT_DIR}/raw"
chmod 700 "${OUT_DIR}" 2>/dev/null || true

copy_to_clipboard() {
  if [ "${AIRNOTE_DIAG_NO_PBCOPY:-0}" = "1" ]; then
    return 1
  fi
  if command -v pbcopy >/dev/null 2>&1; then
    pbcopy < "$1" && return 0
  fi
  return 1
}

write_python_missing_summary() {
  cat > "${SUMMARY_FILE}" <<EOF
AirNote local meeting diagnostics could not run because python3 is missing.

Please install Python 3 or ask support for the no-Python collector.

Checked at: $(date)
Mac user: $(whoami 2>/dev/null || echo unknown)
Data folder expected: ${DATA_DIR}
Meetings folder expected: ${MEETINGS_ROOT}
Logs folder expected: ${LOG_DIR}

Nothing was modified.
EOF
  copy_to_clipboard "${SUMMARY_FILE}" >/dev/null 2>&1 || true
  echo "python3 missing. A short message was copied to clipboard if pbcopy is available."
  exit 1
}

command -v python3 >/dev/null 2>&1 || write_python_missing_summary

{
  echo "Collected at: $(date)"
  echo "Since date: ${SINCE_DATE}"
  echo
  echo "sw_vers:"
  sw_vers 2>&1 || true
  echo
  echo "uname:"
  uname -a 2>&1 || true
  echo
  echo "User:"
  whoami 2>&1 || true
  echo
  echo "AirNote app versions:"
  for app in "/Applications/AirNote.app" "${HOME}/Applications/AirNote.app"; do
    if [ -d "$app" ]; then
      echo "$app"
      /usr/bin/defaults read "$app/Contents/Info" CFBundleShortVersionString 2>/dev/null || true
      /usr/bin/defaults read "$app/Contents/Info" CFBundleVersion 2>/dev/null || true
    fi
  done
  echo
  echo "AirNote processes:"
  ps axww -o pid,etime,pcpu,pmem,command | grep -Ei 'AirNote|airnote-backend|said-backend|whisper' | grep -v grep || true
} > "${OUT_DIR}/raw/system.txt"

{
  echo "Data dir: ${DATA_DIR}"
  [ -d "${DATA_DIR}" ] && du -sh "${DATA_DIR}" 2>/dev/null || echo "missing"
  echo
  echo "Meetings root: ${MEETINGS_ROOT}"
  [ -d "${MEETINGS_ROOT}" ] && du -sh "${MEETINGS_ROOT}" 2>/dev/null || echo "missing"
  echo
  echo "Logs dir: ${LOG_DIR}"
  [ -d "${LOG_DIR}" ] && du -sh "${LOG_DIR}" 2>/dev/null || echo "missing"
  echo
  echo "SQLite db:"
  ls -lh "${DATA_DIR}/db.sqlite" 2>/dev/null || echo "missing"
} > "${OUT_DIR}/raw/path_sizes.txt"

if [ -d "${LOG_DIR}" ]; then
  for file in "${LOG_DIR}/said.log" "${LOG_DIR}/backend.log" "${LOG_DIR}/crash-breadcrumbs.jsonl"; do
    if [ -f "$file" ]; then
      base="$(basename "$file")"
      tail -n 6000 "$file" > "${OUT_DIR}/logs/${base}.tail" 2>/dev/null || true
    fi
  done
  {
    for file in "${OUT_DIR}"/logs/*.tail; do
      [ -f "$file" ] || continue
      echo "===== $(basename "$file") ====="
      grep -Ein 'meeting|transcrib|summar|mom|intelligence|cancel|failed|error|warning|whisper|processing|recovery|removed empty|delete|hidden|archive' "$file" 2>/dev/null | tail -n 1200 || true
      echo
    done
  } > "${OUT_DIR}/logs/meeting_related_log_lines.txt"
else
  echo "Log directory missing: ${LOG_DIR}" > "${OUT_DIR}/logs/LOG_DIR_MISSING.txt"
fi

if [ -d "${MEETINGS_ROOT}" ]; then
  find "${MEETINGS_ROOT}" -maxdepth 3 -print0 2>/dev/null \
    | xargs -0 ls -ldeO@ 2>/dev/null \
    > "${OUT_DIR}/raw/meeting_file_tree.txt" || true
else
  echo "Meetings directory missing: ${MEETINGS_ROOT}" > "${OUT_DIR}/raw/meeting_file_tree.txt"
fi

python3 - "${MEETINGS_ROOT}" "${SINCE_DATE}" "${OUT_DIR}" <<'PY'
import csv
import json
import os
import re
import sys
import wave
from datetime import datetime, date, time, timedelta
from pathlib import Path

meetings_root = Path(sys.argv[1]).expanduser()
since_raw = sys.argv[2]
out_dir = Path(sys.argv[3]).expanduser()
now_ms = int(datetime.now().timestamp() * 1000)

RECOVERABLE_AUDIO = ("meeting.merged.wav", "mic.wav", "system.wav")
EXTRA_AUDIO = ("meeting.wav", "audio.wav")
AI_FILES = (
    "meeting.ai.json",
    "meeting.mom.json",
    "meeting-ai-manual/latest.meeting-ai.json",
    "meeting-ai-manual/summary.json",
)
TRANSCRIPT_JSON = (
    "meeting.transcript.final.json",
    "meeting.transcript.json",
    "meeting.merged.transcript.json",
    "mic.transcript.json",
    "system.transcript.json",
)
TRANSCRIPT_TXT = (
    "meeting.transcript.final.txt",
    "meeting.transcript.txt",
    "meeting.merged.transcript.txt",
    "mic.transcript.txt",
    "system.transcript.txt",
)

def parse_since(raw):
    try:
        d = date.fromisoformat(raw)
        return datetime.combine(d, time.min).astimezone()
    except Exception:
        return None

since_dt = parse_since(since_raw)

def read_text(path, limit=None):
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
        return text if limit is None else text[:limit]
    except Exception:
        return ""

def read_json(path):
    try:
        return json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except Exception as e:
        return {"__parse_error__": str(e)} if path.exists() else None

def safe_id(name):
    return re.match(r"^[A-Za-z0-9_-]{1,128}$", name) is not None

def ms_from_id(name):
    m = re.match(r"^local-(\d+)-", name)
    if not m:
        return None
    try:
        return int(m.group(1))
    except Exception:
        return None

def mtime_ms(path):
    try:
        return int(path.stat().st_mtime * 1000)
    except Exception:
        return None

def dt_from_ms(ms):
    if not ms:
        return None
    try:
        return datetime.fromtimestamp(ms / 1000).astimezone()
    except Exception:
        return None

def dt_label(dt):
    return dt.strftime("%Y-%m-%d %H:%M:%S %Z") if dt else "unknown"

def size(path):
    try:
        return path.stat().st_size
    except Exception:
        return 0

def word_count(text):
    return len((text or "").split())

def wav_duration_ms(path):
    try:
        with wave.open(str(path), "rb") as w:
            rate = w.getframerate()
            frames = w.getnframes()
            return int(frames * 1000 / rate) if rate else None
    except Exception:
        return None

def transcript_text_from_json(value):
    if not isinstance(value, dict):
        return ""
    for key in ("transcript", "text", "cleaned_transcript"):
        val = value.get(key)
        if isinstance(val, str) and val.strip():
            return val.strip()
    segs = value.get("segments")
    if isinstance(segs, list):
        parts = []
        for seg in segs:
            if not isinstance(seg, dict):
                continue
            text = str(seg.get("text") or "").strip()
            if not text:
                continue
            speaker = str(seg.get("speaker_name") or seg.get("speaker_id") or "Speaker").strip()
            start = seg.get("start_ms") or seg.get("display_start_ms") or seg.get("speech_start_ms") or 0
            try:
                start = int(start)
            except Exception:
                start = 0
            mm, ss = divmod(start // 1000, 60)
            parts.append(f"[{mm:02d}:{ss:02d} {speaker}] {text}")
        return "\n".join(parts)
    return ""

def scan_transcript(folder):
    candidates = []
    best = None
    for rel in TRANSCRIPT_JSON:
        path = folder / rel
        if not path.exists():
            continue
        value = read_json(path)
        parse_error = value.get("__parse_error__") if isinstance(value, dict) else ""
        status = value.get("status", "") if isinstance(value, dict) else ""
        error = value.get("error", "") if isinstance(value, dict) else ""
        text = "" if parse_error else transcript_text_from_json(value)
        item = {
            "path": rel,
            "kind": "json",
            "exists": True,
            "status": str(status or ""),
            "error": str(error or ""),
            "parse_error": str(parse_error or ""),
            "size_bytes": size(path),
            "words": word_count(text),
            "chars": len(text),
        }
        candidates.append(item)
        if best is None and text.strip():
            best = item
    for rel in TRANSCRIPT_TXT:
        path = folder / rel
        if not path.exists():
            continue
        text = read_text(path)
        item = {
            "path": rel,
            "kind": "txt",
            "exists": True,
            "status": "",
            "error": "",
            "parse_error": "",
            "size_bytes": size(path),
            "words": word_count(text),
            "chars": len(text),
        }
        candidates.append(item)
        if best is None and text.strip():
            best = item
    return best, candidates

def scan_live(folder):
    live = folder / "live" / "live-transcript.jsonl"
    manifest = read_json(folder / "live" / "live-manifest.json")
    result = {
        "exists": live.exists(),
        "path": "live/live-transcript.jsonl",
        "segments": 0,
        "words": 0,
        "size_bytes": size(live),
        "manifest": manifest if isinstance(manifest, dict) and "__parse_error__" not in manifest else {},
        "parse_errors": 0,
    }
    if not live.exists():
        return result
    for line in read_text(live).splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except Exception:
            result["parse_errors"] += 1
            continue
        text = str(obj.get("text") or "")
        result["segments"] += 1
        result["words"] += word_count(text)
    return result

def scan_ai(folder):
    out = []
    best = None
    for rel in AI_FILES:
        path = folder / rel
        if not path.exists():
            continue
        value = read_json(path)
        parse_error = value.get("__parse_error__") if isinstance(value, dict) else ""
        record = value
        if isinstance(value, list):
            objs = [x for x in value if isinstance(x, dict)]
            record = objs[-1] if objs else {}
        if isinstance(record, dict) and isinstance(record.get("filtered_mom"), str):
            try:
                embedded = json.loads(record["filtered_mom"])
                if isinstance(embedded, dict):
                    record = {**record, **embedded}
            except Exception:
                pass
        if not isinstance(record, dict):
            record = {}
        summary = str(record.get("summary") or record.get("mom") or record.get("minutes") or "")
        actions = record.get("action_items") or record.get("actions") or []
        decisions = record.get("decisions") or []
        item = {
            "path": rel,
            "parse_error": str(parse_error or ""),
            "size_bytes": size(path),
            "title": str(record.get("title") or "").strip(),
            "status": str(record.get("status") or "").strip(),
            "summary_chars": len(summary.strip()),
            "action_count": len(actions) if isinstance(actions, list) else 0,
            "decision_count": len(decisions) if isinstance(decisions, list) else 0,
        }
        out.append(item)
        if best is None and not parse_error and (item["summary_chars"] or item["action_count"] or item["decision_count"]):
            best = item
    return best, out

def scan_audio(folder):
    names = list(dict.fromkeys([*RECOVERABLE_AUDIO, *EXTRA_AUDIO] + [p.name for p in folder.glob("*.wav")]))
    out = []
    for name in names:
        path = folder / name
        if path.exists() and path.is_file():
            out.append({
                "path": name,
                "recognized_by_ui": name in RECOVERABLE_AUDIO,
                "size_bytes": size(path),
                "duration_ms": wav_duration_ms(path),
            })
    return out

def read_overrides():
    path = meetings_root / ".user-overrides.json"
    value = read_json(path)
    if isinstance(value, dict) and "__parse_error__" not in value:
        return value, ""
    if isinstance(value, dict) and "__parse_error__" in value:
        return {}, value["__parse_error__"]
    return {}, ""

def state(folder):
    value = read_json(folder / "meeting.state.json")
    if not isinstance(value, dict) or "__parse_error__" in value:
        return {"phase": "", "error": "", "updated_at_ms": None, "parse_error": value.get("__parse_error__", "") if isinstance(value, dict) else ""}
    return {
        "phase": str(value.get("phase") or ""),
        "error": str(value.get("error") or ""),
        "updated_at_ms": value.get("updated_at_ms") if isinstance(value.get("updated_at_ms"), int) else None,
        "parse_error": "",
    }

def list_relative_files(folder):
    files = []
    try:
        for path in folder.rglob("*"):
            if path.is_file():
                rel = str(path.relative_to(folder))
                files.append({"path": rel, "size_bytes": size(path), "mtime_ms": mtime_ms(path)})
    except Exception:
        pass
    return sorted(files, key=lambda x: x["path"])

def classify(row):
    has_ui_audio = any(a["recognized_by_ui"] for a in row["audio"])
    has_any_audio = bool(row["audio"])
    has_final_json = any(c["path"] == "meeting.transcript.final.json" for c in row["transcript_candidates"])
    has_completed_transcript = any(c["path"] == "meeting.transcript.json" and c["status"] == "completed" and c["words"] > 0 for c in row["transcript_candidates"])
    has_usable_transcript_for_ui = has_final_json or has_completed_transcript
    has_transcript_text = bool(row["best_transcript"])
    has_ai = bool(row["best_ai"])
    has_local_files_for_ui = has_ui_audio or has_usable_transcript_for_ui

    if not row["safe_id"]:
        ui_state = "skipped_by_ui_invalid_folder_name"
    elif not has_local_files_for_ui and not has_ai:
        ui_state = "skipped_by_ui_no_recognized_audio_transcript_or_mom"
    elif row["hidden"]:
        ui_state = "archived_tab" if has_local_files_for_ui else "hidden_no_local_files"
    else:
        ui_state = "all_tab"

    flags = []
    phase = row["phase"]
    error = (row["state_error"] or "").lower()
    if row["hidden"]:
        flags.append("HIDDEN_OR_ARCHIVED")
    if ui_state.startswith("skipped_by_ui"):
        flags.append("UI_SKIPPED")
    if has_transcript_text and not has_ai:
        flags.append("MOM_MISSING_TRANSCRIPT_PRESENT")
    if row["live"]["words"] and not has_transcript_text:
        flags.append("LIVE_TRANSCRIPT_ONLY_OR_PARTIAL")
    if has_any_audio and not has_transcript_text:
        flags.append("AUDIO_PRESENT_TRANSCRIPT_MISSING")
    if phase == "cancelled":
        flags.append("PROCESSING_CANCELLED")
    if phase == "failed":
        flags.append("PROCESSING_FAILED")
    if "summary failed" in error:
        flags.append("SUMMARY_FAILED")
    if phase in ("recording", "transcribing"):
        updated = row["state_updated_at_ms"] or row["created_at_ms"] or 0
        if updated and now_ms - updated > 30 * 60 * 1000:
            flags.append("NON_TERMINAL_STUCK_OVER_30_MIN")
    if has_ai:
        flags.append("MOM_PRESENT")
    if has_transcript_text:
        flags.append("TRANSCRIPT_PRESENT")
    if has_ui_audio:
        flags.append("UI_RECOGNIZED_AUDIO_PRESENT")
    elif has_any_audio:
        flags.append("NON_UI_AUDIO_PRESENT")
    if not flags:
        flags.append("NO_RECOVERABLE_ARTIFACTS_FOUND")
    return ui_state, flags

overrides, overrides_error = read_overrides()
rows = []
invalid_dirs = []

if meetings_root.exists():
    for folder in sorted([p for p in meetings_root.iterdir() if p.is_dir()], key=lambda p: p.name):
        if folder.name.startswith("."):
            continue
        sid = safe_id(folder.name)
        if not sid:
            invalid_dirs.append(str(folder))
        ov = overrides.get(folder.name, {})
        if not isinstance(ov, dict):
            ov = {}
        st = state(folder)
        best_transcript, transcript_candidates = scan_transcript(folder)
        best_ai, ai_candidates = scan_ai(folder)
        audio = scan_audio(folder)
        live = scan_live(folder)
        created_ms = ms_from_id(folder.name) or st.get("updated_at_ms") or mtime_ms(folder)
        created_dt = dt_from_ms(created_ms)
        title = str(ov.get("title") or (best_ai or {}).get("title") or "Untitled meeting").strip()
        row = {
            "id": folder.name,
            "safe_id": sid,
            "folder": str(folder),
            "created_at_ms": created_ms,
            "created_at": dt_label(created_dt),
            "in_since_window": bool(created_dt and since_dt and created_dt >= since_dt) if since_dt else True,
            "phase": st["phase"] or ("summarized" if best_ai else "transcribed" if best_transcript else "audio_only" if audio else "unknown"),
            "state_error": st["error"],
            "state_updated_at_ms": st["updated_at_ms"],
            "state_parse_error": st["parse_error"],
            "hidden": bool(ov.get("hidden")) if isinstance(ov.get("hidden"), bool) else False,
            "favorite": bool(ov.get("favorite")) if isinstance(ov.get("favorite"), bool) else False,
            "override_title": str(ov.get("title") or ""),
            "title": title,
            "lark_doc_url": str(ov.get("lark_doc_url") or ""),
            "best_ai": best_ai,
            "ai_candidates": ai_candidates,
            "best_transcript": best_transcript,
            "transcript_candidates": transcript_candidates,
            "audio": audio,
            "live": live,
            "notes_chars": len(read_text(folder / "meeting.notes.md")),
            "manual_actions_count": 0,
            "user_tags": [],
            "files": list_relative_files(folder),
        }
        manual = read_json(folder / "meeting.manual-actions.json")
        if isinstance(manual, dict) and isinstance(manual.get("items"), list):
            row["manual_actions_count"] = len(manual["items"])
        tags = read_json(folder / "meeting.tags.json")
        if isinstance(tags, dict) and isinstance(tags.get("tags"), list):
            row["user_tags"] = [str(t) for t in tags["tags"]]
        row["ui_state"], row["issue_flags"] = classify(row)
        rows.append(row)

rows_recent = [r for r in rows if r["in_since_window"]]

def count(rows, pred):
    return sum(1 for r in rows if pred(r))

summary = {
    "generated_at": datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S %Z"),
    "since": since_raw,
    "meetings_root": str(meetings_root),
    "meetings_root_exists": meetings_root.exists(),
    "override_parse_error": overrides_error,
    "total_valid_or_scannable_folders": len(rows),
    "invalid_folder_count": len(invalid_dirs),
    "recent_count": len(rows_recent),
    "recent_all_tab_count": count(rows_recent, lambda r: r["ui_state"] == "all_tab"),
    "recent_archived_count": count(rows_recent, lambda r: r["ui_state"] == "archived_tab"),
    "recent_ui_skipped_count": count(rows_recent, lambda r: r["ui_state"].startswith("skipped_by_ui")),
    "recent_with_mom_count": count(rows_recent, lambda r: bool(r["best_ai"])),
    "recent_with_transcript_count": count(rows_recent, lambda r: bool(r["best_transcript"])),
    "recent_audio_present_transcript_missing_count": count(rows_recent, lambda r: "AUDIO_PRESENT_TRANSCRIPT_MISSING" in r["issue_flags"]),
    "recent_mom_missing_transcript_present_count": count(rows_recent, lambda r: "MOM_MISSING_TRANSCRIPT_PRESENT" in r["issue_flags"]),
    "recent_cancelled_count": count(rows_recent, lambda r: "PROCESSING_CANCELLED" in r["issue_flags"]),
    "recent_failed_count": count(rows_recent, lambda r: "PROCESSING_FAILED" in r["issue_flags"] or "SUMMARY_FAILED" in r["issue_flags"]),
}

out_dir.joinpath("raw/meeting_inventory.json").write_text(json.dumps({"summary": summary, "meetings": rows, "invalid_dirs": invalid_dirs}, indent=2), encoding="utf-8")

with out_dir.joinpath("raw/meeting_inventory.csv").open("w", newline="", encoding="utf-8") as f:
    w = csv.writer(f)
    w.writerow(["created_at","id","title","phase","ui_state","hidden","favorite","words","has_mom","has_transcript","audio_files","live_words","issue_flags","state_error","folder"])
    for r in sorted(rows, key=lambda x: x["created_at_ms"] or 0, reverse=True):
        w.writerow([
            r["created_at"],
            r["id"],
            r["title"],
            r["phase"],
            r["ui_state"],
            "yes" if r["hidden"] else "no",
            "yes" if r["favorite"] else "no",
            (r["best_transcript"] or {}).get("words", 0),
            "yes" if r["best_ai"] else "no",
            "yes" if r["best_transcript"] else "no",
            "; ".join(f'{a["path"]}:{a["size_bytes"]}b:{a["duration_ms"] if a["duration_ms"] is not None else "bad_wav"}ms' for a in r["audio"]),
            r["live"]["words"],
            ", ".join(r["issue_flags"]),
            r["state_error"],
            r["folder"],
        ])

lines = []
lines.append("AIRNOTE LOCAL MEETING DIAGNOSTICS")
lines.append("")
lines.append(f"Generated at: {summary['generated_at']}")
lines.append(f"Since filter: {since_raw}")
lines.append(f"Meetings folder: {meetings_root}")
lines.append(f"Meetings folder exists: {'yes' if meetings_root.exists() else 'no'}")
lines.append(f"Override parse error: {overrides_error or 'none'}")
lines.append("")
lines.append("COUNTS")
for key in [
    "total_valid_or_scannable_folders",
    "invalid_folder_count",
    "recent_count",
    "recent_all_tab_count",
    "recent_archived_count",
    "recent_ui_skipped_count",
    "recent_with_mom_count",
    "recent_with_transcript_count",
    "recent_mom_missing_transcript_present_count",
    "recent_audio_present_transcript_missing_count",
    "recent_cancelled_count",
    "recent_failed_count",
]:
    lines.append(f"- {key}: {summary[key]}")
lines.append("")
lines.append("INTERPRETATION CHEAT SHEET")
lines.append("- MOM_MISSING_TRANSCRIPT_PRESENT: data is likely recoverable; generate MoM from local transcript.")
lines.append("- AUDIO_PRESENT_TRANSCRIPT_MISSING: audio likely exists; re-transcription is needed.")
lines.append("- LIVE_TRANSCRIPT_ONLY_OR_PARTIAL: partial live captions may be salvageable, but final transcript may not exist.")
lines.append("- UI_SKIPPED: AirNote's local list would not show this folder because it lacks recognized list artifacts.")
lines.append("- HIDDEN_OR_ARCHIVED: user-visible under Archived, not All.")
lines.append("- PROCESSING_CANCELLED / SUMMARY_FAILED: explains missing MoM after pausing/overlapping meetings.")
lines.append("")
lines.append(f"RECENT MEETINGS SINCE {since_raw}")
recent_sorted = sorted(rows_recent, key=lambda x: x["created_at_ms"] or 0, reverse=True)
if not recent_sorted:
    lines.append("(none found in local meeting folders for this date window)")
else:
    for r in recent_sorted[:80]:
        words = (r["best_transcript"] or {}).get("words", 0)
        audio = ",".join(a["path"] for a in r["audio"]) or "none"
        lines.append("")
        lines.append(f"- {r['created_at']} | {r['title']} | {r['id']}")
        lines.append(f"  phase={r['phase']} ui_state={r['ui_state']} hidden={r['hidden']} words={words} mom={'yes' if r['best_ai'] else 'no'} transcript={'yes' if r['best_transcript'] else 'no'} live_words={r['live']['words']} audio={audio}")
        lines.append(f"  flags={', '.join(r['issue_flags'])}")
        if r["state_error"]:
            lines.append(f"  state_error={r['state_error'][:500]}")
        if r["best_transcript"]:
            lines.append(f"  transcript_file={r['best_transcript']['path']} transcript_status={r['best_transcript']['status'] or 'n/a'}")
        if r["best_ai"]:
            lines.append(f"  mom_file={r['best_ai']['path']} summary_chars={r['best_ai']['summary_chars']}")
lines.append("")
lines.append("FILES IN THIS DIAGNOSTIC BUNDLE")
lines.append("- raw/meeting_inventory.csv")
lines.append("- raw/meeting_inventory.json")
lines.append("- raw/meeting_file_tree.txt")
lines.append("- raw/system.txt")
lines.append("- raw/path_sizes.txt")
lines.append("- logs/*.tail")
lines.append("- logs/meeting_related_log_lines.txt")
lines.append("")
lines.append("NOTE")
lines.append("This collector did not copy meeting audio or full transcript text. It only reports whether those artifacts exist and where they are located.")

out_dir.joinpath("AIRNOTE_DIAGNOSTIC_SUMMARY.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
PY
PY_STATUS=$?

if [ "$PY_STATUS" -ne 0 ]; then
  cat > "${SUMMARY_FILE}" <<EOF
AirNote local meeting diagnostics hit a Python error.

Checked at: $(date)
Since filter: ${SINCE_DATE}
Data folder: ${DATA_DIR}
Meetings folder: ${MEETINGS_ROOT}
Output folder: ${OUT_DIR}

Please send the output folder anyway. The logs and raw system files may still be present.
EOF
fi

{
  echo
  echo "Bundle folder: ${OUT_DIR}"
  echo "Zip path: ${ZIP_PATH}"
} >> "${SUMMARY_FILE}"

if command -v zip >/dev/null 2>&1; then
  (cd "$(dirname "${OUT_DIR}")" && zip -qry "$(basename "${ZIP_PATH}")" "$(basename "${OUT_DIR}")") || true
fi

copy_to_clipboard "${SUMMARY_FILE}" >/dev/null 2>&1 || true

echo
echo "AirNote diagnostics complete."
echo "A summary was copied to the clipboard."
echo "Folder: ${OUT_DIR}"
if [ -f "${ZIP_PATH}" ]; then
  echo "Zip: ${ZIP_PATH}"
fi
echo
echo "Ask the user to paste the clipboard summary back, and attach the zip/folder if possible."

if [ "${AIRNOTE_DIAG_NO_OPEN:-0}" != "1" ] && command -v open >/dev/null 2>&1; then
  open -R "${ZIP_PATH}" >/dev/null 2>&1 || open "${OUT_DIR}" >/dev/null 2>&1 || true
fi
