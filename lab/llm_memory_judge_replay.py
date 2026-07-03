#!/usr/bin/env python3
"""Chronological replay with a second-stage LLM memory judge.

This tests whether weak dynamic-memory candidates can be safely recovered by a
judge before they become polish directives.

Flow:

    past rows -> dynamic memory profiles
    current transcript -> broad weak candidate proposals
    LLM judge -> emit / soft_hint / reject
    emitted rules -> evaluated as directives

The scorer still uses generic candidate generation. The model judge receives
data-derived profile evidence and decides whether a candidate applies in the
current transcript.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LAB = Path(__file__).resolve().parent
if str(LAB) not in sys.path:
    sys.path.insert(0, str(LAB))

import dynamic_memory_profile_replay as dyn
import learning_loop
import memory_candidate_judge as judge
import memory_policy_replay as policy
import model_backed_learning_replay as replay
import polish_lab

RUNS_DIR = LAB / "corpus" / "llm_memory_judge_runs"


def proposal_context(
    memory: dyn.DynamicMemory,
    profile: dyn.AliasProfile,
    transcript: str,
    chunk: str,
    start: int,
    end: int,
    source_score: float,
    match_reason: str,
) -> dict[str, Any]:
    words = learning_loop.alias_norm(transcript).split()
    global_ctx = dyn.context_words(transcript)
    nearby_ctx = global_ctx + dyn.local_context(words, start, end)
    positive_profile = dyn.combined_context(profile, memory.term_positive_context.get(profile.target_norm, Counter()))
    negative_profile = dyn.combined_context(profile, memory.term_negative_context.get(profile.target_norm, Counter()))
    pos_score, pos_hits = dyn.overlap_score(positive_profile, nearby_ctx)
    neg_score, neg_hits = dyn.overlap_score(negative_profile, nearby_ctx)
    preliminary = (
        source_score
        + dyn.profile_confidence(profile)
        + min(0.18, pos_score * 0.22)
        - min(0.24, neg_score * 0.30)
    )
    return {
        "source": chunk,
        "target": profile.canonical_target,
        "learned_source": profile.source,
        "learned_source_norm": profile.source_norm,
        "source_norm": learning_loop.alias_norm(chunk),
        "target_norm": profile.target_norm,
        "memory_count": profile.evidence_count,
        "memory_accounts": profile.account_count,
        "preliminary_score": round(preliminary, 4),
        "match_score": round(source_score, 4),
        "match_reason": match_reason,
        "positive_context_score": round(pos_score, 4),
        "negative_context_score": round(neg_score, 4),
        "positive_context_hits": pos_hits[:10],
        "negative_context_hits": neg_hits[:10],
        "profile_positive_context": profile.top_positive_context(16),
        "profile_negative_context": profile.top_negative_context(16),
        "term_positive_context": [word for word, _ in memory.term_positive_context.get(profile.target_norm, Counter()).most_common(16)],
        "term_negative_context": [word for word, _ in memory.term_negative_context.get(profile.target_norm, Counter()).most_common(16)],
    }


def generate_proposals(
    memory: dyn.DynamicMemory,
    transcript: str,
    *,
    limit: int,
    min_match_score: float,
    include_below_threshold: bool,
) -> list[dict[str, Any]]:
    raw_norm = learning_loop.alias_norm(transcript)
    words = raw_norm.split()
    proposals: list[dict[str, Any]] = []

    for profile in memory.eligible_profiles():
        source_len = max(1, len(profile.source_norm.split()))
        best: dict[str, Any] | None = None
        for chunk, start, end in dyn.phrase_windows_with_offsets(words, source_len):
            source_score, match_reason = dyn.match_score(profile, chunk, transcript)
            if source_score < min_match_score:
                continue
            if match_reason.startswith("below-threshold:") and not include_below_threshold:
                continue
            if learning_loop.alias_norm(chunk) == profile.target_norm:
                continue
            candidate = proposal_context(
                memory,
                profile,
                transcript,
                chunk,
                start,
                end,
                source_score,
                match_reason,
            )
            if best is None or candidate["preliminary_score"] > best["preliminary_score"]:
                best = candidate
        if best is not None:
            proposals.append(best)

    by_target: dict[str, dict[str, Any]] = {}
    for proposal in proposals:
        existing = by_target.get(proposal["target_norm"])
        if existing is None or proposal["preliminary_score"] > existing["preliminary_score"]:
            by_target[proposal["target_norm"]] = proposal
    return sorted(
        by_target.values(),
        key=lambda item: (-item["preliminary_score"], -item["memory_count"], item["target_norm"]),
    )[:limit]


JUDGE_SYSTEM_PROMPT = """You judge whether a learned dictation memory rule applies to the current transcript.

Return ONLY strict JSON:
{"decision":"emit|soft_hint|reject","confidence":0.0,"reason":"short reason"}

Definitions:
- emit: the speaker likely intended the target term in this exact transcript. It is safe to give this as a repair directive.
- soft_hint: plausible, but not safe enough as a directive.
- reject: likely wrong, ordinary word, already correct, unsupported context, or too speculative.

Be conservative. Wrong directives are worse than missed corrections.
Never infer a replacement from technical/domain context alone. The current phrase must be a close spelling/phonetic/speech-recognition match to either the learned alias or the target term.
If match reason says below-threshold, reject unless the evidence is overwhelmingly explicit.
Common words, Hindi filler words, ordinary dev words, and plausible different corrections must be rejected.
Do not answer or clean the transcript. Only judge the candidate rule.
"""


def build_judge_user_message(transcript: str, proposal: dict[str, Any]) -> str:
    return "\n".join(
        [
            "CURRENT TRANSCRIPT:",
            transcript,
            "",
            "CANDIDATE MEMORY RULE:",
            f'Replace phrase "{proposal["source"]}" with "{proposal["target"]}".',
            "",
            "LEARNED MEMORY EVIDENCE:",
            f'- learned alias: "{proposal["learned_source"]}" -> "{proposal["target"]}"',
            f"- unique evidence count: {proposal['memory_count']}",
            f"- account count: {proposal['memory_accounts']}",
            f"- match reason/score: {proposal['match_reason']} / {proposal['match_score']}",
            f"- preliminary score: {proposal['preliminary_score']}",
            f"- current positive context hits: {proposal['positive_context_hits']}",
            f"- current negative context hits: {proposal['negative_context_hits']}",
            f"- profile positive context: {proposal['profile_positive_context']}",
            f"- profile negative context: {proposal['profile_negative_context']}",
            f"- term positive context: {proposal['term_positive_context']}",
            f"- term negative context: {proposal['term_negative_context']}",
            "",
            "Question: should this candidate be emitted as a repair directive for this transcript?",
        ]
    )


def parse_judge_json(text: str) -> dict[str, Any]:
    text = text.strip()
    if text.startswith("```"):
        text = re.sub(r"^```(?:json)?", "", text).strip()
        text = re.sub(r"```$", "", text).strip()
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        match = re.search(r"\{.*\}", text, flags=re.S)
        if not match:
            return {"decision": "reject", "confidence": 0.0, "reason": f"non_json:{text[:120]}"}
        try:
            data = json.loads(match.group(0))
        except json.JSONDecodeError:
            return {"decision": "reject", "confidence": 0.0, "reason": f"bad_json:{text[:120]}"}
    decision = str(data.get("decision", "reject")).strip().lower()
    if decision not in {"emit", "soft_hint", "reject"}:
        decision = "reject"
    try:
        confidence = float(data.get("confidence", 0.0))
    except (TypeError, ValueError):
        confidence = 0.0
    return {
        "decision": decision,
        "confidence": max(0.0, min(confidence, 1.0)),
        "reason": str(data.get("reason", ""))[:300],
    }


def judge_proposal(
    *,
    route: dict[str, Any],
    transcript: str,
    proposal: dict[str, Any],
) -> tuple[dict[str, Any], float, str]:
    user_message = build_judge_user_message(transcript, proposal)
    start = time.perf_counter()
    res = polish_lab.polish_try(
        transcript,
        JUDGE_SYSTEM_PROMPT,
        route,
        user_message=user_message,
    )
    elapsed = time.perf_counter() - start
    if not res.get("ok"):
        return {"decision": "reject", "confidence": 0.0, "reason": f"judge_error:{res.get('error')}"}, elapsed, ""
    raw = str(res.get("polished") or "")
    return parse_judge_json(raw), elapsed, raw


def proposal_to_directive(proposal: dict[str, Any], judge_result: dict[str, Any]) -> dict[str, Any]:
    directive = dict(proposal)
    directive["score"] = proposal["preliminary_score"]
    directive["judge_decision"] = judge_result["decision"]
    directive["judge_confidence"] = judge_result["confidence"]
    directive["judge_reason"] = judge_result["reason"]
    return directive


def resolve_route(slug: str) -> dict[str, Any]:
    return dyn.resolve_route(slug)


def run_replay(
    *,
    rows: list[dict[str, Any]],
    warmup: int,
    eval_limit: int | None,
    proposal_limit: int,
    min_match_score: float,
    include_below_threshold: bool,
    route: dict[str, Any],
    max_judge_calls: int,
    emit_confidence: float,
) -> dict[str, Any]:
    memory = dyn.DynamicMemory()
    results: list[dict[str, Any]] = []
    evaluated = 0
    judge_calls = 0
    judge_latency = 0.0

    for idx, row in enumerate(rows):
        raw = row.get("raw_stt") or row.get("transcript") or ""
        if idx >= warmup and replay.row_is_useful_eval(row):
            proposals = generate_proposals(
                memory,
                raw,
                limit=proposal_limit,
                min_match_score=min_match_score,
                include_below_threshold=include_below_threshold,
            )
            judged: list[dict[str, Any]] = []
            directives: list[dict[str, Any]] = []
            for proposal in proposals:
                if judge_calls >= max_judge_calls:
                    break
                judge_result, latency, raw_response = judge_proposal(
                    route=route,
                    transcript=raw,
                    proposal=proposal,
                )
                judge_calls += 1
                judge_latency += latency
                judged_proposal = dict(proposal)
                judged_proposal["judge"] = judge_result
                judged_proposal["judge_latency_s"] = latency
                judged_proposal["judge_raw_response"] = raw_response[:500]
                judged.append(judged_proposal)
                if judge_result["decision"] == "emit" and judge_result["confidence"] >= emit_confidence:
                    directives.append(proposal_to_directive(proposal, judge_result))

            golds = policy.gold_directive_targets(row)
            safe_targets = {profile.target_norm for profile in memory.eligible_profiles()}
            learnable_golds = [g for g in golds if g["target_norm"] in safe_targets]
            first_seen_golds = [g for g in golds if g["target_norm"] not in safe_targets]
            wrong = [d for d in directives if policy.directive_supported_by_row(d, golds, row) is None]
            missed_learnable = [g for g in learnable_golds if not policy.gold_hit_by_directives(g, directives)]
            missed_all = [g for g in golds if not policy.gold_hit_by_directives(g, directives)]

            results.append(
                {
                    "idx": idx,
                    "sample_id": row.get("sample_id"),
                    "source": row.get("source"),
                    "raw_stt": raw,
                    "old_polished": row.get("polished_output") or "",
                    "user_kept": row.get("user_kept") or "",
                    "proposals": proposals,
                    "judged_proposals": judged,
                    "directives": directives,
                    "gold_directives": golds,
                    "learnable_gold_directives": learnable_golds,
                    "first_seen_gold_directives": first_seen_golds,
                    "wrong_directives": wrong,
                    "missed_learnable_gold": missed_learnable,
                    "missed_gold": missed_all,
                    "eligible_profile_count": len(memory.eligible_profiles()),
                }
            )
            memory.observe_negative(row, directives, golds)
            evaluated += 1
            if eval_limit and evaluated >= eval_limit:
                break
        memory.observe_positive(row)

    summary = summarize(results)
    summary["judge_calls"] = judge_calls
    summary["judge_latency_total_s"] = judge_latency
    summary["judge_latency_avg_s"] = judge_latency / max(judge_calls, 1)
    return summary


def summarize(results: list[dict[str, Any]]) -> dict[str, Any]:
    directive_count = sum(len(r["directives"]) for r in results)
    wrong_count = sum(len(r["wrong_directives"]) for r in results)
    gold_count = sum(len(r["gold_directives"]) for r in results)
    learnable_gold_count = sum(len(r["learnable_gold_directives"]) for r in results)
    first_seen_gold_count = sum(len(r["first_seen_gold_directives"]) for r in results)
    missed_count = sum(len(r["missed_gold"]) for r in results)
    missed_learnable_count = sum(len(r["missed_learnable_gold"]) for r in results)
    judged_count = sum(len(r["judged_proposals"]) for r in results)
    emit_count = directive_count
    soft_count = sum(1 for r in results for p in r["judged_proposals"] if p["judge"]["decision"] == "soft_hint")
    reject_count = sum(1 for r in results for p in r["judged_proposals"] if p["judge"]["decision"] == "reject")
    return {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "eval_rows": len(results),
        "rows_with_proposals": sum(1 for r in results if r["proposals"]),
        "proposal_count": sum(len(r["proposals"]) for r in results),
        "judged_count": judged_count,
        "emit_count": emit_count,
        "soft_hint_count": soft_count,
        "reject_count": reject_count,
        "rows_with_directives": sum(1 for r in results if r["directives"]),
        "directive_count": directive_count,
        "wrong_directive_count": wrong_count,
        "wrong_directive_rate": wrong_count / max(directive_count, 1),
        "gold_directive_count": gold_count,
        "learnable_gold_directive_count": learnable_gold_count,
        "first_seen_gold_directive_count": first_seen_gold_count,
        "missed_gold_count": missed_count,
        "missed_learnable_gold_count": missed_learnable_count,
        "gold_recall": (gold_count - missed_count) / max(gold_count, 1),
        "learnable_gold_recall": (learnable_gold_count - missed_learnable_count) / max(learnable_gold_count, 1),
        "results": results,
    }


def write_report(summary: dict[str, Any], corpus: Path) -> tuple[Path, Path]:
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    json_path = RUNS_DIR / f"llm_memory_judge_replay_{stamp}.json"
    md_path = RUNS_DIR / f"llm_memory_judge_replay_{stamp}.md"
    json_path.write_text(json.dumps({**summary, "corpus": str(corpus)}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    lines = [
        "# LLM Memory Judge Replay",
        "",
        f"- Corpus: `{corpus}`",
        f"- Eval rows: {summary['eval_rows']}",
        f"- Proposals/judged: {summary['proposal_count']} / {summary['judged_count']}",
        f"- Judge decisions emit/soft/reject: {summary['emit_count']} / {summary['soft_hint_count']} / {summary['reject_count']}",
        f"- Directives emitted: {summary['directive_count']}",
        f"- Wrong directives: {summary['wrong_directive_count']} ({summary['wrong_directive_rate']:.1%})",
        f"- Learnable gold recall: {summary['learnable_gold_recall']:.1%} ({summary['learnable_gold_directive_count'] - summary['missed_learnable_gold_count']} / {summary['learnable_gold_directive_count']})",
        f"- Overall gold recall: {summary['gold_recall']:.1%} ({summary['gold_directive_count'] - summary['missed_gold_count']} / {summary['gold_directive_count']})",
        f"- Judge calls: {summary['judge_calls']}, avg latency: {summary['judge_latency_avg_s']:.2f}s",
        "",
        "## Wrong Directives",
        "",
    ]
    wrong_cases = [r for r in summary["results"] if r["wrong_directives"]]
    if not wrong_cases:
        lines.append("None.")
    for r in wrong_cases[:30]:
        lines.extend(
            [
                f"### {r['sample_id']}",
                "",
                f"- Wrong: {[(d['source'], d['target'], d.get('judge_confidence'), d.get('judge_reason')) for d in r['wrong_directives']]}",
                f"- Gold: {[(g['source'], g['target']) for g in r['gold_directives']]}",
                "",
                "**Raw STT**",
                "",
                r["raw_stt"],
                "",
                "**User Kept**",
                "",
                r["user_kept"],
                "",
            ]
        )

    lines.extend(["", "## Emitted Directives", ""])
    emitted_cases = [r for r in summary["results"] if r["directives"]]
    if not emitted_cases:
        lines.append("None.")
    for r in emitted_cases[:30]:
        lines.extend(
            [
                f"### {r['sample_id']}",
                "",
                f"- Directives: {[(d['source'], d['target'], d.get('judge_confidence'), d.get('judge_reason')) for d in r['directives']]}",
                f"- Gold: {[(g['source'], g['target']) for g in r['gold_directives']]}",
                "",
                "**Raw STT**",
                "",
                r["raw_stt"][:900],
                "",
            ]
        )

    lines.extend(["", "## Missed Learnable Gold", ""])
    missed_cases = [r for r in summary["results"] if r["missed_learnable_gold"]]
    if not missed_cases:
        lines.append("None.")
    for r in missed_cases[:30]:
        lines.extend(
            [
                f"### {r['sample_id']}",
                "",
                f"- Missed: {[(g['source'], g['target']) for g in r['missed_learnable_gold']]}",
                f"- Judged: {[(p['source'], p['target'], p['judge']['decision'], p['judge']['confidence'], p['judge']['reason']) for p in r['judged_proposals']]}",
                "",
                "**Raw STT**",
                "",
                r["raw_stt"][:900],
                "",
            ]
        )

    lines.extend(["", "## Rejected Examples", ""])
    rejected = [p for r in summary["results"] for p in r["judged_proposals"] if p["judge"]["decision"] == "reject"]
    if not rejected:
        lines.append("None.")
    for p in rejected[:30]:
        lines.append(f"- `{p['source']}` -> `{p['target']}` conf={p['judge']['confidence']:.2f}: {p['judge']['reason']}")

    md_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return json_path, md_path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=None)
    parser.add_argument("--row-limit", type=int)
    parser.add_argument("--limit", type=int, help="Max evaluation rows.")
    parser.add_argument("--warmup", type=int, default=25)
    parser.add_argument("--proposal-limit", type=int, default=5)
    parser.add_argument("--min-match-score", type=float, default=0.65)
    parser.add_argument(
        "--include-below-threshold",
        action="store_true",
        help="Include weak below-threshold proposals for research. Off by default because they caused false directives.",
    )
    parser.add_argument("--max-judge-calls", type=int, default=60)
    parser.add_argument("--emit-confidence", type=float, default=0.90)
    parser.add_argument("--slug", default="cerebras-gpt-oss")
    args = parser.parse_args()

    corpus = args.corpus or judge.latest_corpus()
    rows = judge.load_rows(corpus, args.row_limit)
    route = resolve_route(args.slug)
    summary = run_replay(
        rows=rows,
        warmup=args.warmup,
        eval_limit=args.limit,
        proposal_limit=args.proposal_limit,
        min_match_score=args.min_match_score,
        include_below_threshold=args.include_below_threshold,
        route=route,
        max_judge_calls=args.max_judge_calls,
        emit_confidence=args.emit_confidence,
    )
    json_path, md_path = write_report(summary, corpus)
    print(
        json.dumps(
            {
                "corpus": str(corpus),
                "eval_rows": summary["eval_rows"],
                "proposal_count": summary["proposal_count"],
                "judged_count": summary["judged_count"],
                "emit_count": summary["emit_count"],
                "soft_hint_count": summary["soft_hint_count"],
                "reject_count": summary["reject_count"],
                "wrong_directive_count": summary["wrong_directive_count"],
                "wrong_directive_rate": round(summary["wrong_directive_rate"], 4),
                "learnable_gold_directive_count": summary["learnable_gold_directive_count"],
                "learnable_gold_recall": round(summary["learnable_gold_recall"], 4),
                "judge_calls": summary["judge_calls"],
                "judge_latency_avg_s": round(summary["judge_latency_avg_s"], 3),
                "json": str(json_path),
                "report": str(md_path),
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
