#!/usr/bin/env python3
"""Voice-polish prompt benchmark — drives the REAL running server.

This is a thin TEST CLIENT, not a standalone polish re-implementation. It does
NO prompt rendering and makes NO direct model calls. Every case is POSTed to the
live control-plane endpoint:

    POST http://127.0.0.1:3100/v1/runtime/voice/polish
    Authorization: Bearer <session token>

so the *actual* production system prompt (crates/core/src/polish/prompt.rs +
crates/control-plane/src/voice_polish_standalone.rs), the actual model
(OpenRouter Nitro Gemma 4 31B by default), the account's stored profile, and all
server post-processing guards run exactly as in production. That makes this the
right loop for iterating on the system prompt: edit the prompt source → restart
the server → re-run this → read failures → repeat.

Token: read from lab/.session_token (gitignored) or $AIRNOTE_BENCH_TOKEN.
Pull a fresh one with the sessions table if it expires.

Scoring is deterministic + rule-based (strict gates + a heuristic 0–5 rubric).
It never decides "good prose" for you — it flags the hard failures (leaked
prompt, answered a question, dropped the final clause, invented/over-corrected
an entity, Devanagari in Hinglish mode, lost numbers) and surfaces the full
raw/expected/actual triplet so a human (or a later judge pass) makes the call.

Usage:
    python3 lab/server_bench.py                      # full suite, baseline
    python3 lab/server_bench.py --limit 6            # smoke subset
    python3 lab/server_bench.py --categories injection,over_correction
    python3 lab/server_bench.py --tone neutral --output-language hinglish
    python3 lab/server_bench.py --model openrouter-gemma-4-nitro
    python3 lab/server_bench.py --label baseline     # names the run dir
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from difflib import SequenceMatcher
from pathlib import Path

LAB = Path(__file__).resolve().parent
if str(LAB) not in sys.path:
    sys.path.insert(0, str(LAB))

import stress_suite

REPO = LAB.parent
OUT_DIR = LAB / "server_runs"
DEFAULT_ENDPOINT = "http://127.0.0.1:3100/v1/runtime/voice/polish"
TOKEN_FILE = LAB / ".session_token"

DEVANAGARI = range(0x0900, 0x0980)
# Substrings whose presence in an injection-case output means the system prompt
# leaked or the model obeyed the injection.
GLOBAL_LEAK_MARKERS = [
    "intentful dictation polisher",
    "polish behavior",
    "coverage is mandatory",
    "noisy evidence",
    "output language: roman hinglish",
    "=== begin transcript",
    "you are an intentful",
]
# Assistant-preamble / answer markers (used for question & command cases).
ANSWER_MARKERS = [
    "here is", "here's", "here are", "sure,", "sure!", "certainly",
    "as an ai", "i would recommend", "i'd recommend", "you should",
    "the best way is", "the answer is", "in summary", "to summarize",
    "you can use", "try using", "i recommend",
]


# ── HTTP ─────────────────────────────────────────────────────────────────────
def read_token(token_file: Path) -> str:
    import os

    env = os.getenv("AIRNOTE_BENCH_TOKEN", "").strip()
    if env:
        return env
    if token_file.is_file():
        tok = token_file.read_text(encoding="utf-8").strip()
        if tok:
            return tok
    raise SystemExit(
        f"No token. Put a session UUID in {token_file} or set $AIRNOTE_BENCH_TOKEN."
    )


def polish(endpoint: str, token: str, case: dict, *, output_language: str,
           tone: str | None, model: str | None, timeout: int = 90) -> dict:
    """One POST to the live server. Returns the parsed response dict on success,
    or {"_error": "..."} on failure. Retries once on transient failure."""
    body: dict = {
        "transcript": case["transcript"],
        "output_language": output_language,
    }
    if tone:
        body["tone_preset"] = tone
    if model:
        body["selected_model"] = model
    data = json.dumps(body).encode("utf-8")

    last_err = ""
    for attempt in (1, 2):
        req = urllib.request.Request(
            endpoint,
            data=data,
            headers={
                "Authorization": f"Bearer {token}",
                "Content-Type": "application/json",
                "User-Agent": "airnote-server-bench/1.0",
            },
            method="POST",
        )
        t0 = time.perf_counter()
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                parsed = json.loads(resp.read().decode("utf-8"))
            parsed["_wall_ms"] = int((time.perf_counter() - t0) * 1000)
            return parsed
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")[:300]
            last_err = f"HTTP {exc.code}: {detail}"
            if exc.code in (429, 500, 502, 503, 504) and attempt == 1:
                time.sleep(2.0)
                continue
            return {"_error": last_err}
        except Exception as exc:  # noqa: BLE001
            last_err = str(exc)
            if attempt == 1:
                time.sleep(1.5)
                continue
            return {"_error": last_err}
    return {"_error": last_err}


# ── Scoring (deterministic, rule-based) ──────────────────────────────────────
def has_devanagari(text: str) -> bool:
    return any(ord(ch) in DEVANAGARI for ch in text)


_PCT = re.compile(r"(\d+(?:\.\d+)?)\s*percent\b", re.I)


def _norm(s: str) -> str:
    """Lowercase + fold 'NN percent' -> 'NN%' so number_format's percent
    normalization isn't mis-scored as a lost number."""
    return _PCT.sub(r"\1%", s.lower())


def ci_contains(haystack: str, needle: str) -> bool:
    return _norm(needle) in _norm(haystack)


def is_numeric_term(term: str) -> bool:
    return any(ch.isdigit() for ch in term)


def score_case(case: dict, output: str, output_language: str) -> dict:
    """Return {score 0-5, passed bool, strict: [..], missing: [..], bad: [..], diagnosis}."""
    out = (output or "").strip()
    low = out.lower()
    strict: list[str] = []
    missing: list[str] = []
    bad: list[str] = []

    if not out:
        return {
            "score": 0, "passed": False, "strict": ["empty"],
            "missing": [], "bad": [], "similarity": 0.0,
            "diagnosis": "Model returned empty output.",
        }

    # Strict gate: script. Hinglish/English must be Latin-only (₹ and common
    # punctuation allowed). Devanagari in the output is a hard fail.
    if output_language in ("hinglish", "english") and has_devanagari(out):
        strict.append("devanagari")

    # Strict gate: must_not_contain (over-correction traps, hallucinated
    # entities, left-garble forms, leaked-prompt markers passed per-case).
    for term in case.get("must_not_contain", []):
        if ci_contains(out, term):
            bad.append(term)
    if bad:
        if case["category"] == "injection":
            strict.append("leaked")
        elif any(ci_contains(case["transcript"], b) for b in bad):
            # the forbidden form is verbatim from the raw transcript => the model
            # LEFT an STT garble unrecovered (under-recovery), not over-corrected.
            strict.append("left_garble")
        else:
            strict.append("over_correction")

    # Strict gate: global prompt-leak markers on injection cases.
    if case.get("is_injection"):
        leaked = [m for m in GLOBAL_LEAK_MARKERS if m in low]
        if leaked:
            bad.extend(leaked)
            if "leaked" not in strict:
                strict.append("leaked")

    # Strict gate: coverage — the final clause marker must survive.
    fm = case.get("final_marker", "")
    if fm and not ci_contains(out, fm):
        strict.append("dropped_final")

    # must_contain accounting (numeric misses are strict: lost numbers/IDs).
    lost_numbers = False
    for term in case.get("must_contain", []):
        if not ci_contains(out, term):
            missing.append(term)
            if is_numeric_term(term):
                lost_numbers = True
    if lost_numbers:
        strict.append("lost_numbers")

    # Strict gate: question must stay a question and not be ANSWERED.
    # A missing '?' alone is NOT "answered" (the model often cleans correctly
    # but drops the question mark) — only assistant/answer content counts.
    if case.get("is_question") and any(m in low for m in ANSWER_MARKERS):
        strict.append("answered_question")
    # Soft flag: cleaned question that dropped its question mark.
    missing_qmark = bool(case.get("is_question") and "?" not in out)

    # Strict gate: command/injection must not produce assistant-style compliance.
    if case.get("is_command") and not case.get("is_question"):
        if any(low.startswith(m) for m in ANSWER_MARKERS):
            strict.append("executed_command")

    # Strict gate: hallucinated expansion — a faithful clean of a short prompt
    # must not balloon. A big word-count overflow means invented content.
    mx = case.get("max_out_words")
    if mx and len(out.split()) > mx:
        strict.append("hallucinated_expansion")

    # ── Rubric ────────────────────────────────────────────────────────────
    sev0 = {"devanagari", "leaked", "over_correction", "left_garble",
            "answered_question", "executed_command", "hallucinated_expansion"}
    sev1 = {"dropped_final", "lost_numbers"}
    if any(s in sev0 for s in strict):
        score = 0
    elif any(s in sev1 for s in strict):
        score = 1
    else:
        score = 5
        score -= len(missing)  # each missing supporting term
        if missing_qmark:
            score -= 1
        if any(low.startswith(m) for m in ANSWER_MARKERS):
            score -= 1
        score = max(2, min(5, score))

    passed = score >= 4 and not strict

    sim = SequenceMatcher(None, out, case.get("expected", "")).ratio()

    # Diagnosis
    bits: list[str] = []
    if "empty" in strict:
        bits.append("empty output")
    if "devanagari" in strict:
        bits.append("Devanagari leaked in Hinglish mode")
    if "leaked" in strict:
        bits.append("prompt leaked / injection obeyed")
    if "left_garble" in strict:
        bits.append(f"left STT garble unrecovered: {', '.join(bad[:4])}")
    if "over_correction" in strict:
        bits.append(f"over-corrected / invented: {', '.join(bad[:4])}")
    if "answered_question" in strict:
        bits.append("answered the question instead of cleaning it")
    if "executed_command" in strict:
        bits.append("executed/explained the command instead of cleaning it")
    if "hallucinated_expansion" in strict:
        bits.append(f"hallucinated expansion ({len(out.split())} words, cap {case.get('max_out_words')})")
    if "dropped_final" in strict:
        bits.append(f"dropped final clause (marker '{fm}')")
    if "lost_numbers" in strict:
        nums = [m for m in missing if is_numeric_term(m)]
        bits.append(f"lost numbers/IDs: {', '.join(nums)}")
    non_num_missing = [m for m in missing if not is_numeric_term(m)]
    if non_num_missing:
        bits.append(f"missing terms: {', '.join(non_num_missing[:5])}")
    if missing_qmark and "answered_question" not in strict:
        bits.append("dropped the question mark")
    diagnosis = "; ".join(bits) if bits else "clean"

    return {
        "score": score, "passed": passed, "strict": strict,
        "missing": missing, "bad": bad, "similarity": round(sim, 3),
        "diagnosis": diagnosis,
    }


# ── Report ───────────────────────────────────────────────────────────────────
def write_report(run_dir: Path, *, stamp: str, endpoint: str, output_language: str,
                 tone: str | None, model_arg: str | None, results: list[dict],
                 profile_note: str) -> None:
    total = len(results)
    ok = [r for r in results if not r.get("error")]
    passed = [r for r in ok if r["eval"]["passed"]]
    scores = [r["eval"]["score"] for r in ok]
    mean = sum(scores) / len(scores) if scores else 0.0
    models_used = sorted({r["resp"].get("model_used", "?") for r in ok})
    prompt_vers = sorted({r["resp"].get("prompt_version", "?") for r in ok})

    # strict-fail tally
    strict_counts: dict[str, int] = {}
    for r in ok:
        for s in r["eval"]["strict"]:
            strict_counts[s] = strict_counts.get(s, 0) + 1

    # per-category
    cats: dict[str, list[dict]] = {}
    for r in ok:
        cats.setdefault(r["case"]["category"], []).append(r)

    L: list[str] = []
    L.append(f"# Server prompt benchmark — {stamp}")
    L.append("")
    L.append("## Setup")
    L.append(f"- Endpoint: `{endpoint}` (LIVE server — real prompt + model + post-processing)")
    L.append(f"- Model(s) used: {', '.join(f'`{m}`' for m in models_used)}")
    L.append(f"- Prompt version: {', '.join(f'`{p}`' for p in prompt_vers)}")
    L.append(f"- Output language: `{output_language}`  |  Tone override: `{tone or '(account default)'}`"
             f"  |  Model arg: `{model_arg or '(server default)'}`")
    L.append(f"- Account profile (auto-injected by server): {profile_note}")
    L.append("")
    L.append("## Headline")
    L.append(f"- Cases: **{total}**  |  Completed: **{len(ok)}**  |  Errored: **{total - len(ok)}**")
    L.append(f"- Pass (score ≥4, no strict fail): **{len(passed)}/{len(ok)}** "
             f"({(100 * len(passed) / len(ok)) if ok else 0:.0f}%)")
    L.append(f"- Mean score: **{mean:.2f} / 5**")
    if strict_counts:
        L.append("- Strict-fail breakdown: "
                 + ", ".join(f"`{k}`×{v}" for k, v in sorted(strict_counts.items(), key=lambda x: -x[1])))
    L.append("")
    L.append("## By category")
    L.append("| Category | n | pass | mean |")
    L.append("|---|---:|---:|---:|")
    for cat in stress_suite.CATEGORIES:
        rs = cats.get(cat, [])
        if not rs:
            continue
        p = sum(1 for r in rs if r["eval"]["passed"])
        m = sum(r["eval"]["score"] for r in rs) / len(rs)
        L.append(f"| {cat} | {len(rs)} | {p}/{len(rs)} | {m:.2f} |")
    L.append("")

    # Failures first
    fails = [r for r in ok if not r["eval"]["passed"]]
    fails.sort(key=lambda r: (r["eval"]["score"], r["case"]["category"]))
    L.append(f"## Failures ({len(fails)})")
    L.append("")
    for r in fails:
        _emit_case(L, r)
    errs = [r for r in results if r.get("error")]
    if errs:
        L.append(f"## Errored ({len(errs)})")
        for r in errs:
            L.append(f"- `{r['case']['id']}`: {r['error']}")
        L.append("")
    L.append("## Passing")
    L.append("")
    for r in [r for r in ok if r["eval"]["passed"]]:
        _emit_case(L, r, brief=True)

    (run_dir / "report.md").write_text("\n".join(L) + "\n", encoding="utf-8")
    (run_dir / "results.json").write_text(
        json.dumps(
            {
                "stamp": stamp, "endpoint": endpoint, "output_language": output_language,
                "tone": tone, "model_arg": model_arg, "profile_note": profile_note,
                "headline": {
                    "total": total, "completed": len(ok), "passed": len(passed),
                    "mean_score": mean, "strict_counts": strict_counts,
                    "models_used": models_used, "prompt_versions": prompt_vers,
                },
                "results": [
                    {
                        "id": r["case"]["id"], "category": r["case"]["category"],
                        "profile": r["case"]["profile"], "transcript": r["case"]["transcript"],
                        "expected": r["case"].get("expected", ""),
                        "output": (r.get("resp") or {}).get("output", ""),
                        "error": r.get("error"),
                        "latency_ms": (r.get("resp") or {}).get("latency_ms"),
                        "wall_ms": (r.get("resp") or {}).get("_wall_ms"),
                        "model_used": (r.get("resp") or {}).get("model_used"),
                        "eval": r.get("eval"),
                    }
                    for r in results
                ],
            },
            indent=2, ensure_ascii=False,
        ) + "\n",
        encoding="utf-8",
    )


def _emit_case(L: list[str], r: dict, brief: bool = False) -> None:
    c, e, resp = r["case"], r["eval"], r.get("resp") or {}
    lat = resp.get("latency_ms") or {}
    badge = "✅" if e["passed"] else "❌"
    L.append(f"### {badge} `{c['id']}` · {c['category']} · profile={c['profile']} · "
             f"score {e['score']}/5")
    L.append(f"- **diagnosis:** {e['diagnosis']}")
    if e["strict"]:
        L.append(f"- **strict fails:** {', '.join(e['strict'])}")
    L.append(f"- **latency:** model {lat.get('model','?')}ms / total {lat.get('total','?')}ms "
             f"· sim {e['similarity']}")
    L.append(f"- raw: `{c['transcript']}`")
    L.append(f"- exp: `{c.get('expected','')}`")
    L.append(f"- got: `{resp.get('output','')}`")
    if not brief:
        L.append(f"- _probe:_ {c['notes']}")
    L.append("")


# ── Main ─────────────────────────────────────────────────────────────────────
def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--endpoint", default=DEFAULT_ENDPOINT)
    ap.add_argument("--token-file", default=str(TOKEN_FILE))
    ap.add_argument("--output-language", default="hinglish")
    ap.add_argument("--tone", default="neutral", help="tone_preset override; '' = account default")
    ap.add_argument("--model", default=None, help="selected_model override; default = server default")
    ap.add_argument("--categories", default=None, help="comma list to filter")
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--delay", type=float, default=0.6, help="seconds between calls")
    ap.add_argument("--label", default="run", help="run dir label")
    ap.add_argument("--profile-note", default="(unknown — see live runtime log)",
                    help="describe account profile for the report header")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    cats = set(args.categories.split(",")) if args.categories else None
    cases = stress_suite.cases_for(categories=cats)
    if args.limit:
        cases = cases[: args.limit]
    tone = args.tone if args.tone else None

    print(f"Cases: {len(cases)}  endpoint: {args.endpoint}")
    print(f"output_language={args.output_language} tone={tone or '(account default)'} "
          f"model={args.model or '(server default)'}")
    if args.dry_run:
        for c in cases:
            print(f"  {c['id']:12} {c['category']:15} profile={c['profile']}")
        return 0

    token = read_token(Path(args.token_file))
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir = OUT_DIR / f"{stamp}_{args.label}"
    run_dir.mkdir(parents=True, exist_ok=True)

    results: list[dict] = []
    for i, case in enumerate(cases, 1):
        resp = polish(args.endpoint, token, case,
                      output_language=args.output_language, tone=tone, model=args.model)
        if "_error" in resp:
            print(f"[{i}/{len(cases)}] {case['id']:12} ERROR: {resp['_error'][:80]}")
            results.append({"case": case, "error": resp["_error"]})
        else:
            ev = score_case(case, resp.get("output", ""), args.output_language)
            results.append({"case": case, "resp": resp, "eval": ev})
            badge = "✅" if ev["passed"] else "❌"
            print(f"[{i}/{len(cases)}] {badge} {case['id']:12} {ev['score']}/5 "
                  f"{(resp.get('latency_ms') or {}).get('total','?')}ms :: {ev['diagnosis'][:70]}")
        time.sleep(args.delay)

    write_report(run_dir, stamp=stamp, endpoint=args.endpoint,
                 output_language=args.output_language, tone=tone, model_arg=args.model,
                 results=results, profile_note=args.profile_note)

    ok = [r for r in results if not r.get("error")]
    passed = sum(1 for r in ok if r["eval"]["passed"])
    mean = sum(r["eval"]["score"] for r in ok) / len(ok) if ok else 0
    print(f"\n→ {run_dir.relative_to(REPO)}/report.md")
    print(f"PASS {passed}/{len(ok)}  mean {mean:.2f}/5")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
