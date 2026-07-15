#!/usr/bin/env python3
"""Parallel polish shootout — same transcript + prompt, ~10 models at once.

Uses the cached STT transcript from polish_lab and sends identical
system + user messages to every model in lab/model_catalog.py (models
with a configured API key). Results are ranked by heuristic quality score.

  python lab/compare_models.py
  python lab/compare_models.py --dry-run
  python lab/compare_models.py --workers 6
  python lab/compare_models.py --provider groq
  python lab/compare_models.py --slug groq-scout,di-maverick-fp8
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

LAB = Path(__file__).resolve().parent
if str(LAB) not in sys.path:
    sys.path.insert(0, str(LAB))

import polish_lab
from model_catalog import LAB_MODEL_CATALOG, available_lab_routes
from scoring import score_output
REPO = LAB.parent
OUT_DIR = LAB / "model_runs"
DEFAULT_PROMPT = LAB / "prompt_system.md"


def run_one(
    *,
    transcript: str,
    system_prompt: str,
    route: dict,
    prompt_path: Path,
) -> dict[str, object]:
    slug = str(route.get("slug", route["model"]))
    t0 = time.perf_counter()
    result = polish_lab.polish_try(transcript, system_prompt, route)
    wall_s = time.perf_counter() - t0
    base = {
        "slug": slug,
        "label": route.get("label", route["model"]),
        "provider": route["provider"],
        "model": route["model"],
        "prompt": prompt_path.name,
        "prompt_path": str(prompt_path.relative_to(REPO)),
        "wall_s": wall_s,
    }
    if not result["ok"]:
        return {
            **base,
            "ok": False,
            "error": result["error"],
            "polish_s": None,
            "polished": "",
            "score": -999,
            "expected_hits": [],
            "missing_terms": [],
            "bad_hits": [],
        }
    polished = str(result["polished"])
    metrics = score_output(polished)
    return {
        **base,
        "ok": True,
        "error": None,
        "polish_s": result["polish_s"],
        "polished": polished,
        **metrics,
    }


def write_report(
    *,
    stamp: str,
    run_dir: Path,
    transcript: str,
    cache: dict,
    prompt_path: Path,
    results: list[dict[str, object]],
    workers: int,
) -> None:
    ok_results = [r for r in results if r.get("ok")]
    ranked = sorted(results, key=lambda r: int(r.get("score", -999)), reverse=True)

    lines = [
        f"# Model shootout — {stamp}",
        "",
        f"- Transcript: `{cache.get('wav_path', '?')}`",
        f"- Prompt: `{prompt_path.relative_to(REPO)}`",
        f"- Models requested: {len(results)}",
        f"- Models succeeded: {len(ok_results)}",
        f"- Parallel workers: {workers}",
        "",
        "## Fixed raw transcript",
        "",
        transcript,
        "",
        "## Ranking (quality heuristic)",
        "",
        "| Rank | Slug | Provider | Model | Score | Latency | Hits | Missing | Bad |",
        "|---:|---|---|---|---:|---:|---|---|---|",
    ]
    for rank, item in enumerate(ranked, start=1):
        if not item.get("ok"):
            lines.append(
                f"| {rank} | `{item['slug']}` | `{item['provider']}` | "
                f"`{item['model']}` | ERR | — | — | — | {item.get('error', '?')} |"
            )
            continue
        lines.append(
            "| {rank} | `{slug}` | `{provider}` | `{model}` | {score} | {lat:.2f}s | {hits} | {miss} | {bad} |".format(
                rank=rank,
                slug=item["slug"],
                provider=item["provider"],
                model=item["model"],
                score=item["score"],
                lat=float(item.get("polish_s") or item.get("wall_s") or 0),
                hits=", ".join(item.get("expected_hits") or []) or "-",
                miss=", ".join(item.get("missing_terms") or []) or "-",
                bad=", ".join(item.get("bad_hits") or []) or "-",
            )
        )

    lines.extend(["", "## Outputs", ""])
    for item in ranked:
        if not item.get("ok"):
            lines.extend(
                [
                    f"### {item['slug']} — FAILED",
                    "",
                    f"Error: {item.get('error')}",
                    "",
                ]
            )
            continue
        lines.extend(
            [
                f"### {item['slug']} — score {item['score']} ({item['label']})",
                "",
                str(item["polished"]),
                "",
            ]
        )

    (run_dir / "report.md").write_text("\n".join(lines), encoding="utf-8")
    (run_dir / "results.json").write_text(
        json.dumps(
            {
                "stamp": stamp,
                "transcript": transcript,
                "wav_path": cache.get("wav_path"),
                "prompt": str(prompt_path.relative_to(REPO)),
                "workers": workers,
                "results": results,
            },
            indent=2,
            ensure_ascii=False,
        )
        + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--prompt",
        type=Path,
        default=DEFAULT_PROMPT,
        help="System prompt file (default: prompt_system.md)",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=10,
        help="Max parallel API calls (default: 10)",
    )
    parser.add_argument(
        "--provider",
        choices=("groq", "together", "deepinfra"),
        action="append",
        help="Limit to provider(s); repeat flag for multiple",
    )
    parser.add_argument(
        "--slug",
        default="",
        help="Comma-separated catalog slugs to run (default: all with API keys)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="List models that would run; no API calls",
    )
    args = parser.parse_args()

    polish_lab.load_dotenv()
    cache = polish_lab.load_cache()
    if not cache or not cache.get("transcript"):
        raise SystemExit("No cached transcript. Run: python lab/polish_lab.py /path/to.wav")

    providers = set(args.provider) if args.provider else None
    slugs = {s.strip() for s in args.slug.split(",") if s.strip()} or None
    routes = available_lab_routes(LAB_MODEL_CATALOG, providers=providers, slugs=slugs)
    if not routes:
        raise SystemExit(
            "No models available. Set GROQ_API_KEY, DEEPINFRA_API_KEY, and/or OPENROUTER_API_KEY in .env"
        )

    prompt_path = args.prompt.expanduser().resolve()
    if not prompt_path.is_file():
        raise SystemExit(f"Prompt not found: {prompt_path}")
    system_prompt = prompt_path.read_text(encoding="utf-8").strip()
    transcript = str(cache["transcript"])

    print(f"Catalog: {len(LAB_MODEL_CATALOG)} models | runnable: {len(routes)}")
    for route in routes:
        print(f"  - {route['slug']}: {route['provider']} / {route['model']}")

    if args.dry_run:
        print(f"\nTranscript ({len(transcript.split())} words):\n{transcript[:200]}...")
        return 0

    workers = max(1, min(args.workers, len(routes)))
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = OUT_DIR / stamp
    run_dir.mkdir(parents=True, exist_ok=True)

    print(f"\nPolishing in parallel (workers={workers})...")
    t0 = time.perf_counter()
    results: list[dict[str, object]] = []

    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {
            pool.submit(
                run_one,
                transcript=transcript,
                system_prompt=system_prompt,
                route=route,
                prompt_path=prompt_path,
            ): route
            for route in routes
        }
        for future in as_completed(futures):
            route = futures[future]
            slug = route.get("slug", route["model"])
            try:
                item = future.result()
            except Exception as exc:
                item = {
                    "slug": slug,
                    "label": route.get("label", route["model"]),
                    "provider": route["provider"],
                    "model": route["model"],
                    "ok": False,
                    "error": str(exc),
                    "score": -999,
                    "polished": "",
                }
            results.append(item)
            if item.get("ok"):
                print(
                    f"  done {slug}: score={item['score']} "
                    f"latency={float(item.get('polish_s') or 0):.2f}s"
                )
            else:
                print(f"  FAIL {slug}: {item.get('error')}")

    total_s = time.perf_counter() - t0
    results.sort(key=lambda r: int(r.get("score", -999)), reverse=True)

    for item in results:
        if not item.get("ok"):
            continue
        safe = str(item["slug"])
        (run_dir / f"{safe}.md").write_text(
            "\n".join(
                [
                    f"# {item['label']}",
                    "",
                    f"- Score: {item['score']}",
                    f"- Provider: `{item['provider']}`",
                    f"- Model: `{item['model']}`",
                    f"- Latency: {float(item.get('polish_s') or 0):.2f}s",
                    "",
                    str(item["polished"]),
                    "",
                ]
            ),
            encoding="utf-8",
        )

    write_report(
        stamp=stamp,
        run_dir=run_dir,
        transcript=transcript,
        cache=cache,
        prompt_path=prompt_path,
        results=results,
        workers=workers,
    )

    print(f"\nWall time: {total_s:.2f}s → {run_dir.relative_to(REPO)}/report.md")
    print("Top 3:")
    for item in results[:3]:
        if item.get("ok"):
            print(f"  {item['slug']}: score={item['score']} — {item['label']}")
        else:
            print(f"  {item['slug']}: FAILED")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
