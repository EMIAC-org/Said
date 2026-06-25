#!/usr/bin/env python3
"""Local ASR for Oriserve/Whisper-Hindi2Hinglish-Apex."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

MODEL_ID = "Oriserve/Whisper-Hindi2Hinglish-Apex"
SCRIPT = Path(__file__).resolve().parent / "transcribe_model.py"


def main() -> int:
    cmd = [sys.executable, str(SCRIPT), "--model", MODEL_ID, *sys.argv[1:]]
    return subprocess.call(cmd)


if __name__ == "__main__":
    raise SystemExit(main())
