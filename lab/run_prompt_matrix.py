#!/usr/bin/env python3
"""Run a batch of polish prompts against the cached lab transcript.

This keeps prompt iteration honest: the STT transcript stays fixed, only the
system prompt changes. Results are saved as one timestamped matrix report.
"""

from __future__ import annotations

import argparse
import collections
import json
import time
from datetime import datetime, timezone
from pathlib import Path

import sys

LAB = Path(__file__).resolve().parent
if str(LAB) not in sys.path:
    sys.path.insert(0, str(LAB))

import polish_lab
from scoring import score_output

REPO = LAB.parent
PROMPT_DIR = LAB / "prompt_matrix"
OUT_DIR = LAB / "matrix_runs"


def route_slug(route: dict[str, str]) -> str:
    model_name = route["model"].split("/")[-1].lower()
    safe_model = "".join(ch if ch.isalnum() else "-" for ch in model_name).strip("-")
    return f"{route['provider']}-{safe_model[:36]}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--prompts",
        type=Path,
        default=PROMPT_DIR,
        help="Directory containing .md system prompts",
    )
    parser.add_argument("--limit", type=int, default=0, help="Run only first N prompts")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="List prompts and cached transcript without calling the polish model",
    )
    parser.add_argument(
        "--default-route-only",
        action="store_true",
        help="Run only the default polish route instead of all configured routes",
    )
    args = parser.parse_args()

    polish_lab.load_dotenv()
    cache = polish_lab.load_cache()
    if not cache or not cache.get("transcript"):
        raise SystemExit("No cached transcript. Run lab/polish_lab.py with a WAV first.")

    prompt_dir = args.prompts.expanduser().resolve()
    prompts = sorted(prompt_dir.glob("*.md"))
    if args.limit > 0:
        prompts = prompts[: args.limit]
    if not prompts:
        raise SystemExit(f"No prompt .md files found in {prompt_dir}")
    routes = (
        [polish_lab.resolve_polish_route()]
        if args.default_route_only
        else polish_lab.resolve_polish_routes()
    )

    transcript = cache["transcript"]
    if args.dry_run:
        print(f"Cached transcript:\n{transcript}\n")
        print("Routes:")
        for route in routes:
            print(f"- {route['provider']}: {route['model']}")
        print()
        print("Prompts:")
        for prompt in prompts:
            print(f"- {prompt.relative_to(REPO)}")
        return 0

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = OUT_DIR / stamp
    run_dir.mkdir(parents=True, exist_ok=True)

    results: list[dict[str, object]] = []
    for idx, prompt_path in enumerate(prompts, start=1):
        system_prompt = prompt_path.read_text(encoding="utf-8").strip()
        for route_idx, route in enumerate(routes, start=1):
            print(
                f"[{idx}/{len(prompts)}][{route_idx}/{len(routes)}] "
                f"{prompt_path.name} via {route['provider']} / {route['model']}..."
            )
            t0 = time.perf_counter()
            polished, polish_s, actual_route = polish_lab.polish_transcript(
                transcript, system_prompt, route
            )
            elapsed = time.perf_counter() - t0
            metrics = score_output(polished)
            result = {
                "prompt": prompt_path.name,
                "prompt_path": str(prompt_path.relative_to(REPO)),
                "provider": actual_route["provider"],
                "model": actual_route["model"],
                "route_slug": route_slug(actual_route),
                "polish_s": polish_s,
                "elapsed_s": elapsed,
                "polished": polished,
                **metrics,
            }
            results.append(result)
            (run_dir / f"{idx:02d}-{route_slug(actual_route)}-{prompt_path.stem}.md").write_text(
                "\n".join(
                    [
                        f"# {prompt_path.name}",
                        "",
                        f"- Provider: `{actual_route['provider']}`",
                        f"- Model: `{actual_route['model']}`",
                        f"- Score: `{metrics['score']}`",
                        f"- Expected hits: {', '.join(metrics['expected_hits']) or '-'}",
                        f"- Missing: {', '.join(metrics['missing_terms']) or '-'}",
                        f"- Bad garbles: {', '.join(metrics['bad_hits']) or '-'}",
                        f"- Polish: {polish_s:.2f}s",
                        "",
                        "## Output",
                        "",
                        polished,
                        "",
                    ]
                ),
                encoding="utf-8",
            )

    ranked = sorted(results, key=lambda item: int(item["score"]), reverse=True)
    by_route: dict[str, list[dict[str, object]]] = collections.defaultdict(list)
    for item in results:
        by_route[str(item["route_slug"])].append(item)
    report_lines = [
        f"# Prompt matrix run - {stamp}",
        "",
        f"- Transcript source: `{cache.get('wav_path', '?')}`",
        f"- Prompt count: {len(prompts)}",
        f"- Route count: {len(routes)}",
        f"- Total calls: {len(results)}",
        "",
        "## Routes",
        "",
        "| Provider | Model |",
        "|---|---|",
    ]
    for route in routes:
        report_lines.append(f"| `{route['provider']}` | `{route['model']}` |")
    report_lines.extend(
        [
            "",
            "## Fixed Raw Transcript",
            "",
            transcript,
            "",
            "## Model Summary",
            "",
            "| Provider | Model | Top prompt | Top score | Average score | Average latency |",
            "|---|---|---|---:|---:|---:|",
        ]
    )
    for _slug, items in sorted(by_route.items()):
        top = max(items, key=lambda item: int(item["score"]))
        avg_score = sum(int(item["score"]) for item in items) / len(items)
        avg_latency = sum(float(item["polish_s"]) for item in items) / len(items)
        report_lines.append(
            f"| `{top['provider']}` | `{top['model']}` | `{top['prompt']}` | {top['score']} | {avg_score:.1f} | {avg_latency:.2f}s |"
        )
    report_lines.extend(
        [
            "",
            "## Ranking",
            "",
            "| Rank | Provider | Model | Prompt | Score | Latency | Hits | Missing | Bad garbles |",
            "|---:|---|---|---|---:|---:|---|---|---|",
        ]
    )
    for rank, item in enumerate(ranked, start=1):
        report_lines.append(
            "| {rank} | `{provider}` | `{model}` | `{prompt}` | {score} | {latency:.2f}s | {hits} | {missing} | {bad} |".format(
                rank=rank,
                provider=item["provider"],
                model=item["model"],
                prompt=item["prompt"],
                score=item["score"],
                latency=float(item["polish_s"]),
                hits=", ".join(item["expected_hits"]) or "-",
                missing=", ".join(item["missing_terms"]) or "-",
                bad=", ".join(item["bad_hits"]) or "-",
            )
        )
    report_lines.extend(
        [
            "",
            "## Outputs",
            "",
        ]
    )
    for item in ranked:
        report_lines.extend(
            [
                f"### {item['provider']} / {item['model']} - {item['prompt']} - score {item['score']}",
                "",
                str(item["polished"]),
                "",
            ]
        )

    (run_dir / "report.md").write_text("\n".join(report_lines), encoding="utf-8")
    (run_dir / "results.json").write_text(
        json.dumps({"results": results}, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )

    print(f"\nSaved matrix report -> {run_dir.relative_to(REPO)}/report.md")
    print("Top 3:")
    for item in ranked[:3]:
        print(f"- {item['provider']} / {item['model']} / {item['prompt']}: {item['score']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
