#!/usr/bin/env python3
"""
polish-bench — feed test transcripts to the running said-backend's
/v1/text/polish endpoint and check the LLM output against expectations.

Usage:
    scripts/polish-bench.py cases.jsonl            # auto-discover port from desktop log
    scripts/polish-bench.py cases.jsonl --port 60641
    scripts/polish-bench.py cases.jsonl --tone format     # use the format-fix prompt
    scripts/polish-bench.py cases.jsonl --diff            # print full input → output diff

Each JSONL case:
    {"name": "vav-email",
     "input": "VAB dot Verma 2678 at the rate Gmail dot com.",
     "expect_contains": ["@gmail.com", "2678"],
     "expect_not": ["at the rate", "dot com"]}
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

DEFAULT_LOG = Path.home() / "Library/Logs/Said/said.log"

GREEN = "\033[32m"
RED   = "\033[31m"
DIM   = "\033[2m"
BOLD  = "\033[1m"
CYAN  = "\033[36m"
RESET = "\033[0m"


def discover_port() -> int | None:
    """Find the port said-backend is listening on by scanning the desktop log."""
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


def polish(host: str, secret: str, text: str, tone_override: str | None) -> str:
    body: dict = {"text": text}
    if tone_override:
        body["tone_override"] = tone_override

    req = urllib.request.Request(
        f"{host}/v1/text/polish",
        data=json.dumps(body).encode("utf-8"),
        headers={
            "Content-Type":  "application/json",
            "Authorization": f"Bearer {secret}",
            "Accept":        "text/event-stream",
        },
        method="POST",
    )

    polished = None
    tokens: list[str] = []
    event_name = ""

    with urllib.request.urlopen(req, timeout=60) as resp:
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
            elif event_name == "error":
                raise RuntimeError(evt.get("message", "unknown error"))
            elif event_name == "token" and "token" in evt:
                tokens.append(evt["token"])

    return polished if polished is not None else "".join(tokens)


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
    p.add_argument("cases", type=Path, help="JSONL file of test cases")
    p.add_argument("--port",   type=int, default=None, help="backend port (default: auto-discover)")
    p.add_argument("--secret", default=os.environ.get("POLISH_SHARED_SECRET", "dev-secret"))
    p.add_argument("--tone",   default=None, help="optional tone_override (e.g. 'format', 'professional')")
    p.add_argument("--diff",   action="store_true", help="print full input → output diff for every case")
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

    print(f"{BOLD}→ host={host}  cases={len(cases)}  tone={args.tone or 'polish'}{RESET}\n")

    passed = failed = 0
    for case in cases:
        name      = case.get("name", "<unnamed>")
        inp       = case["input"]
        wants     = case.get("expect_contains", [])
        notwants  = case.get("expect_not", [])
        case_tone = case.get("tone", args.tone)

        try:
            out = polish(host, args.secret, inp, case_tone)
        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", errors="replace")[:200]
            print(f"{RED}✗ {name:30}{RESET} HTTP {e.code}: {body}")
            failed += 1
            continue
        except Exception as exc:
            print(f"{RED}✗ {name:30}{RESET} {exc}")
            failed += 1
            continue

        missing   = [s for s in wants    if s.lower() not in out.lower()]
        forbidden = [s for s in notwants if s.lower() in out.lower()]
        ok = not missing and not forbidden
        marker = f"{GREEN}✓{RESET}" if ok else f"{RED}✗{RESET}"
        print(f"{marker} {name:30}  {DIM}{out!r}{RESET}")
        if args.diff or not ok:
            print(f"   {DIM}in :  {inp!r}{RESET}")
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
