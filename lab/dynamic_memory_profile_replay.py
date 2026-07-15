#!/usr/bin/env python3
"""Chronological replay for dynamic memory profiles.

This is the next lab step after the directive replay exposed overfitting:

    past rows only
      -> build data-derived term profiles
      -> current transcript retrieves by one generic scorer
      -> optional prompt directives
      -> current row updates positive/negative evidence

The key constraint: no target-specific rescue gates. A profile may contain
target-specific data because the user produced that evidence, but the scoring
formula must be generic for every term.
"""

from __future__ import annotations

import argparse
import difflib
import json
import math
import re
import sys
import time
from collections import Counter
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LAB = Path(__file__).resolve().parent
if str(LAB) not in sys.path:
    sys.path.insert(0, str(LAB))

import learning_loop
import memory_candidate_judge as judge
import memory_policy_replay as policy
import model_backed_learning_replay as replay
import polish_lab
from model_catalog import LAB_MODEL_CATALOG, available_lab_routes

RUNS_DIR = LAB / "corpus" / "dynamic_memory_runs"

CONTEXT_STOPWORDS = learning_loop.COMMON_SOURCE_WORDS | {
    "also",
    "can",
    "check",
    "currently",
    "did",
    "does",
    "done",
    "for",
    "from",
    "get",
    "give",
    "have",
    "how",
    "if",
    "into",
    "just",
    "kar",
    "karna",
    "karo",
    "kya",
    "like",
    "make",
    "matlab",
    "now",
    "only",
    "please",
    "right",
    "so",
    "tell",
    "there",
    "use",
    "using",
    "want",
    "what",
    "when",
    "where",
    "with",
}


@dataclass
class AliasProfile:
    source_norm: str
    target_norm: str
    canonical_target: str
    source_forms: Counter[str] = field(default_factory=Counter)
    positive_context: Counter[str] = field(default_factory=Counter)
    negative_context: Counter[str] = field(default_factory=Counter)
    content_keys: set[str] = field(default_factory=set)
    accounts: set[str] = field(default_factory=set)
    positive_examples: list[str] = field(default_factory=list)
    negative_examples: list[str] = field(default_factory=list)
    high_signal_seen: bool = False
    max_source_target_surface: float = 0.0
    max_source_target_phonetic: float = 0.0

    @property
    def evidence_count(self) -> int:
        return len(self.content_keys)

    @property
    def account_count(self) -> int:
        return len(self.accounts)

    @property
    def source(self) -> str:
        if self.source_forms:
            return self.source_forms.most_common(1)[0][0]
        return self.source_norm

    @property
    def is_acronym_source(self) -> bool:
        return any(replay.learned_source_acronym_like(form) for form in self.source_forms) or replay.learned_source_acronym_like(self.source)

    @property
    def requires_case_signal(self) -> bool:
        compact_sources = [replay.compact(form) for form in self.source_forms] or [replay.compact(self.source_norm)]
        max_len = max((len(value) for value in compact_sources), default=0)
        return self.is_acronym_source and max_len <= 3

    def top_positive_context(self, limit: int = 12) -> list[str]:
        return [word for word, _ in self.positive_context.most_common(limit)]

    def top_negative_context(self, limit: int = 12) -> list[str]:
        return [word for word, _ in self.negative_context.most_common(limit)]


def context_words(text: str | None, *, exclude: str | None = None, limit: int = 80) -> Counter[str]:
    words = learning_loop.alias_norm(text).split()
    exclude_words = set(learning_loop.alias_norm(exclude).split()) if exclude else set()
    out: Counter[str] = Counter()
    for word in words:
        if word in exclude_words:
            continue
        if word in CONTEXT_STOPWORDS:
            continue
        if len(word) <= 2 and not any(ch.isdigit() for ch in word):
            continue
        if len(word) > 32:
            continue
        out[word] += 1
        if sum(out.values()) >= limit:
            break
    return out


def profile_from_candidate(row: dict[str, Any], candidate: learning_loop.Candidate) -> tuple[str, str, str] | None:
    target = replay.canonical_target(candidate.target)
    target_norm = replay.canonical_target_norm(target)
    if not target_norm or not candidate.source_norm:
        return None
    if not judge.target_is_directive_domain(target):
        return None
    if judge.source_is_common_or_dangerous(candidate.source_norm, target_norm):
        return None
    return candidate.source_norm, target_norm, target


def occurrence_key(row: dict[str, Any]) -> str:
    return judge.row_content_key(row)


class DynamicMemory:
    def __init__(self) -> None:
        self.profiles: dict[tuple[str, str], AliasProfile] = {}
        self.term_positive_context: dict[str, Counter[str]] = {}
        self.term_negative_context: dict[str, Counter[str]] = {}

    def observe_positive(self, row: dict[str, Any]) -> None:
        if not learning_loop.should_learn_from_row(row):
            return
        raw = row.get("raw_stt") or row.get("transcript") or ""
        content_key = occurrence_key(row)
        for candidate in learning_loop.extract_candidates(row):
            key = profile_from_candidate(row, candidate)
            if key is None:
                continue
            source_norm, target_norm, target = key
            profile = self.profiles.get((source_norm, target_norm))
            if profile is None:
                profile = AliasProfile(
                    source_norm=source_norm,
                    target_norm=target_norm,
                    canonical_target=target,
                )
                self.profiles[(source_norm, target_norm)] = profile
            profile.source_forms[candidate.source] += 1
            profile.content_keys.add(content_key)
            if row.get("account_hash"):
                profile.accounts.add(str(row["account_hash"]))
            profile.positive_context.update(context_words(raw, exclude=candidate.source))
            self.term_positive_context.setdefault(target_norm, Counter()).update(context_words(raw, exclude=candidate.source))
            profile.high_signal_seen = profile.high_signal_seen or judge.source_is_high_signal(candidate.source)
            profile.max_source_target_surface = max(
                profile.max_source_target_surface,
                judge.surface_similarity(candidate.source, target),
            )
            profile.max_source_target_phonetic = max(
                profile.max_source_target_phonetic,
                replay.phonetic_similarity(candidate.source, target),
            )
            if len(profile.positive_examples) < 3:
                profile.positive_examples.append(str(row.get("sample_id") or ""))

    def observe_negative(self, row: dict[str, Any], directives: list[dict[str, Any]], golds: list[dict[str, Any]]) -> None:
        raw = row.get("raw_stt") or row.get("transcript") or ""
        for directive in directives:
            if policy.directive_hits_gold(directive, golds):
                continue
            key = (directive["learned_source_norm"], directive["target_norm"])
            profile = self.profiles.get(key)
            if not profile:
                continue
            profile.negative_context.update(context_words(raw, exclude=directive["source"]))
            self.term_negative_context.setdefault(directive["target_norm"], Counter()).update(
                context_words(raw, exclude=directive["source"])
            )
            if len(profile.negative_examples) < 5:
                profile.negative_examples.append(str(row.get("sample_id") or ""))

    def eligible_profiles(self) -> list[AliasProfile]:
        return [profile for profile in self.profiles.values() if profile.evidence_count >= 2 and profile.high_signal_seen]


def phrase_windows_with_offsets(words: list[str], source_len: int) -> list[tuple[str, int, int]]:
    windows: list[tuple[str, int, int]] = []
    min_width = max(1, source_len - 1)
    max_width = min(source_len + 2, 5)
    for width in range(min_width, max_width + 1):
        for start in range(0, max(0, len(words) - width) + 1):
            windows.append((" ".join(words[start : start + width]), start, start + width))
    return windows


def local_context(words: list[str], start: int, end: int, *, radius: int = 8) -> Counter[str]:
    left = max(0, start - radius)
    right = min(len(words), end + radius)
    return context_words(" ".join(words[left:start] + words[end:right]))


def overlap_score(profile_words: Counter[str], ctx: Counter[str], *, limit: int = 12) -> tuple[float, list[str]]:
    if not profile_words or not ctx:
        return 0.0, []
    top = [word for word, _ in profile_words.most_common(limit)]
    hits = [word for word in top if word in ctx]
    if not hits:
        return 0.0, []
    weighted = sum(profile_words[word] for word in hits)
    denom = max(sum(profile_words[word] for word in top), 1)
    return weighted / denom, hits


def combined_context(profile: AliasProfile, term_context: Counter[str]) -> Counter[str]:
    out: Counter[str] = Counter()
    out.update(profile.positive_context)
    for word, count in term_context.items():
        out[word] += max(1, math.ceil(count * 0.6))
    return out


def match_score(profile: AliasProfile, chunk: str, transcript: str) -> tuple[float, str]:
    chunk_norm = learning_loop.alias_norm(chunk)
    if not chunk_norm:
        return 0.0, "empty"
    if chunk_norm == profile.target_norm:
        return 0.0, "target-already-present"
    if chunk_norm == profile.source_norm:
        return 1.0, "exact-source"

    source_compact = replay.compact(profile.source_norm)
    chunk_compact = replay.compact(chunk)
    char_sim = difflib.SequenceMatcher(None, profile.source_norm, chunk_norm).ratio()
    compact_sim = difflib.SequenceMatcher(None, source_compact, chunk_compact).ratio() if source_compact else 0.0
    phon = replay.phonetic_similarity(profile.source_norm, chunk_norm)
    surface = max(char_sim, compact_sim)
    common_window = replay.is_tiny_common_window(chunk_norm)
    chunk_words = chunk_norm.split()
    weak_target_span_ok = len(chunk_words) == 1 and not any(word in CONTEXT_STOPWORDS for word in chunk_words)
    target_surface = max(
        difflib.SequenceMatcher(None, profile.target_norm, chunk_norm).ratio(),
        difflib.SequenceMatcher(None, replay.compact(profile.target_norm), chunk_compact).ratio()
        if chunk_compact
        else 0.0,
    )
    target_compact = replay.compact(profile.target_norm)
    weak_target_length_ok = bool(
        chunk_compact
        and target_compact
        and len(chunk_compact) >= math.floor(len(target_compact) * 0.75)
        and len(chunk_compact) <= math.ceil(len(target_compact) * 1.5)
    )

    if profile.requires_case_signal:
        has_case_signal = replay.chunk_has_case_signal(transcript, chunk)
        if not common_window and target_surface >= 0.78:
            return 0.78 + min(0.08, (target_surface - 0.78) * 0.4), f"target-surface:{chunk}"
        if not common_window and weak_target_span_ok and weak_target_length_ok and target_surface >= 0.65:
            return 0.70 + min(0.08, (target_surface - 0.65) * 0.35), f"target-surface-weak:{chunk}"
        if not has_case_signal:
            return 0.0, "acronym-no-case-signal"
        if surface >= 0.66:
            return 0.76 + min(0.10, (surface - 0.66) * 0.45), f"acronym-surface:{chunk}"
        return 0.0, "acronym-weak-surface"

    if surface >= 0.82 and not common_window:
        return 0.84 + min(0.10, (surface - 0.82) * 0.5), f"source-surface:{chunk}"
    if phon >= 0.82 and surface >= 0.48 and not common_window:
        return 0.80 + min(0.10, (phon - 0.82) * 0.45), f"source-phonetic:{chunk}"
    if target_surface >= 0.80 and not common_window:
        return 0.78 + min(0.08, (target_surface - 0.80) * 0.4), f"target-surface:{chunk}"
    if target_surface >= 0.65 and weak_target_span_ok and weak_target_length_ok and not common_window:
        return 0.70 + min(0.08, (target_surface - 0.65) * 0.35), f"target-surface-weak:{chunk}"
    return max(surface, phon * 0.85), f"below-threshold:{chunk}"


def profile_confidence(profile: AliasProfile) -> float:
    score = 0.0
    score += min(0.24, profile.evidence_count * 0.06)
    score += min(0.12, profile.account_count * 0.04)
    score += 0.12 if profile.high_signal_seen else 0.0
    score += 0.10 if profile.max_source_target_surface >= 0.78 else 0.0
    score += 0.08 if profile.max_source_target_phonetic >= 0.72 else 0.0
    return min(score, 0.50)


def retrieve_directives(memory: DynamicMemory, transcript: str, *, limit: int) -> list[dict[str, Any]]:
    raw_norm = learning_loop.alias_norm(transcript)
    words = raw_norm.split()
    scored: list[tuple[float, dict[str, Any]]] = []
    global_ctx = context_words(transcript)

    for profile in memory.eligible_profiles():
        source_len = max(1, len(profile.source_norm.split()))
        best: tuple[float, dict[str, Any]] | None = None
        for chunk, start, end in phrase_windows_with_offsets(words, source_len):
            source_score, match_reason = match_score(profile, chunk, transcript)
            min_match_score = 0.65 if match_reason.startswith("target-surface-weak:") else 0.72
            if source_score < min_match_score:
                continue
            if learning_loop.alias_norm(chunk) == profile.target_norm:
                continue
            positive_profile = combined_context(profile, memory.term_positive_context.get(profile.target_norm, Counter()))
            negative_profile = combined_context(profile, memory.term_negative_context.get(profile.target_norm, Counter()))
            nearby_ctx = global_ctx + local_context(words, start, end)
            pos_score, pos_hits = overlap_score(positive_profile, nearby_ctx)
            neg_score, neg_hits = overlap_score(negative_profile, nearby_ctx)

            # Exact non-acronym aliases are allowed with little context. Fuzzy,
            # phonetic, and acronym matches need learned context support.
            exact = match_reason == "exact-source"
            close_source = match_reason.startswith("source-surface:") and source_score >= 0.87
            needs_context = (not exact and not close_source) or profile.requires_case_signal or match_reason.startswith("target-surface:")
            if needs_context and pos_score <= 0.0:
                continue
            score = source_score
            score += profile_confidence(profile)
            score += min(0.18, pos_score * 0.22)
            score -= min(0.24, neg_score * 0.30)
            if profile.requires_case_signal and not replay.chunk_has_case_signal(transcript, chunk):
                score -= 0.30

            threshold = 0.95 if needs_context else 0.96
            if score < threshold:
                continue

            directive = {
                "source": chunk,
                "target": profile.canonical_target,
                "learned_source": profile.source,
                "learned_source_norm": profile.source_norm,
                "source_norm": learning_loop.alias_norm(chunk),
                "target_norm": profile.target_norm,
                "memory_count": profile.evidence_count,
                "memory_accounts": profile.account_count,
                "score": round(score, 4),
                "match_score": round(source_score, 4),
                "match_reason": match_reason,
                "positive_context_hits": pos_hits[:8],
                "negative_context_hits": neg_hits[:8],
                "profile_positive_context": profile.top_positive_context(),
                "profile_negative_context": profile.top_negative_context(),
            }
            if best is None or score > best[0]:
                best = (score, directive)
        if best:
            scored.append(best)

    by_target: dict[str, tuple[float, dict[str, Any]]] = {}
    for score, directive in scored:
        current = by_target.get(directive["target_norm"])
        if current is None or score > current[0]:
            by_target[directive["target_norm"]] = (score, directive)
    return [
        directive
        for _, directive in sorted(
            by_target.values(),
            key=lambda item: (-item[0], -item[1]["memory_count"], item[1]["target_norm"]),
        )[:limit]
    ]


def build_dynamic_user_message(transcript: str, directives: list[dict[str, Any]]) -> str:
    lines = [
        "You are a TRANSCRIPTION CLEANER, not a conversational AI.",
        "You ONLY clean the spoken transcript into the intended final text.",
        "",
        "DYNAMIC USER MEMORY RULES FOR THIS TRANSCRIPT:",
    ]
    if directives:
        for d in directives:
            pos = ", ".join(d["positive_context_hits"]) or "direct alias evidence"
            neg = ", ".join(d["negative_context_hits"]) or "none"
            lines.append(
                f"- If the current transcript phrase \"{d['source']}\" is being used in this context, "
                f"write it as \"{d['target']}\". Evidence: learned alias \"{d['learned_source']}\"; "
                f"matching context={pos}; negative-context overlap={neg}; confidence={d['score']:.2f}."
            )
    else:
        lines.append("- none")
    lines.extend(
        [
            "",
            "RULES:",
            "- Apply only the rules that match the current transcript phrase and context.",
            "- Do not rewrite unrelated ordinary words.",
            "- Preserve the speaker's language mix and tone.",
            "- Output only the cleaned result.",
            "",
            "=== BEGIN TRANSCRIPT ===",
            transcript,
            "=== END TRANSCRIPT ===",
        ]
    )
    return "\n".join(lines)


def resolve_route(slug: str) -> dict[str, Any]:
    polish_lab.load_dotenv()
    routes = available_lab_routes(LAB_MODEL_CATALOG, slugs={slug})
    if not routes:
        raise SystemExit(f"No route for slug {slug}. Check API key in .env.")
    return routes[0]


def run_replay(
    *,
    rows: list[dict[str, Any]],
    warmup: int,
    eval_limit: int | None,
    directive_limit: int,
    run_model: bool,
    route: dict[str, Any] | None,
    variant: str,
) -> dict[str, Any]:
    memory = DynamicMemory()
    results: list[dict[str, Any]] = []
    evaluated = 0

    for idx, row in enumerate(rows):
        raw = row.get("raw_stt") or row.get("transcript") or ""
        if idx >= warmup and replay.row_is_useful_eval(row):
            directives = retrieve_directives(memory, raw, limit=directive_limit)
            golds = policy.gold_directive_targets(row)
            safe_targets = {profile.target_norm for profile in memory.eligible_profiles()}
            learnable_golds = [g for g in golds if g["target_norm"] in safe_targets]
            first_seen_golds = [g for g in golds if g["target_norm"] not in safe_targets]
            wrong = [d for d in directives if policy.directive_supported_by_row(d, golds, row) is None]
            missed_learnable = [g for g in learnable_golds if not policy.gold_hit_by_directives(g, directives)]
            missed_all = [g for g in golds if not policy.gold_hit_by_directives(g, directives)]

            output = ""
            model_ok = None
            model_error = None
            latency_s = None
            target_hits = 0
            if run_model and route is not None and directives:
                prompt = replay.build_prompt(variant, replay.ReplayMemory(), raw)
                user_message = build_dynamic_user_message(raw, directives)
                start = time.perf_counter()
                res = polish_lab.polish_try(raw, prompt, route, user_message=user_message)
                latency_s = time.perf_counter() - start
                model_ok = bool(res.get("ok"))
                model_error = res.get("error")
                output = str(res.get("polished") or "")
                out_norm = learning_loop.alias_norm(output)
                target_hits = sum(1 for d in directives if d["target_norm"] in out_norm)

            results.append(
                {
                    "idx": idx,
                    "sample_id": row.get("sample_id"),
                    "source": row.get("source"),
                    "raw_stt": raw,
                    "old_polished": row.get("polished_output") or "",
                    "user_kept": row.get("user_kept") or "",
                    "directives": directives,
                    "gold_directives": golds,
                    "learnable_gold_directives": learnable_golds,
                    "first_seen_gold_directives": first_seen_golds,
                    "wrong_directives": wrong,
                    "missed_learnable_gold": missed_learnable,
                    "missed_gold": missed_all,
                    "eligible_profile_count": len(memory.eligible_profiles()),
                    "model_output": output,
                    "model_ok": model_ok,
                    "model_error": model_error,
                    "model_latency_s": latency_s,
                    "model_directive_target_hits": target_hits,
                }
            )
            memory.observe_negative(row, directives, golds)
            evaluated += 1
            if eval_limit and evaluated >= eval_limit:
                break

        memory.observe_positive(row)

    return summarize(results, rows_seen=min(len(rows), warmup + evaluated))


def summarize(results: list[dict[str, Any]], *, rows_seen: int) -> dict[str, Any]:
    directive_count = sum(len(r["directives"]) for r in results)
    wrong_count = sum(len(r["wrong_directives"]) for r in results)
    gold_count = sum(len(r["gold_directives"]) for r in results)
    learnable_gold_count = sum(len(r["learnable_gold_directives"]) for r in results)
    first_seen_gold_count = sum(len(r["first_seen_gold_directives"]) for r in results)
    missed_count = sum(len(r["missed_gold"]) for r in results)
    missed_learnable_count = sum(len(r["missed_learnable_gold"]) for r in results)
    model_directive_count = sum(len(r["directives"]) for r in results if r["model_ok"])
    model_hits = sum(r["model_directive_target_hits"] for r in results if r["model_ok"])
    return {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "rows_seen": rows_seen,
        "eval_rows": len(results),
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
        "model_directive_count": model_directive_count,
        "model_directive_target_hits": model_hits,
        "model_directive_target_hit_rate": model_hits / max(model_directive_count, 1),
        "results": results,
    }


def write_report(summary: dict[str, Any], corpus: Path) -> tuple[Path, Path]:
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    json_path = RUNS_DIR / f"dynamic_memory_profile_replay_{stamp}.json"
    md_path = RUNS_DIR / f"dynamic_memory_profile_replay_{stamp}.md"
    json_path.write_text(json.dumps({**summary, "corpus": str(corpus)}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    lines = [
        "# Dynamic Memory Profile Replay",
        "",
        f"- Corpus: `{corpus}`",
        f"- Eval rows: {summary['eval_rows']}",
        f"- Rows with directives: {summary['rows_with_directives']}",
        f"- Directives emitted: {summary['directive_count']}",
        f"- Wrong directives: {summary['wrong_directive_count']} ({summary['wrong_directive_rate']:.1%})",
        f"- Learnable gold recall: {summary['learnable_gold_recall']:.1%} ({summary['learnable_gold_directive_count'] - summary['missed_learnable_gold_count']} / {summary['learnable_gold_directive_count']})",
        f"- Overall gold recall: {summary['gold_recall']:.1%} ({summary['gold_directive_count'] - summary['missed_gold_count']} / {summary['gold_directive_count']})",
        f"- First-seen gold directives: {summary['first_seen_gold_directive_count']}",
        f"- Model directive target hit rate: {summary['model_directive_target_hits']} / {summary['model_directive_count']} ({summary['model_directive_target_hit_rate']:.1%})",
        "",
        "## Wrong Directives",
        "",
    ]
    wrong_cases = [r for r in summary["results"] if r["wrong_directives"]]
    if not wrong_cases:
        lines.append("None.")
    for r in wrong_cases[:40]:
        lines.extend(
            [
                f"### {r['sample_id']}",
                "",
                f"- Wrong: {[(d['source'], d['target'], d['match_reason'], d['positive_context_hits'], d['negative_context_hits']) for d in r['wrong_directives']]}",
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

    lines.extend(["", "## Missed Learnable Gold", ""])
    missed_cases = [r for r in summary["results"] if r["missed_learnable_gold"]]
    if not missed_cases:
        lines.append("None.")
    for r in missed_cases[:40]:
        lines.extend(
            [
                f"### {r['sample_id']}",
                "",
                f"- Missed learnable: {[(g['source'], g['target']) for g in r['missed_learnable_gold']]}",
                f"- Emitted: {[(d['source'], d['target'], d['match_reason']) for d in r['directives']]}",
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

    lines.extend(["", "## Emitted Directive Examples", ""])
    emitted_cases = [r for r in summary["results"] if r["directives"]]
    if not emitted_cases:
        lines.append("None.")
    for r in emitted_cases[:30]:
        lines.extend(
            [
                f"### {r['sample_id']}",
                "",
                f"- Directives: {[(d['source'], d['target'], d['match_reason'], d['positive_context_hits'], round(d['score'], 3)) for d in r['directives']]}",
                f"- Gold: {[(g['source'], g['target']) for g in r['gold_directives']]}",
                "",
                "**Raw STT**",
                "",
                r["raw_stt"][:800],
                "",
            ]
        )

    md_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return json_path, md_path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=None)
    parser.add_argument("--row-limit", type=int)
    parser.add_argument("--limit", type=int, help="Max evaluation rows.")
    parser.add_argument("--warmup", type=int, default=25)
    parser.add_argument("--directive-limit", type=int, default=8)
    parser.add_argument("--run-model", action="store_true")
    parser.add_argument("--slug", default="openrouter-gemma-4-nitro")
    parser.add_argument("--variant", default="intent_v4")
    args = parser.parse_args()

    corpus = args.corpus or judge.latest_corpus()
    rows = judge.load_rows(corpus, args.row_limit)
    route = resolve_route(args.slug) if args.run_model else None
    summary = run_replay(
        rows=rows,
        warmup=args.warmup,
        eval_limit=args.limit,
        directive_limit=args.directive_limit,
        run_model=args.run_model,
        route=route,
        variant=args.variant,
    )
    json_path, md_path = write_report(summary, corpus)
    print(
        json.dumps(
            {
                "corpus": str(corpus),
                "eval_rows": summary["eval_rows"],
                "rows_with_directives": summary["rows_with_directives"],
                "directive_count": summary["directive_count"],
                "wrong_directive_count": summary["wrong_directive_count"],
                "wrong_directive_rate": round(summary["wrong_directive_rate"], 4),
                "learnable_gold_directive_count": summary["learnable_gold_directive_count"],
                "learnable_gold_recall": round(summary["learnable_gold_recall"], 4),
                "model_directive_target_hit_rate": round(summary["model_directive_target_hit_rate"], 4),
                "json": str(json_path),
                "report": str(md_path),
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
