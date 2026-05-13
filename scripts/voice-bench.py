#!/usr/bin/env python3
"""
voice-bench — feed pre-built STT transcripts to the running said-backend's
/v1/voice endpoint (the real Hinglish voice pipeline) and check LLM output.

Unlike polish-bench (which tests /v1/text/polish with the tray prompt),
this tests the actual voice pipeline: Hinglish language rules, vocab hints,
RAG examples, Hindi particle preservation, and script romanization.

Usage:
    scripts/voice-bench.py                           # use default cases
    scripts/voice-bench.py cases.jsonl               # custom cases
    scripts/voice-bench.py --port 60641              # explicit port
    scripts/voice-bench.py --diff                    # show full input/output

Each JSONL case:
    {"name": "hindi-particles",
     "input": "इसका भी detail चाहिए",
     "expect_contains": ["bhi", "detail"],
     "expect_not": ["इसका"]}
"""

from __future__ import annotations

import argparse
import io
import json
import os
import re
import struct
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

DEFAULT_LOG = Path.home() / "Library/Logs/Said/said.log"
SCRIPT_DIR = Path(__file__).parent
DEFAULT_CASES = SCRIPT_DIR / "voice-cases.jsonl"

GREEN = "\033[32m"
RED   = "\033[31m"
DIM   = "\033[2m"
BOLD  = "\033[1m"
CYAN  = "\033[36m"
YELLOW = "\033[33m"
RESET = "\033[0m"


def discover_port() -> int | None:
    if not DEFAULT_LOG.exists():
        return None
    try:
        out = subprocess.check_output(
            ["grep", "daemon ready at http", str(DEFAULT_LOG)],
            text=True,
        )
    except subprocess.CalledProcessError:
        return None
    matches = re.findall(r"http://127\.0\.0\.1:(\d+)", out)
    return int(matches[-1]) if matches else None


def make_silent_wav(duration_ms: int = 100, sample_rate: int = 16000) -> bytes:
    num_samples = sample_rate * duration_ms // 1000
    buf = io.BytesIO()
    data_size = num_samples * 2
    buf.write(b"RIFF")
    buf.write(struct.pack("<I", 36 + data_size))
    buf.write(b"WAVE")
    buf.write(b"fmt ")
    buf.write(struct.pack("<IHHIIHH", 16, 1, 1, sample_rate, sample_rate * 2, 2, 16))
    buf.write(b"data")
    buf.write(struct.pack("<I", data_size))
    buf.write(b"\x00" * data_size)
    return buf.getvalue()


def voice_polish(host: str, secret: str, transcript: str) -> dict:
    boundary = "----VoiceBenchBoundary9876"
    wav_data = make_silent_wav()

    body = io.BytesIO()

    def write_field(name: str, value: str):
        body.write(f"--{boundary}\r\n".encode())
        body.write(f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode())
        body.write(f"{value}\r\n".encode())

    def write_file(name: str, filename: str, data: bytes, content_type: str):
        body.write(f"--{boundary}\r\n".encode())
        body.write(f'Content-Disposition: form-data; name="{name}"; filename="{filename}"\r\n'.encode())
        body.write(f"Content-Type: {content_type}\r\n\r\n".encode())
        body.write(data)
        body.write(b"\r\n")

    write_field("pre_transcript", transcript)
    write_file("audio", "silence.wav", wav_data, "audio/wav")
    body.write(f"--{boundary}--\r\n".encode())

    req = urllib.request.Request(
        f"{host}/v1/voice/polish",
        data=body.getvalue(),
        headers={
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "Authorization": f"Bearer {secret}",
            "Accept": "text/event-stream",
        },
        method="POST",
    )

    polished = None
    enriched = None
    model = None
    tokens: list[str] = []
    event_name = ""

    with urllib.request.urlopen(req, timeout=120) as resp:
        for raw in resp:
            line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
            if not line:
                event_name = ""
                continue
            if line.startswith("event:"):
                event_name = line[len("event:"):].strip()
                continue
            if not line.startswith("data:"):
                continue
            payload = line[len("data:"):].strip()
            if not payload:
                continue
            try:
                evt = json.loads(payload)
            except json.JSONDecodeError:
                continue
            if event_name == "done" and "polished" in evt:
                polished = evt["polished"]
                enriched = evt.get("enriched_transcript")
                model = evt.get("model_used")
            elif event_name == "error":
                raise RuntimeError(evt.get("message", "unknown error"))
            elif event_name == "token" and "token" in evt:
                tokens.append(evt["token"])

    return {
        "polished": polished if polished is not None else "".join(tokens),
        "enriched": enriched,
        "model": model,
    }


def parse_cases(path: Path) -> list[dict]:
    cases = []
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        try:
            cases.append(json.loads(s))
        except json.JSONDecodeError as e:
            print(f"{RED}line {n}: invalid JSON: {e}{RESET}", file=sys.stderr)
            sys.exit(2)
    return cases


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("cases", type=Path, nargs="?", default=DEFAULT_CASES, help="JSONL file of test cases")
    p.add_argument("--port",   type=int, default=None)
    p.add_argument("--secret", default=os.environ.get("POLISH_SHARED_SECRET", "dev-secret"))
    p.add_argument("--diff",   action="store_true", help="show input → output for every case")
    args = p.parse_args()

    port = args.port or discover_port()
    if not port:
        print(f"{RED}could not discover backend port — pass --port N or run said-desktop first{RESET}", file=sys.stderr)
        return 2
    host = f"http://127.0.0.1:{port}"

    cases = parse_cases(args.cases)
    if not cases:
        print(f"{RED}no cases in {args.cases}{RESET}", file=sys.stderr)
        return 2

    print(f"{BOLD}voice-bench → host={host}  cases={len(cases)}{RESET}\n")

    passed = failed = warned = 0
    for case in cases:
        name       = case.get("name", "<unnamed>")
        inp        = case["input"]
        wants      = case.get("expect_contains", [])
        notwants   = case.get("expect_not", [])
        expect_out = case.get("expected_output")

        try:
            result = voice_polish(host, args.secret, inp)
            out = result["polished"]
        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", errors="replace")[:200]
            print(f"{RED}✗ {name:35}{RESET} HTTP {e.code}: {body}")
            failed += 1
            continue
        except Exception as exc:
            print(f"{RED}✗ {name:35}{RESET} {exc}")
            failed += 1
            continue

        missing   = [s for s in wants    if s.lower() not in out.lower()]
        forbidden = [s for s in notwants if s.lower() in out.lower()]
        ok = not missing and not forbidden
        marker = f"{GREEN}✓{RESET}" if ok else f"{RED}✗{RESET}"
        print(f"{marker} {name:35}  {DIM}{out!r}{RESET}")

        if args.diff or not ok:
            print(f"   {DIM}in :  {inp!r}{RESET}")
        if expect_out:
            print(f"   {CYAN}want: {expect_out!r}{RESET}")
        if missing:
            print(f"   {RED}missing:{RESET}   {missing}")
        if forbidden:
            print(f"   {RED}forbidden:{RESET} {forbidden}")

        if ok:
            passed += 1
        else:
            failed += 1

    print()
    summary = f"{passed} passed, {failed} failed"
    color = GREEN if failed == 0 else RED
    print(f"{BOLD}{color}{summary}{RESET}")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
