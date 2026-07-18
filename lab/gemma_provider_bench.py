#!/usr/bin/env python3
"""Compare direct Together Gemma 4 with OpenRouter routes on voice polish.

This is deliberately a *provider* experiment, not a replacement for the live
server benchmark.  It sends the same production system prompt and user-message
shape to three routes:

1. Together directly (the former control-plane transport)
2. OpenRouter, pinned to Together (measures OpenRouter transport overhead)
3. OpenRouter Nitro (the dynamic high-throughput route under consideration)

Each route receives the same ten frozen STT-error cases, repeated three times
by default.  Requests are streamed just as the production polish endpoint is,
which lets us record time-to-first-token (TTFT), total latency, and output
variation.  The benchmark intentionally does not retry failures: availability
means what the caller observed on its first attempt.

Results are written under lab/model_runs/ (gitignored) so they can safely
contain synthetic transcripts and provider response metadata.

Usage:
  python3 lab/gemma_provider_bench.py
  python3 lab/gemma_provider_bench.py --repeats 1 --case short-02
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import statistics
import sys
import time
import urllib.error
import urllib.request
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LAB = Path(__file__).resolve().parent
REPO = LAB.parent
if str(LAB) not in sys.path:
    sys.path.insert(0, str(LAB))

import polish_lab
import stress_suite
from context_prompt_lab import runtime_user_message
from production_prompt import render_production_system_prompt
from server_bench import score_case


TOGETHER_URL = "https://api.together.ai/v1/chat/completions"
OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions"
DEEPSEEK_URL = "https://api.deepseek.com/v1/chat/completions"
DEEPINFRA_URL = "https://api.deepinfra.com/v1/openai/chat/completions"
TOGETHER_MODEL = "google/gemma-4-31B-it"
OPENROUTER_MODEL = "google/gemma-4-31b-it"
OPENROUTER_NITRO_MODEL = f"{OPENROUTER_MODEL}:nitro"
DEEPSEEK_MODEL = "deepseek-v4-flash"
DEEPINFRA_GEMMA_MODEL = "google/gemma-4-26B-A4B-it"
OUT_DIR = LAB / "model_runs"
OUTPUT_LANGUAGE = "hinglish"
MAX_TOKENS = 1024
STOP = [
    "=== BEGIN TRANSCRIPT",
    "=== END TRANSCRIPT",
    "<transcript>",
    "</transcript>",
]
CASE_IDS = [
    "short-02", "dev-03", "ghard-02", "trap-01", "biz-02",
    "q-01", "hin-03", "cov-02", "inj-01", "halluc-03",
]

DEEPSEEK_ASR_RECONSTRUCTION_ADDENDUM = """

DEEPSEEK-SPECIFIC NOISY-ASR RECONSTRUCTION:
- Before formatting, silently reconstruct the most likely intended words from the full sentence.
- Treat split syllables, spaced acronyms, and ordinary-looking words as possible phonetic renderings of technical terms, brands, names, and identifiers.
- Compare multiple plausible candidates against the surrounding domain context before choosing one.
- Correct only when the sentence strongly supports the candidate. If evidence is weak, preserve the original wording.
- This reconstruction happens before punctuation, casing, or stylistic cleanup.

Illustrative error shapes (not a vocabulary list):
- "pie charm settings" -> "PyCharm settings"
- "get lab runner" -> "GitLab Runner"
- "air table base" -> "Airtable base"

Negative controls:
- "Naina ko message bhejo" stays "Naina ko message bhejo".
- "Solstice Labs ka invoice" stays "Solstice Labs ka invoice".

Return only the final cleaned transcript. Do not explain candidate selection.
""".strip()


@dataclass(frozen=True)
class Route:
    id: str
    label: str
    url: str
    api_key_env: str
    model: str
    provider: dict[str, Any] | None = None
    temperature: float = 0.0
    top_p: float | None = 0.9
    system_prompt_addendum: str = ""


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def percentile(values: list[float], p: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = (len(ordered) - 1) * p
    low = int(index)
    high = min(low + 1, len(ordered) - 1)
    return ordered[low] + (ordered[high] - ordered[low]) * (index - low)


def redact_sensitive(value: Any) -> Any:
    """Remove account identifiers before writing a local benchmark report."""
    if isinstance(value, dict):
        return {
            key: "<redacted>"
            if key.lower() == "id" or any(
                marker in key.lower()
                for marker in ("authorization", "api_key", "secret", "token", "user_id")
            )
            else redact_sensitive(child)
            for key, child in value.items()
        }
    if isinstance(value, list):
        return [redact_sensitive(child) for child in value]
    return value


def clean_error(value: str, limit: int = 500) -> str:
    """Keep reports useful without accidentally recording account identifiers."""
    try:
        value = json.dumps(redact_sensitive(json.loads(value)), ensure_ascii=False, separators=(",", ":"))
    except json.JSONDecodeError:
        pass
    return " ".join(value.replace("\n", " ").split())[:limit]


def routes() -> list[Route]:
    return [
        Route(
            id="together_direct",
            label="Together direct",
            url=TOGETHER_URL,
            api_key_env="TOGETHER_API_KEY",
            model=TOGETHER_MODEL,
        ),
        Route(
            id="openrouter_pinned_together",
            label="OpenRouter pinned to Together",
            url=OPENROUTER_URL,
            api_key_env="OPENROUTER_API_KEY",
            model=OPENROUTER_MODEL,
            # No fallback: this route exists solely to distinguish router
            # overhead from a different downstream inference provider.
            provider={"only": ["together"], "allow_fallbacks": False},
        ),
        Route(
            id="openrouter_nitro",
            label="OpenRouter Nitro (dynamic)",
            url=OPENROUTER_URL,
            api_key_env="OPENROUTER_API_KEY",
            model=OPENROUTER_NITRO_MODEL,
        ),
    ]


def deepseek_route() -> Route:
    return Route(
        id="deepseek_direct",
        label="DeepSeek V4 Flash (direct)",
        url=DEEPSEEK_URL,
        api_key_env="DEEPSEEK_API_KEY",
        model=DEEPSEEK_MODEL,
    )


def tuned_deepseek_routes() -> list[Route]:
    return [
        Route(
            id="deepseek_asr_prompt_only",
            label="DeepSeek V4 Flash (ASR prompt, baseline sampling)",
            url=DEEPSEEK_URL,
            api_key_env="DEEPSEEK_API_KEY",
            model=DEEPSEEK_MODEL,
            system_prompt_addendum=DEEPSEEK_ASR_RECONSTRUCTION_ADDENDUM,
        ),
        Route(
            id="deepseek_temperature_1",
            label="DeepSeek V4 Flash (temperature 1.0)",
            url=DEEPSEEK_URL,
            api_key_env="DEEPSEEK_API_KEY",
            model=DEEPSEEK_MODEL,
            temperature=1.0,
            top_p=None,
        ),
        Route(
            id="deepseek_asr_tuned",
            label="DeepSeek V4 Flash (ASR prompt + temperature 1.0)",
            url=DEEPSEEK_URL,
            api_key_env="DEEPSEEK_API_KEY",
            model=DEEPSEEK_MODEL,
            temperature=1.0,
            top_p=None,
            system_prompt_addendum=DEEPSEEK_ASR_RECONSTRUCTION_ADDENDUM,
        ),
    ]


def production_gemma_route() -> Route:
    return Route(
        id="deepinfra_gemma_4_26b",
        label="Gemma 4 26B A4B (DeepInfra production)",
        url=DEEPINFRA_URL,
        api_key_env="DEEPINFRA_API_KEY",
        model=DEEPINFRA_GEMMA_MODEL,
    )


def load_cases(case_ids: list[str] | None) -> list[dict[str, Any]]:
    by_id = {case["id"]: case for case in stress_suite.CASES}
    wanted = case_ids or CASE_IDS
    missing = [case_id for case_id in wanted if case_id not in by_id]
    if missing:
        raise ValueError(f"Unknown case ids: {', '.join(missing)}")
    if len(wanted) != 10 and not case_ids:
        raise AssertionError("Default benchmark must remain exactly ten cases")
    return [by_id[case_id] for case_id in wanted]


def request_payload(route: Route, system_prompt: str, user_message: str) -> dict[str, Any]:
    effective_system_prompt = system_prompt
    if route.system_prompt_addendum:
        effective_system_prompt = f"{system_prompt}\n\n{route.system_prompt_addendum}"
    payload: dict[str, Any] = {
        "model": route.model,
        "temperature": route.temperature,
        "max_tokens": MAX_TOKENS,
        "reasoning": {"enabled": False},
        "stream": True,
        "stop": STOP,
        "messages": [
            {"role": "system", "content": effective_system_prompt},
            {"role": "user", "content": user_message},
        ],
    }
    if route.top_p is not None:
        payload["top_p"] = route.top_p
    if route.id.startswith("deepseek_") or route.id == "deepinfra_gemma_4_26b":
        payload.pop("reasoning", None)
    if route.id.startswith("deepseek_"):
        # Match the production DeepSeek route: disable model thinking while
        # keeping every correction-benchmark input identical across models.
        payload["thinking"] = {"type": "disabled"}
    if route.provider:
        payload["provider"] = route.provider
    return payload


def allowed_response_headers(headers: Any) -> dict[str, str]:
    """Store diagnostic response headers only; never request headers or secrets."""
    prefixes = ("x-openrouter-", "x-ratelimit-", "cf-cache-status", "x-request-id")
    return {
        key.lower(): value[:200]
        for key, value in headers.items()
        if key.lower().startswith(prefixes)
    }


def parse_sse_stream(resp: Any, started: float) -> tuple[str, float | None, dict[str, Any]]:
    parts: list[str] = []
    ttft_ms: float | None = None
    metadata: dict[str, Any] = {}

    for raw_line in resp:
        line = raw_line.decode("utf-8", errors="replace").strip()
        if not line or not line.startswith("data:"):
            continue
        raw_data = line[5:].strip()
        if raw_data == "[DONE]":
            break
        try:
            event = json.loads(raw_data)
        except json.JSONDecodeError:
            continue

        # Router metadata can arrive in a terminal chunk.  Persist it verbatim
        # for traceability, but this code does not assume a provider field is
        # present (Nitro is intentionally dynamic).
        if isinstance(event.get("openrouter_metadata"), dict):
            metadata["openrouter_metadata"] = event["openrouter_metadata"]
        for key in ("provider", "model", "usage"):
            if key in event:
                metadata[key] = event[key]

        for choice in event.get("choices") or []:
            delta = choice.get("delta") or {}
            content = delta.get("content")
            if isinstance(content, str) and content:
                if ttft_ms is None:
                    ttft_ms = (time.perf_counter() - started) * 1000
                parts.append(content)

    return "".join(parts).strip(), ttft_ms, metadata


def call_route(route: Route, payload: dict[str, Any], timeout: float) -> dict[str, Any]:
    key = os.getenv(route.api_key_env, "").strip()
    if not key:
        raise RuntimeError(f"{route.api_key_env} is missing after loading .env")

    headers = {
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        "Accept": "text/event-stream",
        "User-Agent": "airnote-gemma-provider-bench/1.0",
    }
    if route.id.startswith("openrouter_"):
        headers["X-OpenRouter-Metadata"] = "enabled"

    request = urllib.request.Request(
        route.url,
        data=json.dumps(payload).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as resp:
            output, ttft_ms, metadata = parse_sse_stream(resp, started)
            total_ms = (time.perf_counter() - started) * 1000
            return {
                "ok": True,
                "output": output,
                "ttft_ms": round(ttft_ms, 1) if ttft_ms is not None else None,
                "total_ms": round(total_ms, 1),
                "http_status": resp.status,
                "response_headers": allowed_response_headers(resp.headers),
                "metadata": metadata,
            }
    except urllib.error.HTTPError as exc:
        retry_after = exc.headers.get("Retry-After")
        return {
            "ok": False,
            "http_status": exc.code,
            "error": clean_error(exc.read().decode("utf-8", errors="replace")),
            "retry_after_s": float(retry_after) if retry_after and retry_after.isdigit() else None,
            "total_ms": round((time.perf_counter() - started) * 1000, 1),
        }
    except Exception as exc:  # noqa: BLE001 - benchmark must report any transport failure.
        return {
            "ok": False,
            "http_status": None,
            "error": clean_error(f"{type(exc).__name__}: {exc}"),
            "total_ms": round((time.perf_counter() - started) * 1000, 1),
        }


def summarize_route(records: list[dict[str, Any]]) -> dict[str, Any]:
    successes = [record for record in records if record["result"]["ok"]]
    scored = [record for record in successes if record.get("eval")]
    ttfts = [record["result"]["ttft_ms"] for record in successes if record["result"]["ttft_ms"] is not None]
    totals = [record["result"]["total_ms"] for record in successes]
    pass_count = sum(1 for record in scored if record["eval"]["passed"])
    scores = [record["eval"]["score"] for record in scored]

    by_case: dict[str, list[str]] = defaultdict(list)
    for record in successes:
        by_case[record["case"]["id"]].append(record["result"]["output"])
    stable_cases = sum(1 for outputs in by_case.values() if len(set(outputs)) == 1)
    variant_cases = {
        case_id: len(set(outputs))
        for case_id, outputs in by_case.items()
        if len(set(outputs)) > 1
    }

    return {
        "attempts": len(records),
        "http_successes": len(successes),
        "availability_pct": round(100 * len(successes) / len(records), 1) if records else 0.0,
        "quality_passes": pass_count,
        "quality_pass_pct": round(100 * pass_count / len(scored), 1) if scored else 0.0,
        "mean_score": round(statistics.fmean(scores), 2) if scores else None,
        "ttft_ms": {
            "median": round(statistics.median(ttfts), 1) if ttfts else None,
            "p95": round(percentile(ttfts, 0.95), 1) if ttfts else None,
        },
        "total_ms": {
            "median": round(statistics.median(totals), 1) if totals else None,
            "p95": round(percentile(totals, 0.95), 1) if totals else None,
        },
        "fully_stable_cases": stable_cases,
        "completed_cases": len(by_case),
        "variant_cases": variant_cases,
        "errors": dict(Counter(
            f"HTTP {record['result'].get('http_status')}: {record['result'].get('error', '')}"
            for record in records if not record["result"]["ok"]
        )),
    }


def write_report(run_dir: Path, manifest: dict[str, Any], records: list[dict[str, Any]]) -> None:
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for record in records:
        grouped[record["route"]["id"]].append(record)
    summaries = {route_id: summarize_route(route_records) for route_id, route_records in grouped.items()}

    deepseek_benchmark = any(route["id"].startswith("deepseek_") for route in manifest["routes"])
    title = "DeepSeek V4 Flash correction benchmark" if deepseek_benchmark else "Gemma 4 provider benchmark"
    report = [
        f"# {title} — {manifest['started_at']}",
        "",
        "## What was held constant",
        "",
        f"- Exact rendered production system prompt (SHA-256 `{manifest['system_prompt_sha256']}`), with empty dynamic learning/profile blocks.",
        f"- Exact production cleaner-mode user-message shape (user messages SHA-256 recorded per row).",
        f"- `{len(manifest['case_ids'])}` fixed STT-error cases × `{manifest['repeats']}` repetitions × `{len(manifest['routes'])}` routes.",
        f"- Per-route temperature, top-p, and prompt variants are recorded in the manifest; every route keeps thinking disabled, `max_tokens={MAX_TOKENS}`, streaming, and production stop strings.",
        "- No retries: availability is the first-attempt result observed by this client. This is a short benchmark, not a provider SLA measurement.",
        "",
        "## Headline",
        "",
        "| Route | HTTP availability | Quality pass | Mean score | Median TTFT | P95 TTFT | Median total | P95 total | Exact-output stable cases |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for route in manifest["routes"]:
        summary = summaries[route["id"]]
        ttft = summary["ttft_ms"]
        total = summary["total_ms"]
        report.append(
            f"| {route['label']} | {summary['http_successes']}/{summary['attempts']} "
            f"({summary['availability_pct']}%) | {summary['quality_passes']}/{summary['http_successes']} "
            f"({summary['quality_pass_pct']}%) | {summary['mean_score'] if summary['mean_score'] is not None else '—'} "
            f"| {ttft['median'] if ttft['median'] is not None else '—'} ms | {ttft['p95'] if ttft['p95'] is not None else '—'} ms "
            f"| {total['median'] if total['median'] is not None else '—'} ms | {total['p95'] if total['p95'] is not None else '—'} ms "
            f"| {summary['fully_stable_cases']}/{summary['completed_cases']} |"
        )

    report.extend(["", "## Route interpretation", ""])
    route_ids = {route["id"] for route in manifest["routes"]}
    if "together_direct" in route_ids:
        report.append("- **Together direct** is the former production transport used as a direct-provider baseline.")
    if "openrouter_pinned_together" in route_ids:
        report.append("- **OpenRouter pinned to Together** uses `provider.only=[\"together\"]` and disables fallback. Any delta from direct Together is routing/transport behavior, not a different inference host.")
    if "openrouter_nitro" in route_ids:
        report.append("- **OpenRouter Nitro** is intentionally dynamic. Its observed result is for the Nitro service, not proof that a single downstream provider performed that way every time.")
    if "deepseek_direct" in route_ids:
        report.append("- **DeepSeek direct** uses the production `deepseek-v4-flash` model with thinking disabled.")
    if "deepseek_temperature_1" in route_ids:
        report.append("- **DeepSeek temperature 1** keeps the production prompt, sets temperature to 1.0, and omits top-p.")
    if "deepseek_asr_prompt_only" in route_ids:
        report.append("- **DeepSeek ASR prompt-only** keeps baseline sampling and adds held-out phonetic reconstruction guidance.")
    if "deepseek_asr_tuned" in route_ids:
        report.append("- **DeepSeek ASR tuned** adds held-out phonetic reconstruction guidance to the temperature-1 route; thinking remains disabled.")
    if "deepinfra_gemma_4_26b" in route_ids:
        report.append("- **DeepInfra Gemma** is AirNote's production `google/gemma-4-26B-A4B-it` correction route.")
    report.extend(["", "## Per-case output and scoring", ""])

    for record in records:
        route = record["route"]
        case = record["case"]
        result = record["result"]
        report.append(f"### `{route['id']}` · `{case['id']}` · repeat {record['repeat']}")
        if result["ok"]:
            evaluation = record["eval"]
            report.extend([
                f"- latency: TTFT `{result['ttft_ms']}` ms · total `{result['total_ms']}` ms",
                f"- score: `{evaluation['score']}/5` · pass=`{evaluation['passed']}` · diagnosis: {evaluation['diagnosis']}",
                f"- output: {result['output']}",
            ])
            metadata = result.get("metadata") or {}
            if metadata:
                report.append(f"- router metadata: `{json.dumps(metadata, ensure_ascii=False, sort_keys=True)[:800]}`")
        else:
            report.append(f"- error: HTTP `{result.get('http_status')}` · {result.get('error')}")
        report.append("")

    (run_dir / "report.md").write_text("\n".join(report) + "\n", encoding="utf-8")
    (run_dir / "results.json").write_text(
        json.dumps({"manifest": manifest, "summaries": summaries, "records": records}, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repeats", type=int, default=3, help="runs per case and route (default: 3)")
    parser.add_argument("--case", action="append", dest="case_ids", help="run only this case id; repeatable")
    parser.add_argument("--delay", type=float, default=0.2, help="seconds between requests (default: 0.2)")
    parser.add_argument(
        "--timeout",
        type=float,
        default=15.0,
        help="per-request socket/read timeout seconds (default: 15)",
    )
    parser.add_argument(
        "--route",
        action="append",
        dest="route_ids",
        help="include only this route id; repeatable (default: all routes)",
    )
    parser.add_argument(
        "--include-deepseek",
        action="store_true",
        help="add direct deepseek-v4-flash as a model-comparison route",
    )
    parser.add_argument(
        "--include-production-gemma",
        action="store_true",
        help="add AirNote's DeepInfra Gemma 4 26B A4B production route",
    )
    parser.add_argument(
        "--include-deepseek-tuned",
        action="store_true",
        help="add non-thinking DeepSeek temperature-only and ASR-prompt variants",
    )
    args = parser.parse_args()
    if args.repeats < 1:
        raise SystemExit("--repeats must be at least 1")

    polish_lab.load_dotenv()
    active_routes = routes()
    if args.include_deepseek:
        active_routes.append(deepseek_route())
    if args.include_production_gemma:
        active_routes.append(production_gemma_route())
    if args.include_deepseek_tuned:
        active_routes.extend(tuned_deepseek_routes())
    if args.route_ids:
        requested = set(args.route_ids)
        unknown = requested - {route.id for route in active_routes}
        if unknown:
            raise SystemExit(f"Unknown route id(s): {', '.join(sorted(unknown))}")
        active_routes = [route for route in active_routes if route.id in requested]
    missing_keys = sorted({route.api_key_env for route in active_routes if not os.getenv(route.api_key_env, "").strip()})
    if missing_keys:
        raise SystemExit(f"Missing API keys in .env: {', '.join(missing_keys)}")
    cases = load_cases(args.case_ids)
    system_prompt = render_production_system_prompt(output_language=OUTPUT_LANGUAGE, tone_preset="neutral")

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    suffix = (
        "deepseek-correction-benchmark"
        if args.include_deepseek or args.include_deepseek_tuned
        else "gemma-provider-benchmark"
    )
    run_dir = OUT_DIR / f"{stamp}_{suffix}"
    run_dir.mkdir(parents=True, exist_ok=False)
    manifest: dict[str, Any] = {
        "started_at": stamp,
        "git_head": os.popen("git rev-parse --short HEAD").read().strip(),
        "system_prompt_sha256": sha256_text(system_prompt),
        "case_ids": [case["id"] for case in cases],
        "repeats": args.repeats,
        "routes": [
            {
                "id": route.id,
                "label": route.label,
                "model": route.model,
                "provider": route.provider,
                "temperature": route.temperature,
                "top_p": route.top_p,
                "system_prompt_addendum_sha256": sha256_text(route.system_prompt_addendum)
                if route.system_prompt_addendum
                else None,
            }
            for route in active_routes
        ],
        "request": {
            "max_tokens": MAX_TOKENS,
            "thinking": {"type": "disabled"},
            "stream": True,
            "stop": STOP,
        },
    }
    (run_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    records: list[dict[str, Any]] = []
    total = len(cases) * args.repeats * len(active_routes)
    n = 0
    for repeat in range(1, args.repeats + 1):
        # Rotate order so a single provider does not always run in the same
        # position relative to transient internet congestion.
        ordered_routes = active_routes[repeat - 1:] + active_routes[:repeat - 1]
        for case in cases:
            user_message = runtime_user_message(case["transcript"], OUTPUT_LANGUAGE)
            # Submit routes for the same case together. This prevents the later
            # provider from inheriting a queueing penalty caused by the earlier
            # one, while preserving a per-route socket/read timeout.
            with ThreadPoolExecutor(max_workers=len(ordered_routes)) as executor:
                futures = {
                    route.id: executor.submit(
                        call_route,
                        route,
                        request_payload(route, system_prompt, user_message),
                        args.timeout,
                    )
                    for route in ordered_routes
                }
                results = {route_id: future.result() for route_id, future in futures.items()}

            for route in ordered_routes:
                n += 1
                result = results[route.id]
                record: dict[str, Any] = {
                    "route": {"id": route.id, "label": route.label, "model": route.model},
                    "case": case,
                    "repeat": repeat,
                    "user_message_sha256": sha256_text(user_message),
                    "result": result,
                }
                if result["ok"]:
                    record["eval"] = score_case(case, result["output"], OUTPUT_LANGUAGE)
                    badge = "PASS" if record["eval"]["passed"] else "FAIL"
                    print(
                        f"[{n:02d}/{total}] {route.id:28} {case['id']:10} {badge:4} "
                        f"ttft={result['ttft_ms']}ms total={result['total_ms']}ms score={record['eval']['score']}/5"
                    )
                else:
                    print(
                        f"[{n:02d}/{total}] {route.id:28} {case['id']:10} ERROR "
                        f"http={result.get('http_status')} {result.get('error', '')[:120]}"
                    )
                records.append(record)
                if n < total:
                    # Do not retry a failed sample, but do honor a provider's
                    # published backoff before continuing to the next sample.
                    wait_s = max(args.delay, result.get("retry_after_s") or 0.0)
                    time.sleep(wait_s)

    write_report(run_dir, manifest, records)
    print(f"\nReport: {run_dir.relative_to(REPO) / 'report.md'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
