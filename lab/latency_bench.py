#!/usr/bin/env python3
"""Latency benchmark — Groq Scout vs Groq GPT OSS 120B (production prompt + cached STT).

Is polish slow because of the model or our stack? This script isolates the LLM call:
same production system prompt, same cached Swift transcript, same API payload
shape as said-backend (streaming, temperature, stop sequences, max_tokens).

  python lab/latency_bench.py
  python lab/latency_bench.py --runs 15 --warmup 2
  python lab/latency_bench.py --no-stream   # full JSON response only (not prod path)
  python lab/latency_bench.py --dry-run

Requires GROQ_API_KEY (or GATEWAY_API_KEY) in repo .env.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LAB = Path(__file__).resolve().parent
if str(LAB) not in sys.path:
    sys.path.insert(0, str(LAB))

import polish_lab
from production_prompt import render_production_system_prompt

REPO = LAB.parent
OUT_DIR = LAB / "latency_runs"
CACHE_PATH = LAB / "cache" / "session.json"

GROQ_BASE = "https://api.groq.com/openai/v1"
GROQ_SCOUT = "meta-llama/llama-4-scout-17b-16e-instruct"
GROQ_GPT_OSS = "openai/gpt-oss-120b"

STOP_SEQUENCES = [
    "=== BEGIN TRANSCRIPT",
    "=== END TRANSCRIPT",
    "<transcript>",
    "</transcript>",
]


@dataclass
class ModelTarget:
    slug: str
    provider: str
    base_url: str
    model: str
    temperature: float
    api_key: str


@dataclass
class RunResult:
    slug: str
    run_index: int
    ok: bool
    wall_s: float
    ttft_s: float | None
    output_chars: int
    error: str | None = None
    usage: dict[str, Any] = field(default_factory=dict)
    output_preview: str = ""


def load_transcript() -> tuple[str, dict]:
    if not CACHE_PATH.is_file():
        raise SystemExit(
            f"No cached transcript at {CACHE_PATH}. "
            "Run: python lab/polish_lab.py /path/to.wav"
        )
    cache = json.loads(CACHE_PATH.read_text(encoding="utf-8"))
    transcript = (cache.get("transcript") or "").strip()
    if not transcript:
        raise SystemExit("Cached transcript is empty.")
    return transcript, cache


def build_messages(transcript: str) -> tuple[str, str]:
    system = render_production_system_prompt()
    user = polish_lab.build_user_message(transcript)
    return system, user


def max_tokens_for(user_message: str) -> int:
    est = max(len(user_message) // 4, 64)
    return min(est * 2 + 256, 8192)


def resolve_targets() -> list[ModelTarget]:
    polish_lab.load_dotenv()
    targets: list[ModelTarget] = []
    groq_key = os.getenv("GROQ_API_KEY", "").strip() or os.getenv(
        "GATEWAY_API_KEY", ""
    ).strip()
    if groq_key:
        targets.append(
            ModelTarget(
                slug="groq-scout",
                provider="groq",
                base_url=GROQ_BASE,
                model=GROQ_SCOUT,
                temperature=0.0,
                api_key=groq_key,
            )
        )
        targets.append(
            ModelTarget(
                slug="groq-gpt-oss",
                provider="groq",
                base_url=GROQ_BASE,
                model=GROQ_GPT_OSS,
                temperature=0.0,
                api_key=groq_key,
            )
        )
    if not targets:
        raise SystemExit("Set GROQ_API_KEY (or GATEWAY_API_KEY) in .env")
    return targets


def parse_sse_stream(raw: bytes) -> tuple[str, dict[str, Any]]:
    """Return (full_text, usage_dict). usage may be empty."""
    text_parts: list[str] = []
    usage: dict[str, Any] = {}
    for line in raw.decode("utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line.startswith("data:"):
            continue
        data = line[5:].strip()
        if data == "[DONE]":
            break
        try:
            chunk = json.loads(data)
        except json.JSONDecodeError:
            continue
        if isinstance(chunk.get("usage"), dict):
            usage = chunk["usage"]
        for choice in chunk.get("choices") or []:
            delta = choice.get("delta") or {}
            content = delta.get("content")
            if content:
                text_parts.append(content)
            message = choice.get("message") or {}
            if message.get("content"):
                text_parts.append(str(message["content"]))
    return "".join(text_parts).strip(), usage


def call_stream(
    target: ModelTarget,
    system: str,
    user: str,
) -> RunResult:
    payload = {
        "model": target.model,
        "stream": True,
        "temperature": target.temperature,
        "top_p": 0.9,
        "max_tokens": max_tokens_for(user),
        "stop": STOP_SEQUENCES,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    }
    if "gpt-oss" in target.model:
        payload["max_tokens"] = max(payload["max_tokens"], 4096)
        payload["reasoning_effort"] = "low"
    url = f"{target.base_url}/chat/completions"
    req = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers=polish_lab.api_headers(target.api_key),
        method="POST",
    )
    t0 = time.perf_counter()
    ttft: float | None = None
    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            chunks: list[bytes] = []
            while True:
                chunk = resp.read(4096)
                if not chunk:
                    break
                if ttft is None:
                    # First body byte — proxy for time-to-first-token in streaming.
                    if _sse_has_content(chunk):
                        ttft = time.perf_counter() - t0
                chunks.append(chunk)
        wall = time.perf_counter() - t0
        output, usage = parse_sse_stream(b"".join(chunks))
        if not output:
            return RunResult(
                slug=target.slug,
                run_index=-1,
                ok=False,
                wall_s=wall,
                ttft_s=ttft,
                output_chars=0,
                error="empty output",
                usage=usage,
            )
        return RunResult(
            slug=target.slug,
            run_index=-1,
            ok=True,
            wall_s=wall,
            ttft_s=ttft,
            output_chars=len(output),
            usage=usage,
            output_preview=output[:160],
        )
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")[:400]
        return RunResult(
            slug=target.slug,
            run_index=-1,
            ok=False,
            wall_s=time.perf_counter() - t0,
            ttft_s=None,
            output_chars=0,
            error=f"HTTP {exc.code}: {detail}",
        )
    except Exception as exc:
        return RunResult(
            slug=target.slug,
            run_index=-1,
            ok=False,
            wall_s=time.perf_counter() - t0,
            ttft_s=None,
            output_chars=0,
            error=str(exc),
        )


def _sse_has_content(chunk: bytes) -> bool:
    text = chunk.decode("utf-8", errors="replace")
    for line in text.splitlines():
        if not line.startswith("data:"):
            continue
        data = line[5:].strip()
        if data in ("", "[DONE]"):
            continue
        try:
            parsed = json.loads(data)
        except json.JSONDecodeError:
            continue
        for choice in parsed.get("choices") or []:
            delta = choice.get("delta") or {}
            if delta.get("content"):
                return True
    return False


def call_non_stream(
    target: ModelTarget,
    system: str,
    user: str,
) -> RunResult:
    payload = {
        "model": target.model,
        "stream": False,
        "temperature": target.temperature,
        "top_p": 0.9,
        "max_tokens": max_tokens_for(user),
        "stop": STOP_SEQUENCES,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    }
    url = f"{target.base_url}/chat/completions"
    req = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers=polish_lab.api_headers(target.api_key),
        method="POST",
    )
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        wall = time.perf_counter() - t0
        choices = body.get("choices") or []
        output = ""
        if choices:
            output = (choices[0].get("message") or {}).get("content") or ""
        output = output.strip()
        usage = body.get("usage") or {}
        if not output:
            return RunResult(
                slug=target.slug,
                run_index=-1,
                ok=False,
                wall_s=wall,
                ttft_s=None,
                output_chars=0,
                error="empty output",
                usage=usage,
            )
        return RunResult(
            slug=target.slug,
            run_index=-1,
            ok=True,
            wall_s=wall,
            ttft_s=None,
            output_chars=len(output),
            usage=usage,
            output_preview=output[:160],
        )
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")[:400]
        return RunResult(
            slug=target.slug,
            run_index=-1,
            ok=False,
            wall_s=time.perf_counter() - t0,
            ttft_s=None,
            output_chars=0,
            error=f"HTTP {exc.code}: {detail}",
        )
    except Exception as exc:
        return RunResult(
            slug=target.slug,
            run_index=-1,
            ok=False,
            wall_s=time.perf_counter() - t0,
            ttft_s=None,
            output_chars=0,
            error=str(exc),
        )


def summarize(slug: str, runs: list[RunResult]) -> dict[str, Any]:
    ok_runs = [r for r in runs if r.ok]
    walls = [r.wall_s for r in ok_runs]
    ttfts = [r.ttft_s for r in ok_runs if r.ttft_s is not None]
    out: dict[str, Any] = {
        "slug": slug,
        "attempts": len(runs),
        "successes": len(ok_runs),
        "failures": len(runs) - len(ok_runs),
    }
    if walls:
        out["wall_mean_s"] = statistics.mean(walls)
        out["wall_median_s"] = statistics.median(walls)
        out["wall_min_s"] = min(walls)
        out["wall_max_s"] = max(walls)
        if len(walls) > 1:
            out["wall_stdev_s"] = statistics.stdev(walls)
    if ttfts:
        out["ttft_mean_s"] = statistics.mean(ttfts)
        out["ttft_median_s"] = statistics.median(ttfts)
    if ok_runs:
        groq_usage = [
            r.usage for r in ok_runs if r.usage and "queue_time" in r.usage
        ]
        if groq_usage:
            out["groq_queue_mean_s"] = statistics.mean(
                float(u.get("queue_time", 0)) for u in groq_usage
            )
            out["groq_total_mean_s"] = statistics.mean(
                float(u.get("total_time", 0)) for u in groq_usage
            )
    errors = [r.error for r in runs if r.error]
    if errors:
        out["sample_error"] = errors[0]
    return out


def write_report(
    *,
    stamp: str,
    run_dir: Path,
    cache: dict,
    transcript: str,
    system_chars: int,
    user_chars: int,
    stream: bool,
    runs: int,
    warmup: int,
    summaries: dict[str, dict[str, Any]],
    all_results: list[RunResult],
) -> None:
    lines = [
        f"# Latency benchmark — {stamp}",
        "",
        "## Setup",
        "",
        f"- Transcript: `{cache.get('wav_path', '?')}`",
        f"- STT model (cached): `{cache.get('stt_model', '?')}`",
        f"- System prompt: production `default_voice_prompt_template()` (hinglish, neutral)",
        f"- System prompt chars: {system_chars:,}",
        f"- User message chars: {user_chars:,}",
        f"- Mode: **{'streaming (prod-like)' if stream else 'non-streaming'}**",
        f"- Runs per model: {runs} (+ {warmup} warmup each, discarded)",
        "",
        "### Cached transcript",
        "",
        transcript,
        "",
        "## Summary",
        "",
        "| Model | Mean wall | Median wall | TTFT mean | Success |",
        "|---|---:|---:|---:|---:|",
    ]
    for slug, s in summaries.items():
        mean = s.get("wall_mean_s")
        med = s.get("wall_median_s")
        ttft = s.get("ttft_mean_s")
        lines.append(
            f"| `{slug}` | "
            f"{f'{mean:.3f}s' if mean is not None else '—'} | "
            f"{f'{med:.3f}s' if med is not None else '—'} | "
            f"{f'{ttft:.3f}s' if ttft is not None else '—'} | "
            f"{s.get('successes', 0)}/{s.get('attempts', 0)} |"
        )

    slugs = list(summaries.keys())
    if len(slugs) == 2 and all(
        summaries[s].get("wall_mean_s") is not None for s in slugs
    ):
        a, b = slugs
        ma, mb = summaries[a]["wall_mean_s"], summaries[b]["wall_mean_s"]
        faster = a if ma < mb else b
        slower = b if faster == a else a
        ms, mf = summaries[faster]["wall_mean_s"], summaries[slower]["wall_mean_s"]
        delta = mf - ms
        pct = (delta / mf) * 100 if mf else 0
        lines.extend(
            [
                "",
                f"**Faster on mean wall time:** `{faster}` by **{delta:.3f}s** ({pct:.1f}% vs `{slower}`)",
            ]
        )

    lines.extend(["", "## Raw runs", ""])
    for r in all_results:
        if r.ok:
            ttft = f"{r.ttft_s:.3f}s" if r.ttft_s is not None else "—"
            lines.append(
                f"- `{r.slug}` #{r.run_index}: wall={r.wall_s:.3f}s ttft={ttft} "
                f"chars={r.output_chars}"
            )
        else:
            lines.append(f"- `{r.slug}` #{r.run_index}: FAIL — {r.error}")

    (run_dir / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    (run_dir / "results.json").write_text(
        json.dumps(
            {
                "stamp": stamp,
                "stream": stream,
                "runs": runs,
                "warmup": warmup,
                "summaries": summaries,
                "results": [
                    {
                        "slug": r.slug,
                        "run_index": r.run_index,
                        "ok": r.ok,
                        "wall_s": r.wall_s,
                        "ttft_s": r.ttft_s,
                        "output_chars": r.output_chars,
                        "error": r.error,
                        "usage": r.usage,
                        "output_preview": r.output_preview,
                    }
                    for r in all_results
                ],
            },
            indent=2,
            ensure_ascii=False,
        )
        + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=10, help="Timed runs per model")
    parser.add_argument("--warmup", type=int, default=1, help="Warmup runs (discarded)")
    parser.add_argument(
        "--no-stream",
        action="store_true",
        help="Non-streaming JSON (not production path)",
    )
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    transcript, cache = load_transcript()
    system, user = build_messages(transcript)
    targets = resolve_targets()
    stream = not args.no_stream
    call_fn = call_stream if stream else call_non_stream

    print(f"Transcript words: {len(transcript.split())}")
    print(f"System prompt: {len(system):,} chars (production template from prompt.rs)")
    print(f"User message: {len(user):,} chars")
    print(f"Mode: {'streaming' if stream else 'non-streaming'}")
    print("Models:")
    for t in targets:
        print(f"  - {t.slug}: {t.provider} / {t.model}")

    if args.dry_run:
        return 0

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = OUT_DIR / stamp
    run_dir.mkdir(parents=True, exist_ok=True)

    by_slug: dict[str, list[RunResult]] = {t.slug: [] for t in targets}
    all_results: list[RunResult] = []

    for target in targets:
        for w in range(args.warmup):
            print(f"[warmup] {target.slug}...")
            call_fn(target, system, user)

    # Alternate models each run to reduce provider/network bias.
    for i in range(1, args.runs + 1):
        for target in targets:
            print(f"[run {i}/{args.runs}] {target.slug}...")
            result = call_fn(target, system, user)
            result.run_index = i
            by_slug[target.slug].append(result)
            all_results.append(result)
            if result.ok:
                ttft = (
                    f" ttft={result.ttft_s:.3f}s" if result.ttft_s is not None else ""
                )
                print(f"  ok wall={result.wall_s:.3f}s{ttft} chars={result.output_chars}")
            else:
                print(f"  FAIL: {result.error}")

    summaries = {slug: summarize(slug, runs) for slug, runs in by_slug.items()}
    write_report(
        stamp=stamp,
        run_dir=run_dir,
        cache=cache,
        transcript=transcript,
        system_chars=len(system),
        user_chars=len(user),
        stream=stream,
        runs=args.runs,
        warmup=args.warmup,
        summaries=summaries,
        all_results=all_results,
    )

    print(f"\nSaved → {run_dir.relative_to(REPO)}/report.md")
    print("\n=== RESULTS ===")
    for slug, s in summaries.items():
        mean = s.get("wall_mean_s")
        if mean is not None:
            ttft = s.get("ttft_mean_s")
            extra = f" ttft_mean={ttft:.3f}s" if ttft is not None else ""
            print(f"{slug}: mean={mean:.3f}s median={s.get('wall_median_s', 0):.3f}s{extra}")
        else:
            print(f"{slug}: all failed — {s.get('sample_error', '?')}")

    if len(summaries) == 2:
        keys = list(summaries.keys())
        if all(summaries[k].get("wall_mean_s") is not None for k in keys):
            a, b = keys
            ma, mb = summaries[a]["wall_mean_s"], summaries[b]["wall_mean_s"]
            if ma < mb:
                print(f"\n{f'{a} faster by {mb - ma:.3f}s ({(mb - ma) / mb * 100:.1f}%)'}")
            else:
                print(f"\n{f'{b} faster by {ma - mb:.3f}s ({(ma - mb) / ma * 100:.1f}%)'}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
