#!/usr/bin/env python3
"""Batch polish benchmark — all catalog models × WAV clips in ~/Downloads.

Uses cached Swift STT transcripts, production system prompt, per-clip profile scoring.

  python lab/batch_compare_two_models.py
  python lab/batch_compare_two_models.py --default-set
  python lab/batch_compare_two_models.py --slug fast,groq-scout,phi4,di-scout
  python lab/batch_compare_two_models.py --profile-clips-only
  python lab/batch_compare_two_models.py --dry-run
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LAB = Path(__file__).resolve().parent
REPO = LAB.parent
if str(LAB) not in sys.path:
    sys.path.insert(0, str(LAB))

import polish_lab
from model_catalog import (
    BENCHMARK_DEFAULT_SLUGS,
    LAB_MODEL_CATALOG,
    ROUND1_WINNERS,
    available_lab_routes,
)
from production_prompt import render_production_system_prompt

OUT_DIR = LAB / "model_runs"
PRIOR_BATCH = LAB / "model_runs/batch_20260622T201035Z/batch_results.json"
DOWNLOADS = Path.home() / "Downloads"

# Per-clip expected recoveries + garbles that should not survive polish.
WAV_PROFILES: dict[str, dict[str, list[str]]] = {
    "said-2026-06-22-2327-58-words.wav": {
        "expected": [
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
        "bad": [
            "sonoo",
            "app slot",
            "STD",
            "deep infra",
            "doctor rebuild",
            "CQLite",
            "webbook",
            "century",
            "memory test",
        ],
    },
    "said-2026-06-23-0105-53-words.wav": {
        "expected": ["Caps Lock", "Deepgram", "Sentry", "ZooKeeper", "Kafka"],
        "bad": ["zooki", "cabslock", "century", "estulate", "deep gram"],
    },
    "said-2026-06-23-0116-49-words.wav": {
        "expected": ["Docker", "SQLite", "webhook", "PR"],
        "bad": ["doctor hain", "webbook", "sanhattar", "road cost"],
    },
    "said-2026-06-23-0118-49-words.wav": {
        "expected": ["Google Ads", "Meta Ads", "CPA", "landing page"],
        "bad": ["Citya", "ashok", "vikli", "porting dashboard"],
    },
}


def score_clip_generic(text: str, transcript: str) -> dict[str, Any]:
    """Heuristic score when no per-clip rubric exists (new WAVs)."""
    lower = text.lower()
    preamble = lower.lstrip().startswith(
        ("here", "sure", "the polished", "output:", "polished:")
    )
    non_latin = any(ord(ch) > 127 for ch in text)
    thinking = "<think>" in lower
    length_ratio = len(text) / max(len(transcript), 1)
    # Reward modest cleanup; penalize huge rewrites or heavy truncation.
    score = 10
    score -= int(preamble) * 4
    score -= int(non_latin) * 3
    score -= int(thinking) * 25
    if length_ratio < 0.45 or length_ratio > 2.2:
        score -= 10
    elif length_ratio < 0.7 or length_ratio > 1.6:
        score -= 4
    return {
        "score": score,
        "expected": [],
        "hits": [],
        "missing": [],
        "bad": [],
        "preamble": preamble,
        "non_latin": non_latin,
        "thinking_leak": thinking,
        "length_ratio": round(length_ratio, 2),
        "generic": True,
    }


def score_clip(wav_name: str, text: str, transcript: str) -> dict[str, Any]:
    lower = text.lower()
    profile = WAV_PROFILES.get(wav_name)
    if not profile:
        return score_clip_generic(text, transcript)
    expected = profile.get("expected", [])
    bad_list = profile.get("bad", [])

    hits = [t for t in expected if t.lower() in lower]
    missing = [t for t in expected if t.lower() not in lower]
    bad_hits = [b for b in bad_list if b.lower() in lower]

    preamble = lower.lstrip().startswith(
        ("here", "sure", "the polished", "output:", "polished:")
    )
    non_latin = any(ord(ch) > 127 for ch in text)
    thinking = "<think>" in lower
    length_ratio = len(text) / max(len(transcript), 1)

    score = (
        len(hits) * 3
        - len(bad_hits) * 4
        - int(preamble) * 4
        - int(non_latin) * 3
        - int(thinking) * 25
    )
    if length_ratio > 2.5 or length_ratio < 0.35:
        score -= 8

    return {
        "score": score,
        "expected": expected,
        "hits": hits,
        "missing": missing,
        "bad": bad_hits,
        "preamble": preamble,
        "non_latin": non_latin,
        "thinking_leak": thinking,
        "length_ratio": round(length_ratio, 2),
    }


def load_transcripts(
    *,
    refresh_stt: bool,
    wav_filter: set[str] | None = None,
) -> list[dict[str, Any]]:
    wavs = sorted(DOWNLOADS.glob("said-*.wav"))
    if wav_filter:
        wavs = [w for w in wavs if w.name in wav_filter]
    if not wavs:
        hint = f" (filter={sorted(wav_filter)})" if wav_filter else ""
        raise SystemExit(f"No said-*.wav in {DOWNLOADS}{hint}")

    prior: dict[str, str] = {}
    if PRIOR_BATCH.is_file():
        data = json.loads(PRIOR_BATCH.read_text(encoding="utf-8"))
        for item in data.get("transcripts", []):
            prior[item["wav"]] = item["transcript"]
    # Also reuse transcripts from any prior batch_full run.
    for path in sorted(OUT_DIR.glob("batch_full_*/batch_results.json"), reverse=True):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            continue
        for item in data.get("transcripts", []):
            prior.setdefault(item["wav"], item["transcript"])

    rows: list[dict[str, Any]] = []
    for wav in wavs:
        if refresh_stt or wav.name not in prior:
            print(f"STT {wav.name}...", flush=True)
            transcript, stt_s = polish_lab.transcribe_swift(wav)
        else:
            transcript = prior[wav.name]
            stt_s = None
        rows.append(
            {
                "wav": wav.name,
                "path": str(wav),
                "transcript": transcript,
                "words": len(transcript.split()),
                "stt_s": stt_s,
            }
        )
    return rows


def resolve_routes(
    *,
    slugs: set[str] | None,
    providers: set[str] | None,
) -> list[dict[str, Any]]:
    polish_lab.load_dotenv()
    routes = available_lab_routes(LAB_MODEL_CATALOG, providers=providers, slugs=slugs)
    if not routes:
        raise SystemExit(
            "No benchmark routes available. Set DEEPINFRA_API_KEY and/or GROQ_API_KEY in .env"
        )
    return routes


GROQ_COOLDOWN = {"default": 5.0, "oss": 25.0}
_last_groq_call: float = 0.0


def groq_rate_limit_sleep(route: dict[str, Any]) -> None:
    """Space Groq requests to avoid TPM / burst 429s (sequential runs only)."""
    global _last_groq_call
    if route.get("provider") != "groq":
        return
    model = str(route.get("model", ""))
    cooldown = GROQ_COOLDOWN["oss"] if "gpt-oss" in model else GROQ_COOLDOWN["default"]
    now = time.monotonic()
    elapsed = now - _last_groq_call
    if _last_groq_call > 0 and elapsed < cooldown:
        wait = cooldown - elapsed
        print(f"  [groq cooldown] {route.get('slug')}: sleeping {wait:.1f}s", flush=True)
        time.sleep(wait)
    _last_groq_call = time.monotonic()


def run_job(
    *,
    transcript_row: dict[str, Any],
    route: dict[str, str],
    system_prompt: str,
) -> dict[str, Any]:
    wav = transcript_row["wav"]
    transcript = transcript_row["transcript"]
    t0 = time.perf_counter()
    result = polish_lab.polish_try(transcript, system_prompt, route)
    wall_s = time.perf_counter() - t0
    base = {
        "wav": wav,
        "slug": route["slug"],
        "label": route["label"],
        "provider": route["provider"],
        "model": route["model"],
        "wall_s": wall_s,
    }
    if not result["ok"]:
        return {**base, "ok": False, "error": result["error"], "score": -999}
    polished = str(result["polished"])
    metrics = score_clip(wav, polished, transcript)
    return {
        **base,
        "ok": True,
        "error": None,
        "polished": polished,
        "polish_s": result["polish_s"],
        **metrics,
    }


def write_report(
    *,
    stamp: str,
    run_dir: Path,
    transcripts: list[dict[str, Any]],
    results: list[dict[str, Any]],
    prompt_note: str,
    model_slugs: list[str],
) -> None:
    by_slug: dict[str, list[dict[str, Any]]] = {}
    for r in results:
        by_slug.setdefault(str(r["slug"]), []).append(r)

    summary: list[tuple[str, float, float, int, int]] = []
    for slug, rows in by_slug.items():
        ok = [r for r in rows if r.get("ok")]
        if not ok:
            summary.append((slug, -999.0, -1.0, 0, len(rows)))
            continue
        avg_score = statistics.mean(float(r["score"]) for r in ok)
        avg_lat = statistics.mean(float(r["wall_s"]) for r in ok)
        summary.append((slug, avg_score, avg_lat, len(ok), len(rows)))

    summary.sort(key=lambda x: x[1], reverse=True)

    lines = [
        f"# Full model batch benchmark — {stamp}",
        "",
        f"- WAV files: {len(transcripts)}",
        f"- Models ({len(model_slugs)}): {', '.join(f'`{s}`' for s in model_slugs)}",
        f"- Prompt: {prompt_note}",
        "- Scoring: per-clip profile term recovery + garble/thinking/preamble penalties",
        "",
        "## Overall Ranking",
        "",
        "| Rank | Model | OK | Avg Score | Avg Latency | Balanced |",
        "|---:|---|---:|---:|---:|---:|",
    ]
    for rank, (slug, avg_score, avg_lat, ok_n, total) in enumerate(summary, 1):
        balanced = avg_score - (avg_lat * 2.5 if avg_lat >= 0 else 0)
        lat_s = f"{avg_lat:.2f}s" if avg_lat >= 0 else "—"
        lines.append(
            f"| {rank} | `{slug}` | {ok_n}/{total} | {avg_score:.1f} | {lat_s} | {balanced:.1f} |"
        )

    lines += ["", "## Transcripts", ""]
    for row in transcripts:
        lines += [
            f"### {row['wav']}",
            f"- Words: {row['words']}",
            "",
            row["transcript"],
            "",
        ]

    scored_wavs = [w for w in WAV_PROFILES if any(t["wav"] == w for t in transcripts)]
    if scored_wavs:
        lines += ["## Per-WAV Results (profile clips)", ""]
        for wav in scored_wavs:
            lines += [f"### {wav}", ""]
            wav_rows = [r for r in results if r["wav"] == wav and r.get("ok")]
            wav_rows.sort(key=lambda r: int(r.get("score", -999)), reverse=True)
            lines.append(
                "| Rank | Model | Score | Latency | Hits | Missing | Bad |"
            )
            lines.append("|---:|---|---:|---:|---|---|---|")
            for rank, r in enumerate(wav_rows, 1):
                lines.append(
                    f"| {rank} | `{r['slug']}` | {r['score']} | {r['wall_s']:.2f}s | "
                    f"{len(r.get('hits', []))} | {', '.join(r.get('missing', [])) or '-'} | "
                    f"{', '.join(r.get('bad', [])) or '-'} |"
                )
            lines.append("")
            for r in wav_rows:
                lines += [
                    f"#### `{r['slug']}` — {r['score']} pts",
                    "",
                    str(r.get("polished", "")),
                    "",
                ]

    (run_dir / "batch_report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--slug",
        default="",
        help="Comma-separated catalog slugs (default: all with API keys)",
    )
    parser.add_argument(
        "--default-set",
        action="store_true",
        help=f"Run focused set: {', '.join(BENCHMARK_DEFAULT_SLUGS)}",
    )
    parser.add_argument(
        "--round1-winners",
        action="store_true",
        help=f"Run round-1 top 5: {', '.join(ROUND1_WINNERS)}",
    )
    parser.add_argument(
        "--wav",
        default="",
        help="Comma-separated WAV basenames in ~/Downloads (default: all said-*.wav)",
    )
    parser.add_argument(
        "--provider",
        choices=("groq", "cerebras", "deepinfra"),
        action="append",
        help="Limit to provider(s)",
    )
    parser.add_argument(
        "--profile-clips-only",
        action="store_true",
        help="Only WAVs with scoring profiles (4 dev clips)",
    )
    parser.add_argument(
        "--groq-cooldown",
        type=float,
        default=5.0,
        help="Seconds between non-OSS Groq calls (default 5)",
    )
    parser.add_argument(
        "--groq-oss-cooldown",
        type=float,
        default=25.0,
        help="Seconds between Groq GPT-OSS calls (default 25)",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=1,
        help="Parallel jobs (default 1 — keep 1 for Groq cooldown)",
    )
    parser.add_argument("--refresh-stt", action="store_true", help="Re-run Swift STT on WAVs")
    parser.add_argument("--dry-run", action="store_true", help="List models and WAVs only")
    args = parser.parse_args()

    GROQ_COOLDOWN["default"] = max(0.0, args.groq_cooldown)
    GROQ_COOLDOWN["oss"] = max(0.0, args.groq_oss_cooldown)

    slugs: set[str] | None
    if args.round1_winners:
        slugs = set(ROUND1_WINNERS)
    elif args.default_set:
        slugs = set(BENCHMARK_DEFAULT_SLUGS)
    elif args.slug.strip():
        slugs = {s.strip() for s in args.slug.split(",") if s.strip()}
    else:
        slugs = None

    providers = set(args.provider) if args.provider else None
    routes = resolve_routes(slugs=slugs, providers=providers)
    model_slugs = [str(r["slug"]) for r in routes]

    wav_filter: set[str] | None = None
    if args.wav.strip():
        wav_filter = {w.strip() for w in args.wav.split(",") if w.strip()}

    polish_lab.load_dotenv()
    system_prompt = render_production_system_prompt()
    transcripts = load_transcripts(refresh_stt=args.refresh_stt, wav_filter=wav_filter)
    if args.profile_clips_only:
        profile_names = set(WAV_PROFILES.keys())
        transcripts = [t for t in transcripts if t["wav"] in profile_names]

    print(f"Models: {len(routes)} | WAVs: {len(transcripts)} | jobs: {len(routes) * len(transcripts)}")
    for route in routes:
        print(f"  - {route['slug']}: {route['provider']} / {route['model']}")
    for row in transcripts:
        print(f"  - {row['wav']} ({row['words']} words)")

    if args.dry_run:
        return

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    tag = "winners_r2" if args.round1_winners else "full"
    run_dir = OUT_DIR / f"batch_{tag}_{stamp}"
    run_dir.mkdir(parents=True, exist_ok=True)

    jobs: list[tuple[dict[str, Any], dict[str, Any]]] = [
        (row, route) for row in transcripts for route in routes
    ]

    results: list[dict[str, Any]] = []
    workers = max(1, min(args.workers, len(jobs)))

    def _run(pair: tuple[dict[str, Any], dict[str, Any]]) -> dict[str, Any]:
        row, route = pair
        if workers == 1:
            groq_rate_limit_sleep(route)
        return run_job(transcript_row=row, route=route, system_prompt=system_prompt)

    if workers == 1:
        for job in jobs:
            results.append(_run(job))
    else:
        with ThreadPoolExecutor(max_workers=workers) as pool:
            futures = {pool.submit(_run, job): job for job in jobs}
            for future in as_completed(futures):
                results.append(future.result())

    payload = {
        "stamp": stamp,
        "models": model_slugs,
        "prompt": "production_prompt.render_production_system_prompt()",
        "transcripts": transcripts,
        "results": sorted(results, key=lambda r: (r["wav"], r["slug"])),
    }
    (run_dir / "batch_results.json").write_text(
        json.dumps(payload, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    write_report(
        stamp=stamp,
        run_dir=run_dir,
        transcripts=transcripts,
        results=results,
        prompt_note="production `default_voice_prompt_template()` (hinglish, neutral)",
        model_slugs=model_slugs,
    )
    print(run_dir / "batch_report.md")


if __name__ == "__main__":
    main()
