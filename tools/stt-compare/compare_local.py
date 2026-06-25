#!/usr/bin/env python3
"""Compare local Hinglish STT models + Scout polish.

Models: Zero STT, Oriserve Apex, Oriserve Swift
Polish: Groq Scout only (control-plane polish-cli).
Weights: downloaded to tools/stt-compare/.hf-cache, purged after run.

Usage:
  python tools/stt-compare/compare_local.py
  python tools/stt-compare/compare_local.py --only swift
  python tools/stt-compare/compare_local.py --skip-stt --no-cleanup
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
STT_DIR = Path(__file__).resolve().parent
DEFAULT_AUDIO = Path.home() / "Downloads" / "6109386237469007631.ogg"
CACHE_PATH = STT_DIR / ".local_stt_benchmark.json"
WINNERS_PATH = STT_DIR / "WINNERS.json"
PY = REPO / "tools/zero-stt-hinglish-test/.venv/bin/python"
ZERO_SCRIPT = REPO / "tools/zero-stt-hinglish-test/transcribe.py"
CONTROL_PLANE = REPO / "crates/control-plane"
POLISH_BIN = CONTROL_PLANE / "target/debug/polish-cli"

MODELS = {
    "zero": {
        "id": "shunyalabs/zero-stt-hinglish",
        "label": "Zero STT Hinglish",
        "script": ZERO_SCRIPT,
        "raw_key": "zero_stt_raw",
        "time_key": "zero_stt_s",
        "polished_key": "zero_stt_polished",
        "polish_time_key": "zero_polish_s",
    },
    "apex": {
        "id": "Oriserve/Whisper-Hindi2Hinglish-Apex",
        "label": "Oriserve Apex",
        "script": STT_DIR / "transcribe_apex.py",
        "raw_key": "apex_raw",
        "time_key": "apex_stt_s",
        "polished_key": "apex_polished",
        "polish_time_key": "apex_polish_s",
    },
    "swift": {
        "id": "Oriserve/Whisper-Hindi2Hinglish-Swift",
        "label": "Oriserve Swift",
        "script": STT_DIR / "transcribe_swift.py",
        "raw_key": "swift_raw",
        "time_key": "swift_stt_s",
        "polished_key": "swift_polished",
        "polish_time_key": "swift_polish_s",
    },
}
POLISH_MODEL = "meta-llama/llama-4-scout-17b-16e-instruct"


def load_dotenv() -> None:
    env_path = REPO / ".env"
    if not env_path.is_file():
        return
    for line in env_path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, val = line.split("=", 1)
        key, val = key.strip(), val.strip().strip("\"'")
        if key and key not in os.environ:
            os.environ[key] = val


def hf_env() -> dict[str, str]:
    from hf_env import LOCAL_HF_HOME, use_local_hf_cache

    use_local_hf_cache()
    env = os.environ.copy()
    env["HF_HOME"] = str(LOCAL_HF_HOME)
    env["HF_HUB_CACHE"] = str(LOCAL_HF_HOME / "hub")
    env["PYTHONPATH"] = str(STT_DIR) + (
        os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else ""
    )
    return env


def parse_transcript(stdout: str) -> str:
    text = ""
    capture = False
    for line in stdout.splitlines():
        if line.strip() == "--- Transcript ---":
            capture = True
            continue
        if capture:
            if line.strip() == "--- End ---":
                break
            if not line.startswith("--- "):
                text += line
    return text.strip()


def run_transcribe(script: Path, audio: Path) -> tuple[str, float]:
    py = PY if PY.is_file() else Path(sys.executable)
    t0 = time.perf_counter()
    proc = subprocess.run(
        [str(py), str(script), str(audio)],
        capture_output=True,
        text=True,
        timeout=900,
        env=hf_env(),
        cwd=STT_DIR,
    )
    elapsed = time.perf_counter() - t0
    if proc.returncode != 0:
        raise RuntimeError(f"{script.name} failed:\n{proc.stderr}\n{proc.stdout}")
    text = parse_transcript(proc.stdout)
    if not text:
        raise RuntimeError(f"{script.name} returned empty transcript")
    return text, elapsed


def resolve_polish_cli() -> Path:
    for path in (POLISH_BIN, CONTROL_PLANE / "target/release/polish-cli"):
        if path.is_file():
            return path
    subprocess.run(
        ["cargo", "build", "--bin", "polish-cli"],
        cwd=CONTROL_PLANE,
        check=True,
    )
    for path in (POLISH_BIN, CONTROL_PLANE / "target/release/polish-cli"):
        if path.is_file():
            return path
    raise RuntimeError("polish-cli not found after build")


def polish_scout(raw: str) -> tuple[str, float]:
    env = os.environ.copy()
    env["OUTPUT_LANGUAGE"] = "hinglish"
    env["SELECTED_MODEL"] = "smart"
    t0 = time.perf_counter()
    proc = subprocess.run(
        [str(resolve_polish_cli()), raw],
        capture_output=True,
        text=True,
        timeout=180,
        env=env,
    )
    elapsed = time.perf_counter() - t0
    if proc.returncode != 0:
        raise RuntimeError(f"polish-cli failed: {proc.stderr.strip()}")
    return proc.stdout.strip(), elapsed


def section(title: str, body: str) -> None:
    print("\n" + "=" * 72)
    print(title)
    print("=" * 72)
    print(body)


def score_polished(text: str) -> int:
    """Heuristic quality score for winner ranking on the benchmark clip."""
    lower = text.lower()
    score = 0
    for token in ("vipul", "nipun", "agreement", "tds 2%", "times of india"):
        if token in lower:
            score += 1
    if "vikpul" in lower or "nikpurn" in lower:
        score -= 1
    return score


def pick_winner(result: dict) -> dict:
    ranked = []
    for key, meta in MODELS.items():
        polished = result.get(meta["polished_key"], "")
        if not polished:
            continue
        ranked.append(
            {
                "key": key,
                "id": meta["id"],
                "label": meta["label"],
                "score": score_polished(polished),
                "stt_s": result.get(meta["time_key"], 0),
            }
        )
    ranked.sort(key=lambda x: (-x["score"], x["stt_s"]))
    winner = ranked[0] if ranked else None
    return {
        "overall_winner": winner,
        "ranking": ranked,
        "scoring": "Scout-polished heuristics: vipul, nipun, agreement, tds 2%, times of india",
    }


def update_winners_file(result: dict, winners: dict) -> None:
    payload = {
        "last_updated": time.strftime("%Y-%m-%d"),
        "benchmark_audio": result.get("audio"),
        "polish_model": POLISH_MODEL,
        "overall_winner": winners.get("overall_winner"),
        "ranking": winners.get("ranking"),
        "models": {
            key: {
                "id": meta["id"],
                "label": meta["label"],
                "stt_s": result.get(meta["time_key"]),
                "raw_chars": len(result.get(meta["raw_key"], "")),
                "polished_preview": (result.get(meta["polished_key"], "")[:200] + "...")
                if len(result.get(meta["polished_key"], "")) > 200
                else result.get(meta["polished_key"], ""),
            }
            for key, meta in MODELS.items()
            if result.get(meta["raw_key"])
        },
    }
    WINNERS_PATH.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")


def cleanup_weights() -> None:
    from hf_env import purge_all_stt_weights

    result = purge_all_stt_weights()
    for path in result["local"]:
        print(f"  local: {path}")
    for path in result["global"]:
        print(f"  global: {path}")
    if not result["local"] and not result["global"]:
        print("  (nothing to remove)")


def main() -> int:
    load_dotenv()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio", nargs="?", type=Path, default=DEFAULT_AUDIO)
    parser.add_argument("--skip-stt", action="store_true")
    parser.add_argument("--no-cleanup", action="store_true")
    parser.add_argument(
        "--only",
        choices=tuple(MODELS.keys()),
        help="Run STT for one model only (others use cache if present)",
    )
    args = parser.parse_args()
    audio = args.audio.expanduser().resolve()
    if not audio.is_file():
        print(f"Audio not found: {audio}", file=sys.stderr)
        return 1

    print(f"Audio: {audio}")
    print(f"Polish: Groq Scout ({POLISH_MODEL})")
    print(f"HF cache: {STT_DIR / '.hf-cache'} (purged after run unless --no-cleanup)")

    result: dict = {}
    if CACHE_PATH.is_file():
        result = json.loads(CACHE_PATH.read_text(encoding="utf-8"))

    legacy = STT_DIR / ".last_compare.json"
    if legacy.is_file():
        leg = json.loads(legacy.read_text(encoding="utf-8"))
        for k, v in leg.items():
            result.setdefault(k, v)

    result["audio"] = str(audio)
    result["polish_model"] = POLISH_MODEL

    run_keys = [args.only] if args.only else list(MODELS.keys())
    step = 1
    total_stt = len(run_keys)

    for key in run_keys:
        meta = MODELS[key]
        cached_raw = result.get(meta["raw_key"])
        if args.skip_stt and cached_raw:
            print(f"\n[{step}/{total_stt}] {meta['label']} — cached")
            print(f"      {result.get(meta['time_key'], 0):.1f}s | {len(cached_raw)} chars")
        elif cached_raw and args.only != key and args.only is None:
            print(f"\n[{step}/{total_stt}] {meta['label']} — cached (no re-run)")
            print(f"      {result.get(meta['time_key'], 0):.1f}s | {len(cached_raw)} chars")
        else:
            print(f"\n[{step}/{total_stt}] {meta['label']}...")
            raw, stt_s = run_transcribe(meta["script"], audio)
            result[meta["raw_key"]] = raw
            result[meta["time_key"]] = stt_s
            print(f"      {stt_s:.1f}s | {len(raw)} chars")
        step += 1

    print("\n[polish] Scout polish (all available raws)...")
    for key, meta in MODELS.items():
        raw = result.get(meta["raw_key"])
        if not raw:
            continue
        polished, polish_s = polish_scout(raw)
        result[meta["polished_key"]] = polished
        result[meta["polish_time_key"]] = polish_s
        print(f"      {meta['label']}: {polish_s:.1f}s")

    winners = pick_winner(result)
    result["winners"] = winners
    CACHE_PATH.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    update_winners_file(result, winners)

    for key, meta in MODELS.items():
        raw = result.get(meta["raw_key"])
        polished = result.get(meta["polished_key"])
        if raw:
            section(f"{meta['label']} — RAW", raw)
        if polished:
            section(f"{meta['label']} — SCOUT POLISHED", polished)

    print("\n" + "-" * 72)
    print("TIMING")
    print("-" * 72)
    for key, meta in MODELS.items():
        if result.get(meta["raw_key"]):
            print(f"  {meta['label']:20} STT {result.get(meta['time_key'], 0):.1f}s")
    print("-" * 72)
    print("WINNER RANKING (Scout-polished heuristics)")
    for i, row in enumerate(winners.get("ranking", []), 1):
        mark = " <-- overall" if i == 1 else ""
        print(f"  {i}. {row['label']}  score={row['score']}  stt={row['stt_s']:.1f}s{mark}")
    print(f"\nSaved: {CACHE_PATH}")
    print(f"Saved: {WINNERS_PATH}")

    if not args.no_cleanup:
        print("\n[cleanup] Removing STT model weights from Mac...")
        cleanup_weights()
        print("[cleanup] Done.")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
