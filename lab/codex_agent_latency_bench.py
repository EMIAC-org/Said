#!/usr/bin/env python3
"""Measure Codex Spark agent completion latency against Cerebras Gemma 4.

This is intentionally an end-to-end comparison, not a raw model benchmark:
Spark is invoked through the authenticated Codex CLI, while Gemma is invoked
through Cerebras's public API. The report keeps those routes distinct.

  python lab/codex_agent_latency_bench.py --runs 5 --warmup 1
  python lab/codex_agent_latency_bench.py --prompt "Reply exactly: benchmark ok"
  python lab/codex_agent_latency_bench.py --dry-run
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LAB = Path(__file__).resolve().parent
REPO = LAB.parent
OUT_DIR = LAB / "latency_runs"
CEREBRAS_BASE = "https://api.cerebras.ai/v1"
GEMMA_MODEL = "gemma-4-31b"
SPARK_MODEL = "gpt-5.3-codex-spark"
DEFAULT_PROMPT = "Reply with exactly: latency benchmark ok"


def load_dotenv() -> None:
    env_path = REPO / ".env"
    if not env_path.is_file():
        return
    for line in env_path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key, value = key.strip(), value.strip().strip("\"'")
        if key and key not in os.environ:
            os.environ[key] = value


def percentile(values: list[float], fraction: float) -> float:
    if len(values) == 1:
        return values[0]
    index = (len(values) - 1) * fraction
    low = int(index)
    high = min(low + 1, len(values) - 1)
    return values[low] + (values[high] - values[low]) * (index - low)


def summarize(rows: list[dict[str, Any]]) -> dict[str, float] | None:
    values = sorted(float(row["wall_s"]) for row in rows if row.get("ok"))
    if not values:
        return None
    return {
        "min_s": values[0],
        "median_s": statistics.median(values),
        "mean_s": statistics.mean(values),
        "p95_s": percentile(values, 0.95),
    }


def run_spark(prompt: str, timeout_s: int) -> dict[str, Any]:
    """Use the supported Codex client auth path; never read its credentials."""
    codex = shutil.which("codex")
    if not codex:
        return {"ok": False, "error": "Codex CLI is not on PATH", "wall_s": 0.0}

    with tempfile.TemporaryDirectory(prefix="airnote-codex-bench-") as cwd:
        command = [
            codex,
            "exec",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--model",
            SPARK_MODEL,
            "--cd",
            cwd,
            prompt,
        ]
        started = time.perf_counter()
        try:
            completed = subprocess.run(
                command,
                capture_output=True,
                text=True,
                timeout=timeout_s,
                check=False,
            )
        except subprocess.TimeoutExpired:
            return {
                "ok": False,
                "wall_s": time.perf_counter() - started,
                "error": f"timed out after {timeout_s}s",
            }

    wall_s = time.perf_counter() - started
    output = completed.stdout.strip()
    if completed.returncode != 0:
        return {
            "ok": False,
            "wall_s": wall_s,
            "error": completed.stderr.strip()[-800:] or f"exit {completed.returncode}",
            "output": output[-800:],
        }
    return {"ok": True, "wall_s": wall_s, "output": output[-800:]}


def run_gemma(prompt: str, api_key: str, timeout_s: int) -> dict[str, Any]:
    payload = {
        "model": GEMMA_MODEL,
        "messages": [
            {
                "role": "system",
                "content": "Follow the user instruction exactly. Return only the requested text.",
            },
            {"role": "user", "content": prompt},
        ],
        "temperature": 0.0,
        # A small explicit cap prevents Cerebras reserving the full context window.
        "max_completion_tokens": 64,
        "stream": False,
    }
    request = urllib.request.Request(
        f"{CEREBRAS_BASE}/chat/completions",
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "User-Agent": "airnote-lab/1.0",
        },
        method="POST",
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(request, timeout=timeout_s) as response:
            body = json.loads(response.read().decode("utf-8"))
        wall_s = time.perf_counter() - started
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
        return {"ok": False, "wall_s": wall_s, "error": "Cerebras returned no content"}
    return {"ok": True, "wall_s": wall_s, "output": output, "usage": body.get("usage", {})}


def run_target(
    name: str,
    call: Any,
    *,
    warmup: int,
    runs: int,
) -> list[dict[str, Any]]:
    for index in range(warmup):
        result = call()
        state = "ok" if result.get("ok") else "failed"
        print(f"warmup {name} {index + 1}/{warmup}: {state} ({result['wall_s']:.2f}s)")

    rows: list[dict[str, Any]] = []
    for index in range(runs):
        result = call()
        result["run"] = index + 1
        rows.append(result)
        state = "ok" if result.get("ok") else "failed"
        print(f"run {name} {index + 1}/{runs}: {state} ({result['wall_s']:.2f}s)")
    return rows


def write_report(
    *,
    stamp: str,
    prompt: str,
    warmup: int,
    spark: list[dict[str, Any]],
    gemma: list[dict[str, Any]],
) -> Path:
    run_dir = OUT_DIR / f"codex_agent_vs_gemma_{stamp}"
    run_dir.mkdir(parents=True, exist_ok=True)
    summaries = {"spark": summarize(spark), "gemma": summarize(gemma)}
    payload = {
        "benchmark_type": "end_to_end_agent_vs_direct_api",
        "prompt": prompt,
        "warmup_runs": warmup,
        "spark": {"model": SPARK_MODEL, "transport": "codex_exec", "results": spark},
        "gemma": {"model": GEMMA_MODEL, "transport": "cerebras_chat_completions", "results": gemma},
        "summary": summaries,
    }
    (run_dir / "results.json").write_text(
        json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )

    lines = [
        f"# Codex Spark agent vs Cerebras Gemma 4 latency - {stamp}",
        "",
        "This is an end-to-end transport comparison, not a raw model benchmark.",
        "Spark includes the Codex CLI agent/runtime path; Gemma is a direct API call.",
        "",
        f"- Prompt: `{prompt}`",
        f"- Warmups per route: {warmup}",
        "",
        "| Route | Model | Successful | Min | Median | Mean | P95 |",
        "|---|---|---:|---:|---:|---:|---:|",
    ]
    for label, model, rows in (("Codex agent", SPARK_MODEL, spark), ("Direct API", GEMMA_MODEL, gemma)):
        summary = summaries["spark" if label == "Codex agent" else "gemma"]
        successful = sum(1 for row in rows if row.get("ok"))
        if summary:
            lines.append(
                f"| {label} | `{model}` | {successful}/{len(rows)} | "
                f"{summary['min_s']:.2f}s | {summary['median_s']:.2f}s | "
                f"{summary['mean_s']:.2f}s | {summary['p95_s']:.2f}s |"
            )
        else:
            lines.append(f"| {label} | `{model}` | 0/{len(rows)} | failed | failed | failed | failed |")
    lines.extend(["", "## Per-run results", ""])
    for label, rows in (("Spark", spark), ("Gemma", gemma)):
        lines.extend([f"### {label}", ""])
        for row in rows:
            detail = row.get("error") or row.get("output", "")
            lines.append(f"- Run {row['run']}: {row['wall_s']:.2f}s - {detail}")
        lines.append("")
    (run_dir / "report.md").write_text("\n".join(lines), encoding="utf-8")
    return run_dir


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=int, default=5, help="Measured calls per route (default: 5)")
    parser.add_argument("--warmup", type=int, default=1, help="Warmup calls per route (default: 1)")
    parser.add_argument("--timeout", type=int, default=90, help="Per-call timeout seconds (default: 90)")
    parser.add_argument("--prompt", default=DEFAULT_PROMPT, help="Same instruction sent to both routes")
    parser.add_argument("--dry-run", action="store_true", help="Validate prerequisites without calls")
    args = parser.parse_args()
    if args.runs < 1 or args.warmup < 0 or args.timeout < 1:
        raise SystemExit("--runs must be >= 1; --warmup must be >= 0; --timeout must be >= 1")

    load_dotenv()
    if not shutil.which("codex"):
        raise SystemExit("Codex CLI is not on PATH")
    if args.dry_run:
        print(f"Spark: {SPARK_MODEL} via authenticated codex exec")
        print(f"Gemma: {GEMMA_MODEL} via Cerebras chat completions")
        print("No calls made.")
        return 0

    api_key = os.getenv("CEREBRAS_API_KEY", "").strip()
    if not api_key:
        raise SystemExit("Set CEREBRAS_API_KEY in the gitignored repo .env before running.")

    spark = run_target(
        "spark",
        lambda: run_spark(args.prompt, args.timeout),
        warmup=args.warmup,
        runs=args.runs,
    )
    gemma = run_target(
        "gemma",
        lambda: run_gemma(args.prompt, api_key, args.timeout),
        warmup=args.warmup,
        runs=args.runs,
    )
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    report_dir = write_report(
        stamp=stamp,
        prompt=args.prompt,
        warmup=args.warmup,
        spark=spark,
        gemma=gemma,
    )
    print(f"Report: {report_dir.relative_to(REPO)}/report.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
