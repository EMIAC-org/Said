#!/usr/bin/env python3
"""Async batch STT via Soniox REST API (stt-async-v5)."""

from __future__ import annotations

import argparse
import io
import json
import os
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

DEFAULT_AUDIO = Path.home() / "Downloads" / "6109386237469007631.ogg"
API_BASE = "https://api.soniox.com/v1"
MODEL = "stt-async-v5"


def load_dotenv(repo: Path) -> None:
    env_path = repo / ".env"
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


def api_json(method: str, url: str, key: str, body: bytes | None = None, headers: dict | None = None) -> dict:
    hdrs = {"Authorization": f"Bearer {key}"}
    if headers:
        hdrs.update(headers)
    req = urllib.request.Request(url, data=body, headers=hdrs, method=method)
    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            raw = resp.read()
            return json.loads(raw.decode()) if raw else {}
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"Soniox HTTP {exc.code}: {detail[:500]}") from exc


def upload_file(key: str, audio: Path) -> str:
    boundary = f"----Soniox{uuid.uuid4().hex}"
    data = audio.read_bytes()
    buf = io.BytesIO()
    buf.write(f"--{boundary}\r\n".encode())
    buf.write(f'Content-Disposition: form-data; name="file"; filename="{audio.name}"\r\n'.encode())
    buf.write(b"Content-Type: application/octet-stream\r\n\r\n")
    buf.write(data)
    buf.write(f"\r\n--{boundary}--\r\n".encode())
    payload = api_json(
        "POST",
        f"{API_BASE}/files",
        key,
        buf.getvalue(),
        {"Content-Type": f"multipart/form-data; boundary={boundary}"},
    )
    file_id = payload.get("id")
    if not file_id:
        raise RuntimeError(f"Soniox upload failed: {payload!r}")
    return file_id


def create_transcription(key: str, file_id: str) -> str:
    body = json.dumps(
        {
            "file_id": file_id,
            "model": MODEL,
            "language_hints": ["hi", "en"],
            "enable_language_identification": True,
        }
    ).encode()
    payload = api_json(
        "POST",
        f"{API_BASE}/transcriptions",
        key,
        body,
        {"Content-Type": "application/json"},
    )
    tx_id = payload.get("id")
    if not tx_id:
        raise RuntimeError(f"Soniox transcription create failed: {payload!r}")
    return tx_id


def poll_transcription(key: str, tx_id: str, timeout_s: float = 600) -> str:
    deadline = time.perf_counter() + timeout_s
    while time.perf_counter() < deadline:
        payload = api_json("GET", f"{API_BASE}/transcriptions/{tx_id}", key)
        status = payload.get("status") or ""
        if status == "completed":
            text = (payload.get("text") or "").strip()
            if not text and payload.get("tokens"):
                text = "".join(t.get("text", "") for t in payload["tokens"]).strip()
            if text:
                return text
            raise RuntimeError(f"Soniox completed with empty text: {payload!r}")
        if status in {"failed", "error"}:
            err = payload.get("error_message") or payload.get("message") or str(payload)
            raise RuntimeError(f"Soniox failed: {err}")
        time.sleep(2.0)
    raise RuntimeError("Soniox transcription timed out")


def main() -> int:
    repo = Path(__file__).resolve().parents[2]
    load_dotenv(repo)

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio", nargs="?", type=Path, default=DEFAULT_AUDIO)
    args = parser.parse_args()
    audio = args.audio.expanduser().resolve()
    if not audio.is_file():
        print(f"Error: audio not found: {audio}", file=sys.stderr)
        return 1

    api_key = os.environ.get("SONIOX_API_KEY", "").strip()
    if not api_key:
        print("Error: set SONIOX_API_KEY in .env or environment", file=sys.stderr)
        return 1

    print(f"Audio: {audio}")
    print(f"Model: {MODEL}")
    t0 = time.perf_counter()
    file_id = upload_file(api_key, audio)
    print(f"Uploaded file_id={file_id}", flush=True)
    tx_id = create_transcription(api_key, file_id)
    print(f"Transcription id={tx_id} (polling)...", flush=True)
    text = poll_transcription(api_key, tx_id)
    elapsed = time.perf_counter() - t0

    print("\n--- Transcript ---")
    print(text)
    print("--- End ---")
    print(f"Inference: {elapsed:.2f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
