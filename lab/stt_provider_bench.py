#!/usr/bin/env python3
"""Benchmark OpenAI gpt-4o-mini-transcribe against ElevenLabs Scribe v2.

The baseline is intentionally unbiased: neither provider receives language hints,
prompt text, nor keyterms.

    python lab/stt_provider_bench.py
    python lab/stt_provider_bench.py --limit 1
    python lab/stt_provider_bench.py --output ~/Downloads/my-stt-run
"""

from __future__ import annotations

import argparse
import csv
import json
import mimetypes
import os
import statistics
import time
import unicodedata
import wave
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import requests


REPO = Path(__file__).resolve().parent.parent
DOWNLOADS = Path.home() / "Downloads"
OPENAI_URL = "https://api.openai.com/v1/audio/transcriptions"
ELEVENLABS_URL = "https://api.elevenlabs.io/v1/speech-to-text"

PROVIDERS = {
    "openai": {
        "label": "OpenAI GPT-4o mini Transcribe",
        "model": "gpt-4o-mini-transcribe",
    },
    "elevenlabs": {
        "label": "ElevenLabs Scribe v2",
        "model": "scribe_v2",
    },
}

REFERENCE_TRANSCRIPTS = {
    "said-2026-06-22-2327-58-words.wav": (
        "Okay suno zara — humne Caps Lock dictation release kiya hai, par Swift "
        "local STT slow lag raha hai aur polish latency around sixteen hundred "
        "milliseconds aa rahi hai. Server runtime on hai, DeepInfra Maverick test "
        "karna hai. Docker rebuild karo, SQLite migration check karo, webhook retry "
        "fix karo, aur Sentry mein run id daalo. Phir PR merge karo."
    ),
}

TERM_PROFILES = {
    "said-2026-06-22-2327-58-words.wav": [
        "Caps Lock",
        "Swift",
        "STT",
        "DeepInfra",
        "Docker",
        "SQLite",
        "webhook",
        "Sentry",
        "PR",
    ],
    "said-2026-06-23-0105-53-words.wav": [
        "Caps Lock",
        "Local speech",
        "Sentry",
        "ZooKeeper",
        "Kafka",
    ],
    "said-2026-06-23-0116-49-words.wav": [
        "Docker",
        "SQLite",
        "webhook",
        "PR",
    ],
    "said-2026-06-23-0118-49-words.wav": [
        "Google Ads",
        "Meta Ads",
        "CPA",
        "landing page",
    ],
}


def load_dotenv(path: Path) -> None:
    if not path.is_file():
        return
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
            value = value[1:-1]
        if key:
            os.environ.setdefault(key, value)


def wav_duration(path: Path) -> float:
    with wave.open(str(path), "rb") as wav:
        return wav.getnframes() / wav.getframerate()


def normalize_words(text: str) -> list[str]:
    normalized = unicodedata.normalize("NFKC", text).casefold()
    words: list[str] = []
    current: list[str] = []
    for char in normalized:
        if unicodedata.category(char)[0] in {"L", "M", "N"}:
            current.append(char)
        elif current:
            words.append("".join(current))
            current = []
    if current:
        words.append("".join(current))
    return words


def edit_distance(left: list[str], right: list[str]) -> int:
    previous = list(range(len(right) + 1))
    for i, left_item in enumerate(left, start=1):
        current = [i]
        for j, right_item in enumerate(right, start=1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[j] + 1,
                    previous[j - 1] + (left_item != right_item),
                )
            )
        previous = current
    return previous[-1]


def word_error_rate(reference: str, hypothesis: str) -> float:
    reference_words = normalize_words(reference)
    return edit_distance(reference_words, normalize_words(hypothesis)) / max(
        len(reference_words), 1
    )


def normalized_term(term: str) -> str:
    return " ".join(normalize_words(term))


def term_hits(text: str, expected: list[str]) -> list[str]:
    haystack = f" {' '.join(normalize_words(text))} "
    return [
        term
        for term in expected
        if f" {normalized_term(term)} " in haystack
    ]


def error_message(response: requests.Response) -> str:
    body = response.text.strip().replace("\n", " ")
    return f"HTTP {response.status_code}: {body[:500]}"


def transcribe_openai(path: Path, api_key: str) -> dict[str, Any]:
    content_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
    started = time.perf_counter()
    with path.open("rb") as audio:
        response = requests.post(
            OPENAI_URL,
            headers={"Authorization": f"Bearer {api_key}"},
            files={"file": (path.name, audio, content_type)},
            data={
                "model": PROVIDERS["openai"]["model"],
                "response_format": "json",
                "temperature": "0",
            },
            timeout=180,
        )
    latency_s = time.perf_counter() - started
    if not response.ok:
        raise RuntimeError(error_message(response))
    payload = response.json()
    return {"text": str(payload.get("text", "")).strip(), "latency_s": latency_s}


def transcribe_elevenlabs(path: Path, api_key: str) -> dict[str, Any]:
    content_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
    started = time.perf_counter()
    with path.open("rb") as audio:
        response = requests.post(
            ELEVENLABS_URL,
            headers={"xi-api-key": api_key},
            files={"file": (path.name, audio, content_type)},
            data={
                "model_id": PROVIDERS["elevenlabs"]["model"],
                "diarize": "false",
                "tag_audio_events": "false",
            },
            timeout=180,
        )
    latency_s = time.perf_counter() - started
    if not response.ok:
        raise RuntimeError(error_message(response))
    payload = response.json()
    return {
        "text": str(payload.get("text", "")).strip(),
        "latency_s": latency_s,
        "language_code": payload.get("language_code"),
        "language_probability": payload.get("language_probability"),
    }


def percentile(values: list[float], quantile: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    position = (len(ordered) - 1) * quantile
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return ordered[lower] * (1 - fraction) + ordered[upper] * fraction


def write_outputs(output_dir: Path, payload: dict[str, Any]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    (output_dir / "results.json").write_text(
        json.dumps(payload, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    with (output_dir / "results.csv").open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(
            handle,
            fieldnames=[
                "wav",
                "duration_s",
                "provider",
                "model",
                "ok",
                "latency_s",
                "realtime_factor",
                "word_count",
                "wer",
                "term_hits",
                "term_total",
                "text",
                "error",
            ],
        )
        writer.writeheader()
        for row in payload["results"]:
            writer.writerow(
                {
                    **{key: row.get(key) for key in writer.fieldnames},
                    "term_hits": len(row.get("term_hits", [])),
                }
            )


def summarize(results: list[dict[str, Any]]) -> dict[str, Any]:
    summary: dict[str, Any] = {}
    for provider in PROVIDERS:
        rows = [row for row in results if row["provider"] == provider and row["ok"]]
        latencies = [float(row["latency_s"]) for row in rows]
        rtfs = [float(row["realtime_factor"]) for row in rows]
        reference_rows = [row for row in rows if row.get("wer") is not None]
        rubric_rows = [row for row in rows if row.get("term_total")]
        hits = sum(len(row["term_hits"]) for row in rubric_rows)
        terms = sum(int(row["term_total"]) for row in rubric_rows)
        summary[provider] = {
            **PROVIDERS[provider],
            "successful": len(rows),
            "failed": len(
                [
                    row
                    for row in results
                    if row["provider"] == provider and not row["ok"]
                ]
            ),
            "median_latency_s": statistics.median(latencies) if latencies else None,
            "p95_latency_s": percentile(latencies, 0.95) if latencies else None,
            "mean_realtime_factor": statistics.mean(rtfs) if rtfs else None,
            "reference_mean_wer": (
                statistics.mean(float(row["wer"]) for row in reference_rows)
                if reference_rows
                else None
            ),
            "technical_term_hits": hits,
            "technical_term_total": terms,
            "technical_term_recall": hits / terms if terms else None,
        }

    disagreements: list[dict[str, Any]] = []
    for wav_name in sorted({row["wav"] for row in results}):
        rows = {
            row["provider"]: row
            for row in results
            if row["wav"] == wav_name and row["ok"]
        }
        if set(rows) != set(PROVIDERS):
            continue
        left = normalize_words(rows["openai"]["text"])
        right = normalize_words(rows["elevenlabs"]["text"])
        disagreement = edit_distance(left, right) / max(len(left), len(right), 1)
        disagreements.append({"wav": wav_name, "word_disagreement_rate": disagreement})
    summary["pairwise"] = {
        "mean_word_disagreement_rate": (
            statistics.mean(row["word_disagreement_rate"] for row in disagreements)
            if disagreements
            else None
        ),
        "clips": disagreements,
    }
    return summary


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--downloads",
        type=Path,
        default=DOWNLOADS,
        help="Directory containing said-*.wav and airnote-*.wav",
    )
    parser.add_argument("--limit", type=int, help="Only run the first N clips")
    parser.add_argument("--output", type=Path, help="Output directory")
    parser.add_argument(
        "--recalculate",
        type=Path,
        help="Recalculate derived metrics in an existing results.json without API calls",
    )
    args = parser.parse_args()

    if args.recalculate:
        results_path = args.recalculate.expanduser()
        payload = json.loads(results_path.read_text(encoding="utf-8"))
        for row in payload["results"]:
            if not row["ok"]:
                continue
            expected_terms = TERM_PROFILES.get(row["wav"], [])
            reference = REFERENCE_TRANSCRIPTS.get(row["wav"])
            row["word_count"] = len(normalize_words(row["text"]))
            row["wer"] = (
                word_error_rate(reference, row["text"]) if reference else None
            )
            row["term_hits"] = term_hits(row["text"], expected_terms)
            row["term_total"] = len(expected_terms)
        payload["summary"] = summarize(payload["results"])
        write_outputs(results_path.parent, payload)
        print(json.dumps(payload["summary"], indent=2))
        print(f"Recalculated: {results_path}")
        return

    load_dotenv(REPO / ".env")
    openai_key = os.environ.get("OPENAI_API_KEY", "").strip()
    elevenlabs_key = (
        os.environ.get("ELEVEN_LABS_API_KEY", "").strip()
        or os.environ.get("ELEVENLABS_API_KEY", "").strip()
    )
    missing = [
        name
        for name, value in [
            ("OPENAI_API_KEY", openai_key),
            ("ELEVEN_LABS_API_KEY", elevenlabs_key),
        ]
        if not value
    ]
    if missing:
        raise SystemExit(f"Missing required key(s): {', '.join(missing)}")

    wavs = sorted(
        set(args.downloads.glob("said-*.wav"))
        | set(args.downloads.glob("airnote-*.wav"))
    )
    if args.limit is not None:
        wavs = wavs[: args.limit]
    if not wavs:
        raise SystemExit(f"No AirNote WAV files found in {args.downloads}")

    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output_dir = (
        args.output.expanduser()
        if args.output
        else args.downloads / f"AirNote-STT-Benchmark-{run_id}"
    )
    metadata = {
        "run_id": run_id,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "dataset_dir": str(args.downloads.resolve()),
        "output_dir": str(output_dir.resolve()),
        "baseline": "No prompt, language hint, or keyterms supplied to either provider.",
        "providers": PROVIDERS,
        "clips": len(wavs),
        "total_audio_s": sum(wav_duration(path) for path in wavs),
    }
    payload: dict[str, Any] = {"metadata": metadata, "results": [], "summary": {}}
    write_outputs(output_dir, payload)

    total_calls = len(wavs) * len(PROVIDERS)
    call_number = 0
    for clip_index, path in enumerate(wavs):
        duration_s = wav_duration(path)
        provider_order = (
            ["openai", "elevenlabs"]
            if clip_index % 2 == 0
            else ["elevenlabs", "openai"]
        )
        for provider in provider_order:
            call_number += 1
            label = PROVIDERS[provider]["label"]
            print(
                f"[{call_number}/{total_calls}] {label}: {path.name} "
                f"({duration_s:.1f}s)",
                flush=True,
            )
            base = {
                "wav": path.name,
                "path": str(path.resolve()),
                "duration_s": duration_s,
                "provider": provider,
                "model": PROVIDERS[provider]["model"],
            }
            try:
                response = (
                    transcribe_openai(path, openai_key)
                    if provider == "openai"
                    else transcribe_elevenlabs(path, elevenlabs_key)
                )
                text = response.pop("text")
                expected_terms = TERM_PROFILES.get(path.name, [])
                reference = REFERENCE_TRANSCRIPTS.get(path.name)
                latency_s = float(response["latency_s"])
                row = {
                    **base,
                    **response,
                    "ok": True,
                    "text": text,
                    "word_count": len(normalize_words(text)),
                    "realtime_factor": latency_s / max(duration_s, 0.001),
                    "reference": reference,
                    "wer": word_error_rate(reference, text) if reference else None,
                    "term_hits": term_hits(text, expected_terms),
                    "term_total": len(expected_terms),
                    "error": None,
                }
                print(
                    f"  {latency_s:.2f}s | {row['word_count']} words | "
                    f"{text[:100]}",
                    flush=True,
                )
            except (OSError, ValueError, requests.RequestException, RuntimeError) as exc:
                row = {
                    **base,
                    "ok": False,
                    "error": str(exc),
                    "text": "",
                    "word_count": 0,
                    "latency_s": None,
                    "realtime_factor": None,
                    "reference": REFERENCE_TRANSCRIPTS.get(path.name),
                    "wer": None,
                    "term_hits": [],
                    "term_total": len(TERM_PROFILES.get(path.name, [])),
                }
                print(f"  ERROR: {exc}", flush=True)
            payload["results"].append(row)
            payload["summary"] = summarize(payload["results"])
            write_outputs(output_dir, payload)

    print(json.dumps(payload["summary"], indent=2), flush=True)
    print(f"Results: {output_dir}", flush=True)


if __name__ == "__main__":
    main()
