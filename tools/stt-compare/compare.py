#!/usr/bin/env python3
"""Deepgram vs zero-stt-hinglish — raw STT + server-runtime Groq polish (hinglish).

Polish uses `polish-cli` from control-plane (same prompts/Groq params as
POST /v1/runtime/voice/polish).

Usage:
  python tools/stt-compare/compare.py
  python tools/stt-compare/compare.py ~/Downloads/foo.ogg
  python tools/stt-compare/compare.py --skip-stt   # reuse cached raw transcripts
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_AUDIO = Path.home() / "Downloads" / "6109386237469007631.ogg"
CACHE_PATH = Path(__file__).resolve().parent / ".last_compare.json"
ZERO_VENV = REPO / "tools/zero-stt-hinglish-test/.venv/bin/python"
ZERO_SCRIPT = REPO / "tools/zero-stt-hinglish-test/transcribe.py"
CONTROL_PLANE = REPO / "crates/control-plane"
POLISH_BIN = CONTROL_PLANE / "target/debug/polish-cli"


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


def deepgram_transcribe(audio: Path) -> tuple[str, float]:
    key = os.environ.get("DEEPGRAM_API_KEY", "")
    if not key:
        raise RuntimeError("DEEPGRAM_API_KEY not set in .env")

    data = audio.read_bytes()
    url = (
        "https://api.deepgram.com/v1/listen"
        "?model=nova-3&language=multi&smart_format=true"
    )
    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "Authorization": f"Token {key}",
            "Content-Type": "audio/ogg",
        },
        method="POST",
    )
    t0 = time.perf_counter()
    with urllib.request.urlopen(req, timeout=120) as resp:
        body = json.loads(resp.read().decode())
    elapsed = time.perf_counter() - t0
    text = (
        body.get("results", {})
        .get("channels", [{}])[0]
        .get("alternatives", [{}])[0]
        .get("transcript", "")
        .strip()
    )
    if not text:
        raise RuntimeError(f"Deepgram empty transcript: {body!r}")
    return text, elapsed


def zero_stt_transcribe(audio: Path) -> tuple[str, float]:
    if not ZERO_SCRIPT.is_file():
        raise RuntimeError(f"Missing {ZERO_SCRIPT} — run zero-stt setup first")
    py = ZERO_VENV if ZERO_VENV.is_file() else Path(sys.executable)
    t0 = time.perf_counter()
    proc = subprocess.run(
        [str(py), str(ZERO_SCRIPT), str(audio)],
        capture_output=True,
        text=True,
        timeout=600,
    )
    elapsed = time.perf_counter() - t0
    if proc.returncode != 0:
        raise RuntimeError(f"zero-stt failed:\n{proc.stderr}\n{proc.stdout}")

    text = ""
    capture = False
    for line in proc.stdout.splitlines():
        if line.strip() == "--- Transcript ---":
            capture = True
            continue
        if capture:
            if line.strip() == "--- End ---":
                break
            if line.startswith("--- "):
                continue
            text += line
    text = text.strip()
    if not text:
        # RNNT may be empty on some clips; fall back to CTC
        proc2 = subprocess.run(
            [str(py), str(ZERO_SCRIPT), str(audio), "--decoder", "ctc"],
            capture_output=True,
            text=True,
            timeout=600,
        )
        for line in proc2.stdout.splitlines():
            if line.strip() == "--- Transcript ---":
                capture = True
                text = ""
                continue
            if capture:
                if line.strip() == "--- End ---":
                    break
                if not line.startswith("--- "):
                    text += line
        text = text.strip()
    if not text:
        raise RuntimeError("zero-stt returned empty transcript")
    return text, elapsed


def resolve_polish_cli() -> Path:
    candidates = [
        POLISH_BIN,
        CONTROL_PLANE / "target/release/polish-cli",
    ]
    for path in candidates:
        if path.is_file():
            return path
    print("Building polish-cli (first time only)...", flush=True)
    subprocess.run(
        ["cargo", "build", "--bin", "polish-cli"],
        cwd=CONTROL_PLANE,
        check=True,
    )
    for path in candidates:
        if path.is_file():
            return path
    raise RuntimeError("polish-cli binary not found after cargo build")


def ensure_polish_cli() -> Path:
    return resolve_polish_cli()


def polish_transcript(raw: str) -> tuple[str, float]:
    polish_bin = ensure_polish_cli()
    env = os.environ.copy()
    env.setdefault("OUTPUT_LANGUAGE", "hinglish")
    env.setdefault("SELECTED_MODEL", "smart")
    t0 = time.perf_counter()
    proc = subprocess.run(
        [str(polish_bin), raw],
        capture_output=True,
        text=True,
        timeout=120,
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


def main() -> int:
    load_dotenv()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio", nargs="?", type=Path, default=DEFAULT_AUDIO)
    parser.add_argument("--skip-stt", action="store_true", help="Reuse cached raw transcripts")
    args = parser.parse_args()
    audio = args.audio.expanduser().resolve()
    if not audio.is_file():
        print(f"Audio not found: {audio}", file=sys.stderr)
        return 1

    print(f"Audio: {audio}")

    if args.skip_stt and CACHE_PATH.is_file():
        cached = json.loads(CACHE_PATH.read_text(encoding="utf-8"))
        raw_dg = cached["deepgram_raw"]
        raw_zs = cached["zero_stt_raw"]
        dg_stt_s = cached.get("deepgram_stt_s", 0)
        zs_stt_s = cached.get("zero_stt_s", 0)
        print("(using cached raw STT from .last_compare.json)")
    else:
        print("\n[1/4] Deepgram STT...")
        raw_dg, dg_stt_s = deepgram_transcribe(audio)
        print(f"      done in {dg_stt_s:.1f}s ({len(raw_dg)} chars)")

        print("\n[2/4] zero-stt-hinglish STT...")
        raw_zs, zs_stt_s = zero_stt_transcribe(audio)
        print(f"      done in {zs_stt_s:.1f}s ({len(raw_zs)} chars)")

        CACHE_PATH.write_text(
            json.dumps(
                {
                    "audio": str(audio),
                    "deepgram_raw": raw_dg,
                    "zero_stt_raw": raw_zs,
                    "deepgram_stt_s": dg_stt_s,
                    "zero_stt_s": zs_stt_s,
                },
                ensure_ascii=False,
                indent=2,
            ),
            encoding="utf-8",
        )

    print("\n[3/4] Polishing Deepgram raw (control-plane Groq pipeline)...")
    polished_dg, dg_polish_s = polish_transcript(raw_dg)
    print(f"      done in {dg_polish_s:.1f}s")

    print("\n[4/4] Polishing zero-stt raw (control-plane Groq pipeline)...")
    polished_zs, zs_polish_s = polish_transcript(raw_zs)
    print(f"      done in {zs_polish_s:.1f}s")

    section("DEEPGRAM — RAW STT", raw_dg)
    section("ZERO STT HINGLISH — RAW STT", raw_zs)
    section("DEEPGRAM — POLISHED (Roman Hinglish, Groq llama-3.1-8b-instant)", polished_dg)
    section("ZERO STT — POLISHED (Roman Hinglish, Groq llama-3.1-8b-instant)", polished_zs)

    print("\n" + "-" * 72)
    print("TIMING SUMMARY")
    print("-" * 72)
    if not args.skip_stt:
        print(f"  Deepgram STT:     {dg_stt_s:.1f}s")
        print(f"  Zero STT:         {zs_stt_s:.1f}s")
    print(f"  Polish (DG raw):  {dg_polish_s:.1f}s")
    print(f"  Polish (ZS raw):  {zs_polish_s:.1f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
