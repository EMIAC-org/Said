#!/usr/bin/env python3
"""Compare Codex Spark and Cerebras Gemma 4 on AirNote correction cases.

Uses the existing stress-suite cases and deterministic server_bench scorecard.
Spark is invoked through Codex CLI; Gemma is invoked through direct Cerebras
chat completions. This is a useful product-behavior comparison, but the two
transport paths are intentionally reported separately.

  python3 lab/codex_correction_bench.py
  python3 lab/codex_correction_bench.py --categories dev_garble,garble_hard
  python3 lab/codex_correction_bench.py --limit 3 --dry-run
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LAB = Path(__file__).resolve().parent
REPO = LAB.parent
OUT_DIR = LAB / "model_runs"
CEREBRAS_BASE = "https://api.cerebras.ai/v1"
GEMMA_MODEL = "gemma-4-31b"
SPARK_MODEL = "gpt-5.3-codex-spark"
DEFAULT_CATEGORIES = {"dev_garble", "garble_hard", "over_correction"}

import polish_lab
import server_bench
import stress_suite
from codex_agent_latency_bench import run_spark
from production_prompt import render_production_system_prompt


def call_gemma(
    *, system_prompt: str, user_message: str, api_key: str, timeout_s: int
) -> dict[str, Any]:
    payload = {
        "model": GEMMA_MODEL,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_message},
        ],
        "temperature": 0.0,
        # Long cases need room to preserve the final clause. This is still a
        # bounded reservation rather than Cerebras's full context window.
        "max_completion_tokens": 1024,
        "stream": False,
    }
    request = urllib.request.Request(
        f"{CEREBRAS_BASE}/chat/completions",
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "User-Agent": "airnote-correction-bench/1.0",
        },
        method="POST",
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(request, timeout=timeout_s) as response:
            body = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        return {
            "ok": False,
            "wall_s": time.perf_counter() - started,
            "error": f"HTTP {exc.code}: {exc.read().decode('utf-8', errors='replace')[:800]}",
        }
    except Exception as exc:
        return {"ok": False, "wall_s": time.perf_counter() - started, "error": str(exc)}

    choices = body.get("choices") or []
    output = str(choices[0].get("message", {}).get("content", "")).strip() if choices else ""
    if not output:
        return {"ok": False, "wall_s": time.perf_counter() - started, "error": "empty output"}
    return {
        "ok": True,
        "wall_s": time.perf_counter() - started,
        "output": output,
        "usage": body.get("usage", {}),
    }


def spark_prompt(system_prompt: str, user_message: str) -> str:
    return f"""Do not inspect files and do not run tools.

Act only as the transcription cleaner specified below. Follow the system prompt
and process the final user message. Return only the polished transcription with
no explanation, labels, quotes, or markdown.

--- SYSTEM PROMPT ---
{system_prompt}
--- END SYSTEM PROMPT ---

--- USER MESSAGE ---
{user_message}
--- END USER MESSAGE ---"""


def evaluate(case: dict[str, Any], response: dict[str, Any]) -> dict[str, Any]:
    if not response.get("ok"):
        return {"error": response.get("error", "unknown error"), "response": response}
    output = str(response["output"])
    return {
        "error": None,
        "response": response,
        "eval": server_bench.score_case(case, output, "hinglish"),
    }


def stats(rows: list[dict[str, Any]]) -> dict[str, Any]:
    completed = [row for row in rows if not row.get("error")]
    passed = [row for row in completed if row["eval"]["passed"]]
    latencies = sorted(float(row["response"]["wall_s"]) for row in completed)
    strict: dict[str, int] = {}
    for row in completed:
        for failure in row["eval"]["strict"]:
            strict[failure] = strict.get(failure, 0) + 1
    return {
        "completed": len(completed),
        "passed": len(passed),
        "mean_score": round(statistics.mean(row["eval"]["score"] for row in completed), 3) if completed else 0.0,
        "median_s": round(statistics.median(latencies), 3) if latencies else None,
        "strict": strict,
    }


def write_report(
    *,
    stamp: str,
    categories: set[str],
    cases: list[dict[str, Any]],
    spark: list[dict[str, Any]],
    gemma: list[dict[str, Any]],
) -> Path:
    run_dir = OUT_DIR / f"correction_spark_vs_gemma_{stamp}"
    run_dir.mkdir(parents=True, exist_ok=True)
    spark_stats, gemma_stats = stats(spark), stats(gemma)
    payload = {
        "benchmark_type": "correction_behavior_agent_vs_direct_api",
        "categories": sorted(categories),
        "spark": {"model": SPARK_MODEL, "transport": "codex_exec", "summary": spark_stats, "results": spark},
        "gemma": {"model": GEMMA_MODEL, "transport": "cerebras_chat_completions", "summary": gemma_stats, "results": gemma},
    }
    (run_dir / "results.json").write_text(
        json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )

    lines = [
        f"# Correction benchmark: Codex Spark vs Cerebras Gemma 4 - {stamp}",
        "",
        "Cases and scoring come from `lab/stress_suite.py` and `lab/server_bench.py`.",
        "Spark is an authenticated Codex agent invocation; Gemma is a direct API call.",
        "",
        f"- Categories: {', '.join(f'`{item}`' for item in sorted(categories))}",
        "",
        "| Route | Model | Pass | Mean score | Median latency | Strict failures |",
        "|---|---|---:|---:|---:|---|",
    ]
    for label, model, summary in (("Codex agent", SPARK_MODEL, spark_stats), ("Direct API", GEMMA_MODEL, gemma_stats)):
        failures = ", ".join(f"{name} x{count}" for name, count in sorted(summary["strict"].items())) or "-"
        latency = f"{summary['median_s']:.2f}s" if summary["median_s"] is not None else "-"
        lines.append(
            f"| {label} | `{model}` | {summary['passed']}/{summary['completed']} | "
            f"{summary['mean_score']:.2f}/5 | {latency} | {failures} |"
        )
    lines.extend(["", "## Cases", ""])
    for index, case in enumerate(cases):
        spark_row, gemma_row = spark[index], gemma[index]
        lines.extend([f"### `{case['id']}` - {case['category']}", "", f"- Raw: `{case['transcript']}`"])
        for label, row in (("Spark", spark_row), ("Gemma", gemma_row)):
            if row.get("error"):
                lines.append(f"- {label}: ERROR `{row['error']}`")
                continue
            result = row["response"]
            verdict = row["eval"]
            lines.append(
                f"- {label}: score {verdict['score']}/5, {result['wall_s']:.2f}s, "
                f"{verdict['diagnosis']}\n  - Output: `{result['output']}`"
            )
        lines.append("")
    (run_dir / "report.md").write_text("\n".join(lines), encoding="utf-8")
    return run_dir


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--categories",
        default=",".join(sorted(DEFAULT_CATEGORIES)),
        help="Comma-separated stress-suite categories (default: correction-critical set)",
    )
    parser.add_argument("--limit", type=int, default=None, help="Limit selected cases")
    parser.add_argument("--delay", type=float, default=0.15, help="Delay between calls in seconds")
    parser.add_argument("--timeout", type=int, default=90, help="Per-call timeout seconds")
    parser.add_argument("--dry-run", action="store_true", help="List cases without calls")
    args = parser.parse_args()
    if args.limit is not None and args.limit < 1:
        raise SystemExit("--limit must be >= 1")

    categories = {item.strip() for item in args.categories.split(",") if item.strip()}
    unknown = categories - set(stress_suite.CATEGORIES)
    if unknown:
        raise SystemExit(f"Unknown categories: {', '.join(sorted(unknown))}")
    cases = stress_suite.cases_for(categories=categories)
    if args.limit:
        cases = cases[: args.limit]
    if not cases:
        raise SystemExit("No correction cases selected")
    if args.dry_run:
        for case in cases:
            print(f"{case['id']:12} {case['category']:16} {case['notes']}")
        return 0

    polish_lab.load_dotenv()
    api_key = os.getenv("CEREBRAS_API_KEY", "").strip()
    if not api_key:
        raise SystemExit("Set CEREBRAS_API_KEY in the gitignored root .env")
    system_prompt = render_production_system_prompt()
    spark_results: list[dict[str, Any]] = []
    gemma_results: list[dict[str, Any]] = []
    print(f"Cases: {len(cases)}; Spark={SPARK_MODEL}; Gemma={GEMMA_MODEL}")
    for index, case in enumerate(cases, start=1):
        user_message = polish_lab.build_user_message(str(case["transcript"]))
        spark_row = evaluate(case, run_spark(spark_prompt(system_prompt, user_message), args.timeout))
        spark_results.append(spark_row)
        print(f"[{index}/{len(cases)}] Spark {case['id']}: {spark_row.get('response', {}).get('wall_s', 0):.2f}s")
        time.sleep(args.delay)
        gemma_row = evaluate(
            case,
            call_gemma(
                system_prompt=system_prompt,
                user_message=user_message,
                api_key=api_key,
                timeout_s=args.timeout,
            ),
        )
        gemma_results.append(gemma_row)
        print(f"[{index}/{len(cases)}] Gemma {case['id']}: {gemma_row.get('response', {}).get('wall_s', 0):.2f}s")
        time.sleep(args.delay)

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    report_dir = write_report(
        stamp=stamp,
        categories=categories,
        cases=cases,
        spark=spark_results,
        gemma=gemma_results,
    )
    print(f"Report: {report_dir.relative_to(REPO)}/report.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
