#!/usr/bin/env python3
"""Benchmark STT providers against retained Said WAV files.

This tool is intentionally eval-only. It reads WAV files and optional SQLite
metadata, then writes JSONL/CSV results. It does not write to Said's DB.
"""

import argparse
import csv
import datetime as dt
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
import wave
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, Iterable, List, Optional, Sequence, Tuple


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_AUDIO_DIRS = [
    Path("~/Library/Application Support/VoicePolish/audio").expanduser(),
    Path("~/Library/Application Support/Said/audio").expanduser(),
]
DEFAULT_DB_PATHS = [
    Path("~/Library/Application Support/VoicePolish/db.sqlite").expanduser(),
    Path("~/Library/Application Support/Said/db.sqlite").expanduser(),
]
BUILTIN_TERMS = [
    "EMIAC",
    "Macobs",
    "Kubernetes",
    "n8n",
    "Perplexity",
    "Claude",
    "JavaScript",
    "TypeScript",
    "Deepgram",
    "OpenAI",
    "Groq",
    "Gemini",
    "React",
    "Tauri",
    "Rust",
    "Python",
    "Docker",
    "Postgres",
    "SQLite",
    "GitHub",
    "Lark",
    "Divo",
    "Testbot",
    "hrm8",
    "Said",
]


@dataclass
class RecordingMeta:
    recording_id: str = ""
    audio_id: str = ""
    timestamp_ms: Optional[int] = None
    transcript: str = ""
    raw_transcript: str = ""
    local_corrected_transcript: str = ""
    polished_output: str = ""
    polished: str = ""
    final_text: str = ""
    recording_seconds: Optional[float] = None
    model_used: str = ""
    confidence: Optional[float] = None

    def reference_text(self) -> str:
        for value in (
            self.final_text,
            self.polished_output,
            self.polished,
            self.local_corrected_transcript,
            self.raw_transcript,
            self.transcript,
        ):
            if value:
                return value
        return ""


@dataclass
class Case:
    wav_path: Path
    audio_id: str
    expected_terms: List[str] = field(default_factory=list)
    note: str = ""
    meta: Optional[RecordingMeta] = None
    duration_s: Optional[float] = None


@dataclass
class ProviderResult:
    provider: str
    ok: bool
    skipped: bool = False
    transcript: str = ""
    latency_ms: Optional[int] = None
    confidence: Optional[float] = None
    model: str = ""
    error: str = ""
    extra: Dict[str, Any] = field(default_factory=dict)


class ProviderSkip(Exception):
    pass


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


def now_run_id() -> str:
    return dt.datetime.now().strftime("%Y%m%d-%H%M%S")


def normalize_text(text: str) -> str:
    text = text.lower()
    chars: List[str] = []
    last_space = True
    for ch in unicodedata.normalize("NFKC", text):
        cat = unicodedata.category(ch)
        if cat[0] in ("L", "N"):
            chars.append(ch)
            last_space = False
        else:
            if not last_space:
                chars.append(" ")
                last_space = True
    return "".join(chars).strip()


def compact_norm(text: str) -> str:
    return normalize_text(text).replace(" ", "")


def token_norms(text: str) -> List[str]:
    return [t for t in normalize_text(text).split() if t]


def levenshtein(a: str, b: str) -> int:
    if a == b:
        return 0
    if not a:
        return len(b)
    if not b:
        return len(a)
    if len(a) < len(b):
        a, b = b, a
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cost = 0 if ca == cb else 1
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost))
        prev = cur
    return prev[-1]


def script_kind(text: str) -> str:
    has_deva = any("\u0900" <= ch <= "\u097f" for ch in text)
    has_latin = any(("a" <= ch.lower() <= "z") for ch in text)
    if has_deva and has_latin:
        return "mixed"
    if has_deva:
        return "devanagari"
    if has_latin:
        return "roman"
    return "other"


def term_match(term: str, transcript: str) -> Dict[str, Any]:
    term_c = compact_norm(term)
    transcript_c = compact_norm(transcript)
    tokens = token_norms(transcript)
    exact = bool(term_c) and term_c in transcript_c
    best_text = ""
    best_distance = len(term_c) if term_c else 0

    max_window = min(4, max(1, len(tokens)))
    for i in range(len(tokens)):
        for width in range(1, max_window + 1):
            window = tokens[i : i + width]
            if not window:
                continue
            candidate = "".join(window)
            if not candidate:
                continue
            dist = levenshtein(term_c, candidate)
            if dist < best_distance:
                best_distance = dist
                best_text = " ".join(window)

    denom = max(1, max(len(term_c), len(compact_norm(best_text))))
    ratio = best_distance / denom
    near = bool(term_c) and not exact and len(term_c) >= 4 and ratio <= 0.34
    return {
        "term": term,
        "exact": exact,
        "near": near,
        "best_match": best_text,
        "edit_distance": best_distance,
        "distance_ratio": round(ratio, 4),
    }


def open_wave_duration(path: Path) -> Optional[float]:
    try:
        with wave.open(str(path), "rb") as wf:
            frames = wf.getnframes()
            rate = wf.getframerate()
            if rate:
                return frames / float(rate)
    except Exception:
        pass
    return None


def load_recordings(db_paths: Sequence[Path]) -> Dict[str, RecordingMeta]:
    by_audio: Dict[str, RecordingMeta] = {}
    for db_path in db_paths:
        if not db_path.is_file():
            continue
        try:
            conn = sqlite3.connect(str(db_path))
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                """
                SELECT id, audio_id, timestamp_ms, transcript, raw_transcript,
                       local_corrected_transcript, polished_output, polished,
                       final_text, recording_seconds, model_used, confidence
                FROM recordings
                WHERE audio_id IS NOT NULL AND audio_id != ''
                """
            ).fetchall()
            for row in rows:
                meta = RecordingMeta(
                    recording_id=row["id"] or "",
                    audio_id=row["audio_id"] or "",
                    timestamp_ms=row["timestamp_ms"],
                    transcript=row["transcript"] or "",
                    raw_transcript=row["raw_transcript"] or "",
                    local_corrected_transcript=row["local_corrected_transcript"] or "",
                    polished_output=row["polished_output"] or "",
                    polished=row["polished"] or "",
                    final_text=row["final_text"] or "",
                    recording_seconds=row["recording_seconds"],
                    model_used=row["model_used"] or "",
                    confidence=row["confidence"],
                )
                keys = {meta.audio_id, meta.audio_id.removeprefix("audio-")}
                for key in keys:
                    if key:
                        by_audio[key] = meta
            conn.close()
        except sqlite3.Error as exc:
            print(f"[warn] could not read DB {db_path}: {exc}", file=sys.stderr)
    return by_audio


def load_vocab_terms(db_paths: Sequence[Path], min_weight: float) -> List[str]:
    terms: Dict[str, str] = {}
    for term in BUILTIN_TERMS:
        terms[compact_norm(term)] = term
    for db_path in db_paths:
        if not db_path.is_file():
            continue
        try:
            conn = sqlite3.connect(str(db_path))
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                """
                SELECT term, weight, source, term_type
                FROM vocabulary
                WHERE term IS NOT NULL AND term != ''
                """
            ).fetchall()
            for row in rows:
                term = row["term"] or ""
                source = (row["source"] or "").lower()
                term_type = (row["term_type"] or "").lower()
                weight = float(row["weight"] or 0)
                protected = source in {"manual", "starred"} or term_type in {
                    "brand",
                    "acronym",
                    "proper_noun",
                    "proper noun",
                    "code_identifier",
                    "code identifier",
                    "manual",
                    "starred",
                }
                if protected or weight >= min_weight:
                    norm = compact_norm(term)
                    if norm:
                        terms[norm] = term
            conn.close()
        except sqlite3.Error as exc:
            print(f"[warn] could not read vocabulary from {db_path}: {exc}", file=sys.stderr)
    return sorted(terms.values(), key=lambda x: x.lower())


def discover_wavs(audio_dirs: Sequence[Path]) -> List[Path]:
    seen = set()
    paths: List[Path] = []
    for audio_dir in audio_dirs:
        if not audio_dir.is_dir():
            continue
        for path in audio_dir.glob("*.wav"):
            resolved = path.resolve()
            if resolved not in seen:
                seen.add(resolved)
                paths.append(resolved)
    paths.sort(key=lambda p: p.stat().st_mtime if p.exists() else 0, reverse=True)
    return paths


def parse_csv_terms(value: str) -> List[str]:
    terms = []
    for term in (value or "").split(","):
        cleaned = term.strip()
        if cleaned:
            terms.append(cleaned)
    return terms


def build_case_for_wav(
    wav: Path,
    recordings: Dict[str, RecordingMeta],
    scoring_terms: Sequence[str],
    explicit_expected: Optional[List[str]] = None,
    note: str = "",
) -> Case:
    audio_id = wav.stem
    lookup_keys = [audio_id, audio_id.removeprefix("audio-")]
    meta = None
    for key in lookup_keys:
        meta = recordings.get(key)
        if meta:
            break
    expected = list(explicit_expected or [])
    if not expected and meta:
        ref = meta.reference_text()
        ref_c = compact_norm(ref)
        for term in scoring_terms:
            term_c = compact_norm(term)
            if term_c and term_c in ref_c:
                expected.append(term)
    deduped: Dict[str, str] = {}
    for term in expected:
        norm = compact_norm(term)
        if norm:
            deduped[norm] = term
    duration = meta.recording_seconds if meta and meta.recording_seconds else open_wave_duration(wav)
    return Case(
        wav_path=wav,
        audio_id=audio_id,
        expected_terms=list(deduped.values()),
        note=note,
        meta=meta,
        duration_s=duration,
    )


def load_manifest(
    manifest: Path,
    wavs: Sequence[Path],
    recordings: Dict[str, RecordingMeta],
    scoring_terms: Sequence[str],
) -> List[Case]:
    by_name = {p.name: p for p in wavs}
    by_stem = {p.stem: p for p in wavs}
    cases: List[Case] = []
    with manifest.open("r", encoding="utf-8") as fh:
        for line_no, raw_line in enumerate(fh, 1):
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            obj = json.loads(line)
            wav_ref = obj.get("wav") or obj.get("path") or obj.get("audio_id")
            if not wav_ref:
                raise ValueError(f"{manifest}:{line_no}: missing wav/path/audio_id")
            wav_path = Path(wav_ref).expanduser()
            if not wav_path.is_file():
                wav_path = by_name.get(wav_ref) or by_stem.get(Path(wav_ref).stem)
            if not wav_path or not Path(wav_path).is_file():
                raise FileNotFoundError(f"{manifest}:{line_no}: cannot find WAV {wav_ref!r}")
            expected = obj.get("expected") or obj.get("expected_terms") or []
            if isinstance(expected, str):
                expected = parse_csv_terms(expected)
            cases.append(
                build_case_for_wav(
                    Path(wav_path),
                    recordings,
                    scoring_terms,
                    explicit_expected=list(expected),
                    note=obj.get("note") or "",
                )
            )
    return cases


def select_cases(cases: List[Case], latest: Optional[int], limit: Optional[int]) -> List[Case]:
    selected = cases
    if latest is not None:
        selected = selected[:latest]
    if limit is not None:
        selected = selected[:limit]
    return selected


def provider_db_raw(case: Case, args: argparse.Namespace) -> ProviderResult:
    if not case.meta:
        raise ProviderSkip("no DB row for audio_id")
    text = case.meta.raw_transcript or case.meta.transcript
    if not text:
        raise ProviderSkip("DB raw transcript is empty")
    return ProviderResult(provider="db_raw", ok=True, transcript=text, model=case.meta.model_used)


def provider_db_local(case: Case, args: argparse.Namespace) -> ProviderResult:
    if not case.meta:
        raise ProviderSkip("no DB row for audio_id")
    text = case.meta.local_corrected_transcript or case.meta.transcript
    if not text:
        raise ProviderSkip("DB local-corrected transcript is empty")
    return ProviderResult(provider="db_local", ok=True, transcript=text, model=case.meta.model_used)


def provider_db_polished(case: Case, args: argparse.Namespace) -> ProviderResult:
    if not case.meta:
        raise ProviderSkip("no DB row for audio_id")
    text = case.meta.final_text or case.meta.polished_output or case.meta.polished
    if not text:
        raise ProviderSkip("DB polished/final text is empty")
    return ProviderResult(provider="db_polished", ok=True, transcript=text, model=case.meta.model_used)


def provider_deepgram_raw(case: Case, args: argparse.Namespace) -> ProviderResult:
    key = os.environ.get("DEEPGRAM_API_KEY", "").strip()
    if not key:
        raise ProviderSkip("DEEPGRAM_API_KEY is not set")
    params = {
        "model": args.deepgram_model,
        "language": args.deepgram_language,
        "smart_format": "false",
        "punctuate": "false",
        "numerals": "false",
    }
    if args.deepgram_extra:
        for pair in args.deepgram_extra:
            if "=" in pair:
                k, v = pair.split("=", 1)
                params[k] = v
    url = "https://api.deepgram.com/v1/listen?" + urllib.parse.urlencode(params)
    data = case.wav_path.read_bytes()
    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "Authorization": f"Token {key}",
            "Content-Type": "audio/wav",
        },
        method="POST",
    )
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=args.provider_timeout_s) as resp:
            body = resp.read()
    except urllib.error.HTTPError as exc:
        details = exc.read().decode("utf-8", errors="replace")[:500]
        raise RuntimeError(f"Deepgram HTTP {exc.code}: {details}")
    latency_ms = int((time.perf_counter() - t0) * 1000)
    payload = json.loads(body.decode("utf-8"))
    alt = (
        payload.get("results", {})
        .get("channels", [{}])[0]
        .get("alternatives", [{}])[0]
    )
    transcript = alt.get("transcript") or ""
    return ProviderResult(
        provider="deepgram_raw",
        ok=bool(transcript),
        transcript=transcript,
        latency_ms=latency_ms,
        confidence=alt.get("confidence"),
        model=args.deepgram_model,
        extra={
            "words": len(alt.get("words") or []),
            "request_id": payload.get("metadata", {}).get("request_id"),
        },
    )


def provider_vosk(case: Case, args: argparse.Namespace) -> ProviderResult:
    model_path = Path(args.vosk_model or os.environ.get("VOSK_MODEL", "")).expanduser()
    if not str(model_path) or not model_path.exists():
        raise ProviderSkip("set --vosk-model or VOSK_MODEL to a local Vosk model directory")
    try:
        import vosk  # type: ignore
    except ImportError:
        raise ProviderSkip("python package 'vosk' is not installed")

    with wave.open(str(case.wav_path), "rb") as wf:
        if wf.getnchannels() != 1 or wf.getsampwidth() != 2:
            raise ProviderSkip("Vosk provider expects mono 16-bit PCM WAV")
        model = get_cached_vosk_model(str(model_path), vosk)
        rec = vosk.KaldiRecognizer(model, wf.getframerate())
        rec.SetWords(False)
        parts: List[str] = []
        t0 = time.perf_counter()
        while True:
            data = wf.readframes(4000)
            if not data:
                break
            if rec.AcceptWaveform(data):
                obj = json.loads(rec.Result())
                if obj.get("text"):
                    parts.append(obj["text"])
        final = json.loads(rec.FinalResult())
        if final.get("text"):
            parts.append(final["text"])
        latency_ms = int((time.perf_counter() - t0) * 1000)
    return ProviderResult(
        provider="vosk",
        ok=bool(parts),
        transcript=" ".join(parts).strip(),
        latency_ms=latency_ms,
        model=str(model_path),
    )


_VOSK_CACHE: Dict[str, Any] = {}


def get_cached_vosk_model(path: str, vosk_module: Any) -> Any:
    if path not in _VOSK_CACHE:
        _VOSK_CACHE[path] = vosk_module.Model(path)
    return _VOSK_CACHE[path]


_FASTER_WHISPER_CACHE: Dict[Tuple[str, str, str], Any] = {}


def provider_faster_whisper(case: Case, args: argparse.Namespace) -> ProviderResult:
    try:
        from faster_whisper import WhisperModel  # type: ignore
    except ImportError:
        raise ProviderSkip("python package 'faster-whisper' is not installed")

    model_name = args.faster_whisper_model
    cache_key = (model_name, args.faster_whisper_device, args.faster_whisper_compute_type)
    if cache_key not in _FASTER_WHISPER_CACHE:
        _FASTER_WHISPER_CACHE[cache_key] = WhisperModel(
            model_name,
            device=args.faster_whisper_device,
            compute_type=args.faster_whisper_compute_type,
        )
    model = _FASTER_WHISPER_CACHE[cache_key]
    t0 = time.perf_counter()
    segments, info = model.transcribe(
        str(case.wav_path),
        language=args.faster_whisper_language or None,
        beam_size=1,
        vad_filter=False,
        word_timestamps=False,
        condition_on_previous_text=False,
    )
    parts = [seg.text.strip() for seg in segments if seg.text.strip()]
    latency_ms = int((time.perf_counter() - t0) * 1000)
    return ProviderResult(
        provider="faster_whisper",
        ok=bool(parts),
        transcript=" ".join(parts),
        latency_ms=latency_ms,
        confidence=None,
        model=model_name,
        extra={"language": getattr(info, "language", None), "language_probability": getattr(info, "language_probability", None)},
    )


def provider_whisper_cpp(case: Case, args: argparse.Namespace) -> ProviderResult:
    binary = args.whisper_cpp_bin or shutil.which("whisper-cli") or shutil.which("main")
    model = args.whisper_cpp_model or os.environ.get("WHISPER_CPP_MODEL", "")
    if not binary:
        raise ProviderSkip("set --whisper-cpp-bin or install whisper-cli")
    if not model or not Path(model).expanduser().is_file():
        raise ProviderSkip("set --whisper-cpp-model or WHISPER_CPP_MODEL")
    model_path = str(Path(model).expanduser())
    with tempfile.TemporaryDirectory(prefix="stt-bench-whisper-") as tmp:
        out_base = str(Path(tmp) / "out")
        cmd = [
            binary,
            "-m",
            model_path,
            "-f",
            str(case.wav_path),
            "-l",
            args.whisper_cpp_language,
            "-otxt",
            "-of",
            out_base,
            "-np",
        ]
        t0 = time.perf_counter()
        proc = subprocess.run(cmd, text=True, capture_output=True, timeout=args.provider_timeout_s)
        latency_ms = int((time.perf_counter() - t0) * 1000)
        if proc.returncode != 0:
            raise RuntimeError((proc.stderr or proc.stdout).strip()[:800])
        out_txt = Path(out_base + ".txt")
        if out_txt.is_file():
            transcript = out_txt.read_text(encoding="utf-8", errors="replace").strip()
        else:
            transcript = clean_whisper_stdout(proc.stdout)
    return ProviderResult(
        provider="whisper_cpp",
        ok=bool(transcript),
        transcript=transcript,
        latency_ms=latency_ms,
        model=model_path,
    )


def clean_whisper_stdout(text: str) -> str:
    lines = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith("[") and "-->" in stripped:
            stripped = stripped.split("]", 1)[-1].strip()
        if stripped and not stripped.startswith("whisper_"):
            lines.append(stripped)
    return " ".join(lines).strip()


PROVIDERS: Dict[str, Callable[[Case, argparse.Namespace], ProviderResult]] = {
    "db_raw": provider_db_raw,
    "db_local": provider_db_local,
    "db_polished": provider_db_polished,
    "deepgram_raw": provider_deepgram_raw,
    "vosk": provider_vosk,
    "faster_whisper": provider_faster_whisper,
    "whisper_cpp": provider_whisper_cpp,
}


def expand_providers(value: str) -> List[str]:
    requested = [item.strip() for item in value.split(",") if item.strip()]
    expanded: List[str] = []
    for item in requested:
        if item == "auto":
            expanded.extend(["db_raw", "db_local", "db_polished"])
            if os.environ.get("DEEPGRAM_API_KEY"):
                expanded.append("deepgram_raw")
            if os.environ.get("VOSK_MODEL"):
                expanded.append("vosk")
            if os.environ.get("WHISPER_CPP_MODEL"):
                expanded.append("whisper_cpp")
        elif item == "db":
            expanded.extend(["db_raw", "db_local", "db_polished"])
        else:
            expanded.append(item)
    deduped = []
    seen = set()
    for item in expanded:
        if item not in seen:
            deduped.append(item)
            seen.add(item)
    unknown = [p for p in deduped if p not in PROVIDERS]
    if unknown:
        raise ValueError(f"unknown providers: {', '.join(unknown)}")
    return deduped


def score_result(case: Case, result: ProviderResult, scoring_terms: Sequence[str]) -> Dict[str, Any]:
    expected = case.expected_terms
    matches = [term_match(term, result.transcript) for term in expected]
    exact_hits = [m["term"] for m in matches if m["exact"]]
    near_hits = [m["term"] for m in matches if m["near"]]
    misses = [m["term"] for m in matches if not m["exact"] and not m["near"]]
    expected_norms = {compact_norm(t) for t in expected}
    hallucinated = []
    transcript_c = compact_norm(result.transcript)
    for term in scoring_terms:
        norm = compact_norm(term)
        if norm and norm not in expected_norms and norm in transcript_c:
            hallucinated.append(term)
    rtf = None
    if result.latency_ms is not None and case.duration_s and case.duration_s > 0:
        rtf = round((result.latency_ms / 1000.0) / case.duration_s, 4)
    return {
        "expected_count": len(expected),
        "exact_hit_count": len(exact_hits),
        "near_hit_count": len(near_hits),
        "miss_count": len(misses),
        "exact_hits": exact_hits,
        "near_hits": near_hits,
        "misses": misses,
        "hallucinated_terms": hallucinated,
        "term_matches": matches,
        "script": script_kind(result.transcript),
        "rtf": rtf,
    }


def result_row(case: Case, result: ProviderResult, score: Dict[str, Any]) -> Dict[str, Any]:
    meta = case.meta
    return {
        "audio_id": case.audio_id,
        "wav_path": str(case.wav_path),
        "recording_id": meta.recording_id if meta else "",
        "timestamp_ms": meta.timestamp_ms if meta else "",
        "duration_s": round(case.duration_s, 3) if case.duration_s else "",
        "provider": result.provider,
        "ok": result.ok,
        "skipped": result.skipped,
        "error": result.error,
        "model": result.model,
        "latency_ms": result.latency_ms if result.latency_ms is not None else "",
        "rtf": score.get("rtf") if score.get("rtf") is not None else "",
        "confidence": result.confidence if result.confidence is not None else "",
        "script": score["script"],
        "expected_terms": "|".join(case.expected_terms),
        "exact_hits": "|".join(score["exact_hits"]),
        "near_hits": "|".join(score["near_hits"]),
        "misses": "|".join(score["misses"]),
        "hallucinated_terms": "|".join(score["hallucinated_terms"]),
        "transcript": result.transcript,
        "note": case.note,
    }


def write_cases(path: Path, cases: Sequence[Case]) -> None:
    with path.open("w", encoding="utf-8") as fh:
        for case in cases:
            meta = case.meta
            obj = {
                "audio_id": case.audio_id,
                "wav_path": str(case.wav_path),
                "duration_s": case.duration_s,
                "expected_terms": case.expected_terms,
                "note": case.note,
                "recording_id": meta.recording_id if meta else None,
                "timestamp_ms": meta.timestamp_ms if meta else None,
                "db_raw_transcript": (meta.raw_transcript or meta.transcript) if meta else "",
                "db_local_corrected_transcript": meta.local_corrected_transcript if meta else "",
                "db_polished_output": (meta.final_text or meta.polished_output or meta.polished) if meta else "",
            }
            fh.write(json.dumps(obj, ensure_ascii=False) + "\n")


def write_jsonl(path: Path, rows: Sequence[Dict[str, Any]]) -> None:
    with path.open("w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False) + "\n")


def write_csv(path: Path, rows: Sequence[Dict[str, Any]]) -> None:
    if not rows:
        path.write_text("", encoding="utf-8")
        return
    with path.open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=list(rows[0].keys()))
        writer.writeheader()
        writer.writerows(rows)


def summarize(rows: Sequence[Dict[str, Any]]) -> List[Dict[str, Any]]:
    grouped: Dict[str, List[Dict[str, Any]]] = {}
    for row in rows:
        grouped.setdefault(row["provider"], []).append(row)
    summary = []
    for provider, provider_rows in sorted(grouped.items()):
        ok_rows = [r for r in provider_rows if r["ok"] is True]
        latencies = [int(r["latency_ms"]) for r in ok_rows if str(r["latency_ms"]).isdigit()]
        rtfs = [float(r["rtf"]) for r in ok_rows if r["rtf"] != ""]
        expected_total = sum(len(split_pipe(r["expected_terms"])) for r in ok_rows)
        exact_total = sum(len(split_pipe(r["exact_hits"])) for r in ok_rows)
        near_total = sum(len(split_pipe(r["near_hits"])) for r in ok_rows)
        miss_total = sum(len(split_pipe(r["misses"])) for r in ok_rows)
        halluc_total = sum(len(split_pipe(r["hallucinated_terms"])) for r in ok_rows)
        summary.append(
            {
                "provider": provider,
                "rows": len(provider_rows),
                "ok": len(ok_rows),
                "skipped": sum(1 for r in provider_rows if r["skipped"] is True),
                "errors": sum(1 for r in provider_rows if not r["ok"] and not r["skipped"]),
                "expected_terms": expected_total,
                "exact_hits": exact_total,
                "near_hits": near_total,
                "misses": miss_total,
                "hallucinated_terms": halluc_total,
                "avg_latency_ms": round(sum(latencies) / len(latencies), 1) if latencies else "",
                "avg_rtf": round(sum(rtfs) / len(rtfs), 4) if rtfs else "",
            }
        )
    return summary


def split_pipe(value: str) -> List[str]:
    return [v for v in str(value).split("|") if v]


def print_summary(summary_rows: Sequence[Dict[str, Any]], out_dir: Path) -> None:
    print(f"[stt-bench] wrote {out_dir}")
    if not summary_rows:
        print("[stt-bench] no rows")
        return
    headers = [
        "provider",
        "ok",
        "skipped",
        "errors",
        "expected_terms",
        "exact_hits",
        "near_hits",
        "misses",
        "hallucinated_terms",
        "avg_latency_ms",
        "avg_rtf",
    ]
    widths = {h: len(h) for h in headers}
    for row in summary_rows:
        for h in headers:
            widths[h] = max(widths[h], len(str(row.get(h, ""))))
    print("  " + "  ".join(h.ljust(widths[h]) for h in headers))
    for row in summary_rows:
        print("  " + "  ".join(str(row.get(h, "")).ljust(widths[h]) for h in headers))


def run_provider(provider: str, case: Case, args: argparse.Namespace) -> ProviderResult:
    fn = PROVIDERS[provider]
    try:
        result = fn(case, args)
        result.provider = provider
        if not result.ok and not result.error:
            result.error = "empty transcript"
        return result
    except ProviderSkip as exc:
        return ProviderResult(provider=provider, ok=False, skipped=True, error=str(exc))
    except subprocess.TimeoutExpired:
        return ProviderResult(provider=provider, ok=False, error=f"provider timed out after {args.provider_timeout_s}s")
    except Exception as exc:
        return ProviderResult(provider=provider, ok=False, error=str(exc))


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be > 0")
    return parsed


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--providers", default="auto", help="Comma-separated providers: auto, db, db_raw, db_local, db_polished, deepgram_raw, vosk, faster_whisper, whisper_cpp")
    parser.add_argument("--audio-dir", action="append", default=[], help="Directory containing WAV files. Can be passed multiple times.")
    parser.add_argument("--db", action="append", default=[], help="SQLite DB path. Can be passed multiple times.")
    parser.add_argument("--manifest", type=Path, help="JSONL manifest with wav/audio_id and expected terms.")
    parser.add_argument("--terms", default="", help="Comma-separated terms to score. Defaults to protected vocab + built-ins.")
    parser.add_argument("--min-vocab-weight", type=float, default=8.0, help="Include DB vocab terms with at least this weight.")
    parser.add_argument("--latest", type=positive_int, help="Use latest N WAVs after sorting by mtime.")
    parser.add_argument("--limit", type=positive_int, help="Hard limit after manifest/discovery selection.")
    parser.add_argument("--out-dir", type=Path, default=Path("tools/stt-bench/results"), help="Base output directory.")
    parser.add_argument("--run-id", default=now_run_id(), help="Run id / output subdirectory name.")
    parser.add_argument("--provider-timeout-s", type=int, default=120)
    parser.add_argument("--sleep-s", type=float, default=0.0, help="Sleep between provider network calls.")
    parser.add_argument("--dry-run", action="store_true", help="Only discover cases and write cases.jsonl; do not run providers.")

    parser.add_argument("--deepgram-model", default="nova-3")
    parser.add_argument("--deepgram-language", default="multi")
    parser.add_argument("--deepgram-extra", action="append", default=[], help="Extra Deepgram query param as key=value.")

    parser.add_argument("--vosk-model", default="")

    parser.add_argument("--faster-whisper-model", default="small")
    parser.add_argument("--faster-whisper-device", default="auto")
    parser.add_argument("--faster-whisper-compute-type", default="auto")
    parser.add_argument("--faster-whisper-language", default="hi")

    parser.add_argument("--whisper-cpp-bin", default="")
    parser.add_argument("--whisper-cpp-model", default="")
    parser.add_argument("--whisper-cpp-language", default="hi")
    return parser.parse_args(argv)


def main(argv: Sequence[str]) -> int:
    args = parse_args(argv)
    load_dotenv(REPO_ROOT / ".env")

    audio_dirs = [Path(p).expanduser() for p in args.audio_dir] if args.audio_dir else DEFAULT_AUDIO_DIRS
    db_paths = [Path(p).expanduser() for p in args.db] if args.db else DEFAULT_DB_PATHS
    recordings = load_recordings(db_paths)
    scoring_terms = load_vocab_terms(db_paths, args.min_vocab_weight)
    for term in parse_csv_terms(args.terms):
        scoring_terms.append(term)
    deduped_terms: Dict[str, str] = {}
    for term in scoring_terms:
        norm = compact_norm(term)
        if norm:
            deduped_terms[norm] = term
    scoring_terms = sorted(deduped_terms.values(), key=lambda x: x.lower())

    wavs = discover_wavs(audio_dirs)
    if not wavs:
        print("[stt-bench] no WAV files found", file=sys.stderr)
        return 2
    if args.manifest:
        cases = load_manifest(args.manifest, wavs, recordings, scoring_terms)
    else:
        cases = [build_case_for_wav(wav, recordings, scoring_terms) for wav in wavs]
    cases = select_cases(cases, args.latest, args.limit)
    if not cases:
        print("[stt-bench] no cases selected", file=sys.stderr)
        return 2

    out_dir = args.out_dir / args.run_id
    out_dir.mkdir(parents=True, exist_ok=True)
    write_cases(out_dir / "cases.jsonl", cases)

    providers = expand_providers(args.providers)
    print(
        f"[stt-bench] cases={len(cases)} providers={','.join(providers)} "
        f"terms={len(scoring_terms)} out={out_dir}"
    )
    if args.dry_run:
        print(f"[stt-bench] dry run wrote {out_dir / 'cases.jsonl'}")
        return 0

    rows: List[Dict[str, Any]] = []
    detailed_rows: List[Dict[str, Any]] = []
    total = len(cases) * len(providers)
    completed = 0
    for case in cases:
        for provider in providers:
            completed += 1
            print(f"[{completed}/{total}] {provider} {case.audio_id}", flush=True)
            result = run_provider(provider, case, args)
            score = score_result(case, result, scoring_terms)
            row = result_row(case, result, score)
            rows.append(row)
            detailed = dict(row)
            detailed["term_matches"] = score["term_matches"]
            detailed["provider_extra"] = result.extra
            detailed_rows.append(detailed)
            if args.sleep_s > 0 and provider in {"deepgram_raw"}:
                time.sleep(args.sleep_s)

    write_jsonl(out_dir / "results.jsonl", detailed_rows)
    write_csv(out_dir / "results.csv", rows)
    summary_rows = summarize(rows)
    write_csv(out_dir / "provider_summary.csv", summary_rows)
    print_summary(summary_rows, out_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
