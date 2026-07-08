#!/usr/bin/env python3
"""Head-to-head STT competition on arbitrary audio files (WAV/OGG/MP3).

Uses batch/async APIs (transcript quality). Live WS providers are included
when a sync REST path exists; otherwise async upload+poll.

Env keys (all optional except you need at least one):
  GROQ_API_KEY
  OPENAI_API_KEY
  SARVAM_API_KEY
  SONIOX_API_KEY
  GLADIA_API_KEY
  SPEECHMATICS_API_KEY
"""

from __future__ import annotations

import io
import json
import mimetypes
import os
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
import wave
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

REPO_ROOT = Path(__file__).resolve().parents[2]


def load_dotenv(path: Path) -> None:
    if not path.is_file():
        return
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip().strip("\"'")
        if key and key not in os.environ:
            os.environ[key] = value


@dataclass
class Entry:
    name: str
    env_key: str
    run: Callable[[Path, float], "Result"]
    note: str = ""


@dataclass
class Result:
    provider: str
    ok: bool
    transcript: str = ""
    latency_ms: int = 0
    model: str = ""
    error: str = ""
    skipped: bool = False


def wav_duration_s(path: Path) -> float:
    try:
        with wave.open(str(path), "rb") as wf:
            rate = wf.getframerate()
            if rate:
                return wf.getnframes() / float(rate)
    except Exception:
        pass
    return 0.0


def ffprobe_duration_s(path: Path) -> float:
    try:
        out = subprocess.check_output(
            [
                "ffprobe",
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                str(path),
            ],
            text=True,
        ).strip()
        return float(out)
    except Exception:
        return 0.0


def ensure_wav_16k_mono(src: Path, work_dir: Path) -> Path:
    if src.suffix.lower() == ".wav":
        try:
            with wave.open(str(src), "rb") as wf:
                if wf.getframerate() == 16000 and wf.getnchannels() == 1 and wf.getsampwidth() == 2:
                    return src
        except Exception:
            pass
    out = work_dir / f"{src.stem}.16k.wav"
    subprocess.run(
        [
            "ffmpeg",
            "-y",
            "-i",
            str(src),
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            str(out),
        ],
        check=True,
        capture_output=True,
    )
    return out


def http_json(
    method: str,
    url: str,
    headers: Dict[str, str],
    body: Optional[bytes] = None,
    timeout: int = 120,
) -> Tuple[int, Any]:
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read()
            if not raw:
                return resp.status, {}
            return resp.status, json.loads(raw.decode("utf-8"))
    except urllib.error.HTTPError as exc:
        details = exc.read().decode("utf-8", errors="replace")
        try:
            payload = json.loads(details)
        except json.JSONDecodeError:
            payload = {"message": details[:800]}
        return exc.code, payload


def multipart_body(fields: List[Tuple[str, str]], files: List[Tuple[str, str, bytes, str]]) -> Tuple[bytes, str]:
    boundary = f"----SttCompete{uuid.uuid4().hex}"
    buf = io.BytesIO()
    for name, value in fields:
        buf.write(f"--{boundary}\r\n".encode())
        buf.write(f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode())
        buf.write(f"{value}\r\n".encode())
    for name, filename, data, content_type in files:
        buf.write(f"--{boundary}\r\n".encode())
        buf.write(f'Content-Disposition: form-data; name="{name}"; filename="{filename}"\r\n'.encode())
        buf.write(f"Content-Type: {content_type}\r\n\r\n".encode())
        buf.write(data)
        buf.write(b"\r\n")
    buf.write(f"--{boundary}--\r\n".encode())
    return buf.getvalue(), boundary


def poll_until(
    fetch: Callable[[], Tuple[str, Optional[str], Optional[str]]],
    timeout_s: int = 300,
    interval_s: float = 2.0,
) -> str:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        status, transcript, err = fetch()
        status_l = (status or "").lower()
        if status_l in {"done", "completed", "success", "succeeded"}:
            return transcript or ""
        if status_l in {"failed", "error"}:
            raise RuntimeError(err or f"job failed: {status}")
        time.sleep(interval_s)
    raise TimeoutError("polling timed out")


def run_openai_transcribe(wav: Path, duration_s: float, model: str) -> Result:
    key = os.environ.get("OPENAI_API_KEY", "").strip()
    if not key:
        return Result(model, False, skipped=True, error="OPENAI_API_KEY missing")
    body, boundary = multipart_body(
        [("model", model), ("response_format", "json")],
        [("file", wav.name, wav.read_bytes(), "audio/wav")],
    )
    t0 = time.perf_counter()
    status, payload = http_json(
        "POST",
        "https://api.openai.com/v1/audio/transcriptions",
        {
            "Authorization": f"Bearer {key}",
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
        body,
        timeout=300,
    )
    latency_ms = int((time.perf_counter() - t0) * 1000)
    if status >= 400:
        return Result(model, False, latency_ms=latency_ms, error=str(payload)[:500])
    text = (payload.get("text") or "").strip()
    return Result(model, bool(text), transcript=text, latency_ms=latency_ms, model=model)


def sarvam_rest_chunk(wav: Path, mode: str, headers: Dict[str, str]) -> str:
    fields = [("model", "saaras:v3"), ("mode", mode), ("language_code", "hi-IN")]
    body, boundary = multipart_body(fields, [("file", wav.name, wav.read_bytes(), "audio/wav")])
    status, payload = http_json(
        "POST",
        "https://api.sarvam.ai/speech-to-text",
        {**headers, "Content-Type": f"multipart/form-data; boundary={boundary}"},
        body,
        timeout=180,
    )
    if status >= 400:
        raise RuntimeError(str(payload)[:500])
    text = (payload.get("transcript") or payload.get("text") or "").strip()
    if not text and isinstance(payload.get("data"), dict):
        text = (payload["data"].get("transcript") or "").strip()
    return text


def split_wav_chunks(src: Path, work_dir: Path, chunk_s: float = 29.0) -> List[Path]:
    duration = ffprobe_duration_s(src) or wav_duration_s(src)
    if duration <= chunk_s:
        return [src]
    chunks: List[Path] = []
    start = 0.0
    idx = 0
    while start < duration - 0.05:
        out = work_dir / f"{src.stem}.chunk{idx}.wav"
        cmd = [
            "ffmpeg",
            "-y",
            "-ss",
            str(start),
            "-i",
            str(src),
            "-t",
            str(chunk_s),
            "-ar",
            "16000",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            str(out),
        ]
        if start == 0.0:
            cmd = [
                "ffmpeg",
                "-y",
                "-i",
                str(src),
                "-t",
                str(chunk_s),
                "-ar",
                "16000",
                "-ac",
                "1",
                "-c:a",
                "pcm_s16le",
                str(out),
            ]
        subprocess.run(cmd, check=True, capture_output=True)
        chunks.append(out)
        start += chunk_s
        idx += 1
    return chunks


def run_sarvam(wav: Path, duration_s: float, mode: str) -> Result:
    key = os.environ.get("SARVAM_API_KEY", "").strip()
    if not key:
        return Result(f"sarvam:{mode}", False, skipped=True, error="SARVAM_API_KEY missing")
    headers = {"api-subscription-key": key}
    t0 = time.perf_counter()
    chunk_dir = wav.parent / "sarvam_chunks"
    chunk_dir.mkdir(exist_ok=True)
    try:
        parts: List[str] = []
        for chunk in split_wav_chunks(wav, chunk_dir):
            text = sarvam_rest_chunk(chunk, mode, headers)
            if text:
                parts.append(text)
        text = " ".join(parts).strip()
    except Exception as exc:
        latency_ms = int((time.perf_counter() - t0) * 1000)
        return Result(f"sarvam:{mode}", False, latency_ms=latency_ms, error=str(exc))
    latency_ms = int((time.perf_counter() - t0) * 1000)
    return Result(f"sarvam:{mode}", bool(text), transcript=text, latency_ms=latency_ms, model="saaras:v3")


def run_soniox(wav: Path, duration_s: float) -> Result:
    key = os.environ.get("SONIOX_API_KEY", "").strip()
    if not key:
        return Result("soniox", False, skipped=True, error="SONIOX_API_KEY missing")
    t0 = time.perf_counter()
    body, boundary = multipart_body([], [("file", wav.name, wav.read_bytes(), "audio/wav")])
    status, uploaded = http_json(
        "POST",
        "https://api.soniox.com/v1/files",
        {
            "Authorization": f"Bearer {key}",
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
        body,
        timeout=180,
    )
    if status >= 400:
        return Result("soniox", False, error=str(uploaded)[:500])
    file_id = uploaded.get("id")
    if not file_id:
        return Result("soniox", False, error=f"no file id: {uploaded}")

    model = "stt-async-v4"
    status, created = http_json(
        "POST",
        "https://api.soniox.com/v1/transcriptions",
        {"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
        json.dumps(
            {
                "file_id": file_id,
                "model": model,
                "language_hints": ["hi", "en"],
                "enable_language_identification": True,
            }
        ).encode(),
        timeout=60,
    )
    if status >= 400:
        return Result("soniox", False, error=str(created)[:500])
    tx_id = created.get("id")
    if not tx_id:
        return Result("soniox", False, error=f"no transcription id: {created}")

    def fetch() -> Tuple[str, Optional[str], Optional[str]]:
        code, payload = http_json(
            "GET",
            f"https://api.soniox.com/v1/transcriptions/{tx_id}",
            {"Authorization": f"Bearer {key}"},
            timeout=60,
        )
        if code >= 400:
            return "failed", None, str(payload)[:300]
        st = payload.get("status") or ""
        if st == "completed":
            text = (payload.get("text") or "").strip()
            if not text and payload.get("tokens"):
                text = "".join(t.get("text", "") for t in payload["tokens"]).strip()
            return "done", text, None
        if st in {"failed", "error"}:
            err = payload.get("error_message") or payload.get("message") or str(payload)[:300]
            return "failed", None, err
        return st or "processing", None, None

    try:
        text = poll_until(fetch, timeout_s=600, interval_s=2.0)
    except Exception as exc:
        latency_ms = int((time.perf_counter() - t0) * 1000)
        return Result("soniox", False, latency_ms=latency_ms, error=str(exc))
    latency_ms = int((time.perf_counter() - t0) * 1000)
    return Result("soniox", bool(text), transcript=text, latency_ms=latency_ms, model=model)


def run_gladia(wav: Path, duration_s: float, code_switching: bool) -> Result:
    key = os.environ.get("GLADIA_API_KEY", "").strip()
    if not key:
        return Result("gladia", False, skipped=True, error="GLADIA_API_KEY missing")
    t0 = time.perf_counter()
    body, boundary = multipart_body([], [("audio", wav.name, wav.read_bytes(), "audio/wav")])
    status, uploaded = http_json(
        "POST",
        "https://api.gladia.io/v2/upload",
        {
            "x-gladia-key": key,
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
        body,
        timeout=180,
    )
    if status >= 400:
        return Result("gladia", False, error=str(uploaded)[:500])
    audio_url = uploaded.get("audio_url")
    if not audio_url:
        return Result("gladia", False, error=f"no audio_url: {uploaded}")

    label = "gladia:code_switch" if code_switching else "gladia:auto"
    status, created = http_json(
        "POST",
        "https://api.gladia.io/v2/pre-recorded",
        {"x-gladia-key": key, "Content-Type": "application/json"},
        json.dumps(
            {
                "audio_url": audio_url,
                "language_config": {
                    "languages": ["hi", "en"] if code_switching else [],
                    "code_switching": code_switching,
                },
            }
        ).encode(),
        timeout=60,
    )
    if status >= 400:
        return Result(label, False, error=str(created)[:500])
    job_id = created.get("id")
    result_url = created.get("result_url") or (f"https://api.gladia.io/v2/pre-recorded/{job_id}" if job_id else "")
    if not result_url:
        return Result(label, False, error=f"no result url: {created}")

    def fetch() -> Tuple[str, Optional[str], Optional[str]]:
        code, payload = http_json("GET", result_url, {"x-gladia-key": key}, timeout=60)
        if code >= 400:
            return "failed", None, str(payload)[:300]
        st = payload.get("status") or ""
        if st == "done":
            result = payload.get("result", {})
            text = (result.get("transcription", {}).get("full_transcript") or "").strip()
            if not text and result.get("transcription", {}).get("utterances"):
                text = " ".join(u.get("text", "") for u in result["transcription"]["utterances"]).strip()
            return "done", text, None
        if st == "error":
            return "failed", None, str(payload)[:300]
        return st or "processing", None, None

    try:
        text = poll_until(fetch, timeout_s=600, interval_s=2.0)
    except Exception as exc:
        latency_ms = int((time.perf_counter() - t0) * 1000)
        return Result(label, False, latency_ms=latency_ms, error=str(exc))
    latency_ms = int((time.perf_counter() - t0) * 1000)
    return Result(label, bool(text), transcript=text, latency_ms=latency_ms, model="solaria")


def run_speechmatics(wav: Path, duration_s: float) -> Result:
    key = os.environ.get("SPEECHMATICS_API_KEY", "").strip()
    if not key:
        return Result("speechmatics", False, skipped=True, error="SPEECHMATICS_API_KEY missing")
    config = json.dumps(
        {
            "type": "transcription",
            "transcription_config": {"language": "hi", "operating_point": "enhanced"},
        }
    )
    body, boundary = multipart_body(
        [("config", config)],
        [("data_file", wav.name, wav.read_bytes(), "audio/wav")],
    )
    t0 = time.perf_counter()
    status, payload = http_json(
        "POST",
        "https://asr.api.speechmatics.com/v2/jobs",
        {
            "Authorization": f"Bearer {key}",
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
        body,
        timeout=180,
    )
    if status >= 400:
        return Result("speechmatics", False, error=str(payload)[:500])
    job_id = payload.get("id")
    if not job_id:
        return Result("speechmatics", False, error=f"no job id: {payload}")

    def fetch() -> Tuple[str, Optional[str], Optional[str]]:
        code, job = http_json(
            "GET",
            f"https://asr.api.speechmatics.com/v2/jobs/{job_id}",
            {"Authorization": f"Bearer {key}"},
            timeout=60,
        )
        if code >= 400:
            return "failed", None, str(job)[:300]
        st = job.get("job", {}).get("status") or job.get("status") or ""
        if st == "done":
            tr_url = f"https://asr.api.speechmatics.com/v2/jobs/{job_id}/transcript"
            with urllib.request.urlopen(
                urllib.request.Request(tr_url, headers={"Authorization": f"Bearer {key}"}),
                timeout=120,
            ) as resp:
                transcript_json = json.loads(resp.read().decode("utf-8"))
            parts = []
            for item in transcript_json.get("results", []):
                alt = item.get("alternatives", [{}])[0]
                content = alt.get("content", "")
                if item.get("type") == "word" and content:
                    parts.append(content)
            text = " ".join(parts).strip()
            return "done", text, None
        if st == "rejected" or st == "failed":
            return "failed", None, str(job)[:300]
        return st or "processing", None, None

    try:
        text = poll_until(fetch, timeout_s=900, interval_s=3.0)
    except Exception as exc:
        latency_ms = int((time.perf_counter() - t0) * 1000)
        return Result("speechmatics", False, latency_ms=latency_ms, error=str(exc))
    latency_ms = int((time.perf_counter() - t0) * 1000)
    return Result("speechmatics", bool(text), transcript=text, latency_ms=latency_ms, model="ursa")


def discover_downloads_audio(downloads: Path) -> List[Path]:
    paths: List[Path] = []
    for pattern in ("*.ogg", "*.OGG", "*.wav", "*.WAV", "*.mp3", "*.MP3"):
        paths.extend(downloads.glob(pattern))
    return sorted({p.resolve() for p in paths if p.is_file()}, key=lambda p: p.name)


def run_competition(audio_files: List[Path], out_dir: Path) -> int:
    out_dir.mkdir(parents=True, exist_ok=True)
    work_dir = out_dir / "converted"
    work_dir.mkdir(exist_ok=True)

    runners: List[Tuple[str, Callable[[Path, float], Result]]] = [
        ("gpt-4o-transcribe", lambda w, d: run_openai_transcribe(w, d, "gpt-4o-transcribe")),
        ("gpt-4o-mini-transcribe", lambda w, d: run_openai_transcribe(w, d, "gpt-4o-mini-transcribe")),
        ("whisper-1", lambda w, d: run_openai_transcribe(w, d, "whisper-1")),
        ("sarvam:codemix", lambda w, d: run_sarvam(w, d, "codemix")),
        ("sarvam:transcribe", lambda w, d: run_sarvam(w, d, "transcribe")),
        ("soniox", run_soniox),
        ("gladia:code_switch", lambda w, d: run_gladia(w, d, True)),
        ("gladia:auto", lambda w, d: run_gladia(w, d, False)),
        ("speechmatics:hi", run_speechmatics),
    ]

    all_rows: List[Dict[str, Any]] = []
    for src in audio_files:
        duration_s = ffprobe_duration_s(src) or wav_duration_s(src)
        wav = ensure_wav_16k_mono(src, work_dir)
        print(f"\n=== {src.name} ({duration_s:.1f}s) ===", flush=True)
        for label, fn in runners:
            print(f"  → {label} ...", flush=True)
            result = fn(wav, duration_s)
            result.provider = label
            row = {
                "audio_file": str(src),
                "audio_name": src.name,
                "duration_s": round(duration_s, 2),
                "provider": label,
                "ok": result.ok,
                "skipped": result.skipped,
                "latency_ms": result.latency_ms,
                "model": result.model,
                "error": result.error,
                "transcript": result.transcript,
                "word_count": len(result.transcript.split()) if result.transcript else 0,
            }
            all_rows.append(row)
            if result.skipped:
                print(f"     SKIP: {result.error}")
            elif result.ok:
                preview = result.transcript[:140].replace("\n", " ")
                wc = len(result.transcript.split()) if result.transcript else 0
                print(f"     OK {result.latency_ms}ms | {wc} words | {preview}...")
            else:
                print(f"     FAIL: {result.error[:200]}")

    results_path = out_dir / "competition.jsonl"
    with results_path.open("w", encoding="utf-8") as fh:
        for row in all_rows:
            fh.write(json.dumps(row, ensure_ascii=False) + "\n")

    # Markdown summary per audio
    md_path = out_dir / "competition.md"
    lines = ["# STT Competition Results\n"]
    by_audio: Dict[str, List[Dict[str, Any]]] = {}
    for row in all_rows:
        by_audio.setdefault(row["audio_name"], []).append(row)

    missing_keys = sorted(
        {
            r["error"]
            for r in all_rows
            if r["skipped"] and r["error"].endswith("missing")
        }
    )

    for audio_name, rows in by_audio.items():
        lines.append(f"\n## {audio_name}\n")
        lines.append("| Provider | Status | Latency | Words | Transcript (preview) |")
        lines.append("|----------|--------|---------|-------|----------------------|")
        for row in sorted(rows, key=lambda r: (not r["ok"], r["provider"])):
            if row["skipped"]:
                status = "SKIP"
            elif row["ok"]:
                status = "OK"
            else:
                status = "FAIL"
            preview = (row["transcript"] or row["error"] or "")[:120].replace("|", "/").replace("\n", " ")
            lines.append(
                f"| {row['provider']} | {status} | {row['latency_ms']}ms | {row['word_count']} | {preview} |"
            )

    if missing_keys:
        lines.append("\n## Missing API keys\n")
        for item in missing_keys:
            lines.append(f"- `{item}`\n")

    md_path.write_text("".join(lines), encoding="utf-8")
    print(f"\n[compete] wrote {results_path}")
    print(f"[compete] wrote {md_path}")
    return 0


def main(argv: List[str]) -> int:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--downloads",
        type=Path,
        default=Path("~/Downloads").expanduser(),
        help="Folder with test audio (default: ~/Downloads)",
    )
    parser.add_argument(
        "--audio",
        action="append",
        default=[],
        help="Explicit audio file path (repeatable). Default: all ogg/wav/mp3 in --downloads",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=Path("tools/stt-bench/results/competition"),
        help="Output directory",
    )
    args = parser.parse_args(argv)

    load_dotenv(REPO_ROOT / ".env")

    if args.audio:
        audio_files = [Path(p).expanduser().resolve() for p in args.audio]
    else:
        audio_files = discover_downloads_audio(args.downloads)
        # Prefer ogg if user dropped one in Downloads
        ogg_only = [p for p in audio_files if p.suffix.lower() == ".ogg"]
        if ogg_only:
            audio_files = ogg_only

    if not audio_files:
        print("[compete] no audio files found", file=sys.stderr)
        return 2

    run_id = time.strftime("%Y%m%d-%H%M%S")
    return run_competition(audio_files, args.out_dir / run_id)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
