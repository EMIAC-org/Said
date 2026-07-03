#!/usr/bin/env python3
"""Judge whether observed edit candidates are safe to store as memory.

This is a lab-only storage-quality harness. It answers:

    raw STT / polished output -> user kept
    extracted candidate correction
    -> should this become a directive memory, a soft hint, wait for more
       evidence, or be rejected?

The goal is to prevent bad memory from entering the new directive pipeline.
The directive pipeline is powerful: when a repair is shown next to the transcript
as a user-message directive, the model usually obeys it. Therefore storage must
be stricter than retrieval.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import re
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LAB = Path(__file__).resolve().parent
if str(LAB) not in sys.path:
    sys.path.insert(0, str(LAB))

import learning_loop
import model_backed_learning_replay as replay

RUNS_DIR = LAB / "corpus" / "memory_judge_runs"


@dataclass
class Occurrence:
    sample_id: str
    content_key: str
    source: str
    target: str
    raw_stt: str
    old_polished: str
    user_kept: str
    source_label: str
    edit_bucket: str
    account_hash: str | None
    context_ok: bool
    surface_similarity: float
    phonetic_similarity: float


@dataclass
class Aggregate:
    source_norm: str
    target_norm: str
    canonical_target: str
    occurrences: list[Occurrence] = field(default_factory=list)

    @property
    def count(self) -> int:
        return len(self.occurrences)

    @property
    def evidence_occurrences(self) -> list[Occurrence]:
        by_content: dict[str, Occurrence] = {}
        for occurrence in self.occurrences:
            by_content.setdefault(occurrence.content_key, occurrence)
        return list(by_content.values())

    @property
    def evidence_count(self) -> int:
        return len(self.evidence_occurrences)

    @property
    def accounts(self) -> set[str]:
        return {o.account_hash for o in self.occurrences if o.account_hash}

    @property
    def evidence_accounts(self) -> set[str]:
        return {o.account_hash for o in self.evidence_occurrences if o.account_hash}

    @property
    def context_passes(self) -> int:
        return sum(1 for o in self.occurrences if o.context_ok)

    @property
    def evidence_context_passes(self) -> int:
        return sum(1 for o in self.evidence_occurrences if o.context_ok)

    @property
    def context_rate(self) -> float:
        return self.evidence_context_passes / max(self.evidence_count, 1)

    @property
    def max_surface_similarity(self) -> float:
        return max((o.surface_similarity for o in self.evidence_occurrences), default=0.0)

    @property
    def max_phonetic_similarity(self) -> float:
        return max((o.phonetic_similarity for o in self.evidence_occurrences), default=0.0)

    @property
    def source_forms(self) -> list[str]:
        seen: dict[str, int] = {}
        for o in self.occurrences:
            seen[o.source] = seen.get(o.source, 0) + 1
        return [k for k, _ in sorted(seen.items(), key=lambda kv: (-kv[1], kv[0].lower()))[:8]]

    @property
    def target_forms(self) -> list[str]:
        seen: dict[str, int] = {}
        for o in self.occurrences:
            seen[o.target] = seen.get(o.target, 0) + 1
        return [k for k, _ in sorted(seen.items(), key=lambda kv: (-kv[1], kv[0].lower()))[:8]]


def latest_corpus() -> Path:
    full = sorted((LAB / "corpus").glob("learning_corpus_full_*.jsonl"))
    if full:
        return full[-1]
    return learning_loop.latest_corpus()


def load_rows(path: Path, limit: int | None) -> list[dict[str, Any]]:
    rows = learning_loop.load_rows(path, limit)
    for row in rows:
        if "edit_bucket_lab" not in row:
            row["edit_bucket_lab"] = learning_loop.edit_bucket(row.get("polished_output"), row.get("user_kept"))
    return rows


def surface_similarity(a: str, b: str) -> float:
    return difflib.SequenceMatcher(None, learning_loop.alias_norm(a), learning_loop.alias_norm(b)).ratio()


def target_is_directive_domain(target: str) -> bool:
    canonical = replay.canonical_target_norm(target)
    if not canonical:
        return False
    return canonical in replay.CANONICAL_TARGETS


def source_is_high_signal(source: str) -> bool:
    norm = learning_loop.alias_norm(source)
    compact = replay.compact(source)
    words = norm.split()
    if not norm:
        return False
    if any(ch.isdigit() for ch in source):
        return True
    letters = re.sub(r"[^A-Za-z]", "", source)
    if 2 <= len(letters) <= 8 and letters.upper() == letters and any(ch.isupper() for ch in source):
        return True
    if len(words) >= 2 and any(word in {"ko", "go", "vo", "wo", "o"} for word in words):
        return True
    if compact and len(compact) >= 4 and not any(v in compact.lower() for v in "aeiou"):
        return True
    if any(ch in source for ch in "_/@#.-"):
        return True
    return False


def source_is_common_or_dangerous(source: str, canonical_target_norm: str) -> str | None:
    norm = learning_loop.alias_norm(source)
    words = norm.split()
    risky_ordinary_sources = {
        "century",
        "deaf",
        "def",
        "dev",
        "deve",
        "lag",
        "lock",
        "log",
        "side",
        "sight",
        "site",
    }
    if not norm:
        return "empty_source"
    if len(words) == 1 and norm in learning_loop.COMMON_SOURCE_WORDS:
        return "common_single_word_source"
    if len(words) == 1 and norm in risky_ordinary_sources:
        return "risky_ordinary_single_word_source"
    if norm == "dev container":
        return "risky_ordinary_phrase_source"
    if len(norm) <= 2 and not any(ch.isdigit() for ch in norm):
        return "too_short_source"
    return None


def row_content_key(row: dict[str, Any]) -> str:
    hashes = row.get("text_hashes") if isinstance(row.get("text_hashes"), dict) else {}
    parts = [
        hashes.get("raw_stt") or "",
        hashes.get("polished_output") or "",
        hashes.get("user_kept") or "",
    ]
    if not any(parts):
        raw = row.get("raw_stt") or row.get("transcript") or ""
        polished = row.get("polished_output") or ""
        kept = row.get("user_kept") or ""
        payload = "\n---\n".join([raw, polished, kept])
        return hashlib.sha256(payload.encode("utf-8")).hexdigest()
    return "|".join(parts)


def occurrence_from_candidate(row: dict[str, Any], candidate: learning_loop.Candidate) -> Occurrence:
    raw = row.get("raw_stt") or row.get("transcript") or ""
    target = replay.canonical_target(candidate.target)
    target_norm = replay.canonical_target_norm(target)
    return Occurrence(
        sample_id=str(row.get("sample_id") or ""),
        content_key=row_content_key(row),
        source=candidate.source,
        target=target,
        raw_stt=raw,
        old_polished=row.get("polished_output") or "",
        user_kept=row.get("user_kept") or "",
        source_label=str(row.get("source") or "unknown"),
        edit_bucket=str(row.get("edit_bucket_lab") or row.get("edit_bucket") or ""),
        account_hash=row.get("account_hash"),
        context_ok=replay.target_context_allows(target_norm, learning_loop.alias_norm(raw)),
        surface_similarity=surface_similarity(candidate.source, target),
        phonetic_similarity=replay.phonetic_similarity(candidate.source, target),
    )


def collect_aggregates(rows: list[dict[str, Any]]) -> dict[tuple[str, str], Aggregate]:
    out: dict[tuple[str, str], Aggregate] = {}
    for row in rows:
        if not learning_loop.should_learn_from_row(row):
            continue
        for c in learning_loop.extract_candidates(row):
            target = replay.canonical_target(c.target)
            key = (c.source_norm, replay.canonical_target_norm(target))
            if not key[0] or not key[1]:
                continue
            agg = out.get(key)
            if not agg:
                agg = Aggregate(
                    source_norm=key[0],
                    target_norm=key[1],
                    canonical_target=target,
                )
                out[key] = agg
            agg.occurrences.append(occurrence_from_candidate(row, c))
    return out


def classify(agg: Aggregate) -> tuple[str, list[str], float]:
    reasons: list[str] = []
    target_norm = agg.target_norm
    source_norm = agg.source_norm

    if not target_is_directive_domain(agg.canonical_target):
        return "reject", ["target_not_directive_domain"], 0.0

    dangerous = source_is_common_or_dangerous(source_norm, target_norm)
    if dangerous:
        return "reject", [dangerous], 0.0

    if len(source_norm.split()) > 4:
        return "reject", ["source_phrase_too_broad"], 0.0

    if agg.context_passes == 0:
        return "reject", ["target_context_never_passed"], 0.0

    if agg.context_rate < 0.5:
        reasons.append("weak_context_rate")

    high_signal = any(source_is_high_signal(o.source) for o in agg.occurrences)
    repeated = agg.evidence_count >= 2
    multi_account = len(agg.evidence_accounts) >= 2
    strong_surface = agg.max_surface_similarity >= 0.78
    strong_phonetic = agg.max_phonetic_similarity >= 0.72

    score = 0.0
    score += min(0.35, agg.evidence_count * 0.08)
    score += 0.20 if agg.context_rate >= 0.8 else 0.10
    score += 0.15 if high_signal else 0.0
    score += 0.12 if strong_surface else 0.0
    score += 0.10 if strong_phonetic else 0.0
    score += 0.08 if multi_account else 0.0
    score = min(score, 1.0)

    if high_signal:
        reasons.append("high_signal_source")
    if repeated:
        reasons.append("repeated")
    if multi_account:
        reasons.append("multi_account")
    if strong_surface:
        reasons.append("strong_surface_similarity")
    if strong_phonetic:
        reasons.append("strong_phonetic_similarity")

    # Directive memories are intentionally stricter than soft prompt hints.
    if agg.context_rate >= 0.8 and (
        (repeated and high_signal)
        or (repeated and strong_surface)
        or (multi_account and high_signal)
        or (agg.evidence_count >= 3 and high_signal)
    ):
        return "safe_directive", reasons or ["safe_by_rule"], score

    if agg.context_rate >= 0.7 and (high_signal or repeated or strong_surface or strong_phonetic):
        return "soft_hint_only", reasons or ["soft_by_rule"], score

    if agg.context_rate >= 0.5:
        return "needs_more_evidence", reasons or ["needs_more_evidence"], score

    return "reject", reasons or ["weak_evidence"], score


def summarize(aggregates: dict[tuple[str, str], Aggregate]) -> dict[str, Any]:
    rows: list[dict[str, Any]] = []
    for agg in aggregates.values():
        label, reasons, score = classify(agg)
        examples = sorted(agg.occurrences, key=lambda o: o.sample_id)[:3]
        rows.append(
            {
                "source_norm": agg.source_norm,
                "target_norm": agg.target_norm,
                "canonical_target": agg.canonical_target,
                "label": label,
                "score": round(score, 4),
                "reasons": reasons,
                "count": agg.count,
                "evidence_count": agg.evidence_count,
                "account_count": len(agg.accounts),
                "evidence_account_count": len(agg.evidence_accounts),
                "context_passes": agg.context_passes,
                "evidence_context_passes": agg.evidence_context_passes,
                "context_rate": round(agg.context_rate, 4),
                "max_surface_similarity": round(agg.max_surface_similarity, 4),
                "max_phonetic_similarity": round(agg.max_phonetic_similarity, 4),
                "source_forms": agg.source_forms,
                "target_forms": agg.target_forms,
                "examples": [
                    {
                        "sample_id": o.sample_id,
                        "source": o.source,
                        "target": o.target,
                        "source_label": o.source_label,
                        "edit_bucket": o.edit_bucket,
                        "context_ok": o.context_ok,
                        "raw_stt": o.raw_stt,
                        "old_polished": o.old_polished,
                        "user_kept": o.user_kept,
                    }
                    for o in examples
                ],
            }
        )

    counts: dict[str, int] = {}
    for row in rows:
        counts[row["label"]] = counts.get(row["label"], 0) + 1

    return {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "candidate_count": len(rows),
        "label_counts": counts,
        "candidates": sorted(rows, key=lambda r: (r["label"] != "safe_directive", -r["score"], -r["count"], r["canonical_target"])),
    }


def write_report(summary: dict[str, Any], corpus: Path) -> tuple[Path, Path]:
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    json_path = RUNS_DIR / f"memory_candidate_judge_{stamp}.json"
    md_path = RUNS_DIR / f"memory_candidate_judge_{stamp}.md"
    json_path.write_text(json.dumps({**summary, "corpus": str(corpus)}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    lines = [
        "# Memory Candidate Judge",
        "",
        f"- Corpus: `{corpus}`",
        f"- Candidate pairs: {summary['candidate_count']}",
        "",
        "## Label Counts",
        "",
        "| Label | Count |",
        "|---|---:|",
    ]
    for label, count in sorted(summary["label_counts"].items()):
        lines.append(f"| `{label}` | {count} |")

    def add_section(title: str, label: str, limit: int) -> None:
        lines.extend(["", f"## {title}", ""])
        selected = [c for c in summary["candidates"] if c["label"] == label][:limit]
        if not selected:
            lines.append("None.")
            return
        for c in selected:
            lines.extend(
                [
                    f"### `{c['source_norm']}` -> `{c['canonical_target']}`",
                    "",
                    f"- Label: `{c['label']}`",
                    f"- Score: {c['score']}",
                    f"- Count/accounts/context: {c['count']} raw, {c['evidence_count']} unique / {c['account_count']} raw, {c['evidence_account_count']} unique / {c['evidence_context_passes']} unique ({c['context_rate']:.1%})",
                    f"- Reasons: {', '.join(c['reasons'])}",
                    f"- Surface/phonetic: {c['max_surface_similarity']:.2f} / {c['max_phonetic_similarity']:.2f}",
                    "",
                ]
            )
            for ex in c["examples"][:2]:
                lines.extend(
                    [
                        f"- Example `{ex['sample_id']}` context_ok={ex['context_ok']}",
                        f"  - raw: {ex['raw_stt'][:240]}",
                        f"  - kept: {ex['user_kept'][:240]}",
                    ]
                )

    add_section("Safe Directives", "safe_directive", 40)
    add_section("Soft Hints", "soft_hint_only", 30)
    add_section("Needs More Evidence", "needs_more_evidence", 30)
    add_section("Rejected / Dangerous", "reject", 50)

    md_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return json_path, md_path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=None)
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()

    corpus = args.corpus or latest_corpus()
    rows = load_rows(corpus, args.limit)
    aggregates = collect_aggregates(rows)
    summary = summarize(aggregates)
    json_path, md_path = write_report(summary, corpus)
    print(
        json.dumps(
            {
                "corpus": str(corpus),
                "candidate_count": summary["candidate_count"],
                "label_counts": summary["label_counts"],
                "json": str(json_path),
                "report": str(md_path),
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
