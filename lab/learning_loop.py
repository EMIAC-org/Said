#!/usr/bin/env python3
"""Offline closed-loop learning simulator for AirNote dictation.

This tests learning policies on exported corpus rows without touching app DBs.

Input row shape is produced by export_learning_corpus.py:
    raw_stt/transcript -> polished_output -> user_kept

The simulator walks rows chronologically. For each row it:
  1. predicts output with current learned aliases,
  2. scores prediction against user_kept,
  3. extracts new candidate aliases from the observed correction,
  4. optionally promotes candidates based on the selected policy,
  5. replays all future rows with the updated memory.

This is not the final product pipeline. It is the cheap lab harness that tells
us which learning strategy deserves real implementation.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LAB = Path(__file__).resolve().parent
CORPUS_DIR = LAB / "corpus"

TOKEN_RE = re.compile(r"[A-Za-z0-9_@.+#/-]+|[\u0900-\u097F]+", re.UNICODE)

COMMON_SOURCE_WORDS = {
    "a",
    "again",
    "an",
    "and",
    "are",
    "app",
    "be",
    "branch",
    "build",
    "do",
    "hai",
    "hain",
    "he",
    "hello",
    "i",
    "in",
    "is",
    "it",
    "ka",
    "ke",
    "ki",
    "ko",
    "main",
    "me",
    "mein",
    "mera",
    "mere",
    "mujhe",
    "name",
    "nahin",
    "not",
    "of",
    "on",
    "or",
    "that",
    "the",
    "this",
    "time",
    "to",
    "we",
    "work",
    "wo",
    "ye",
    "you",
}

PROTECTED_HINTS = {
    "api",
    "airnote",
    "aws",
    "cerebras",
    "claude",
    "clickup",
    "codex",
    "deepgram",
    "docker",
    "desktop",
    "divo",
    "emiac",
    "gemini",
    "github",
    "gmail",
    "gpt",
    "graphql",
    "groq",
    "kafka",
    "kubernetes",
    "lark",
    "macobs",
    "n8n",
    "openrouter",
    "postgres",
    "pytorch",
    "sentry",
    "scout",
    "sqlite",
    "ssh",
    "stt",
    "tauri",
    "token",
    "webhook",
    "whisper",
    "zookeeper",
}


@dataclass
class Candidate:
    source: str
    target: str
    source_norm: str
    target_norm: str
    count: int = 0
    first_sample: str | None = None
    latest_sample: str | None = None
    protected: bool = False
    rejected_reason: str | None = None


@dataclass
class Memory:
    aliases: dict[str, str] = field(default_factory=dict)
    candidates: dict[tuple[str, str], Candidate] = field(default_factory=dict)
    promoted_by: dict[str, str] = field(default_factory=dict)


def latest_corpus() -> Path:
    paths = sorted(CORPUS_DIR.glob("learning_corpus_all_*.jsonl"))
    if not paths:
        paths = sorted(CORPUS_DIR.glob("learning_corpus_*.jsonl"))
    if not paths:
        raise SystemExit("No corpus found. Run lab/export_learning_corpus.py first.")
    return paths[-1]


def norm(text: str | None) -> str:
    text = (text or "").lower()
    text = re.sub(r"\s+", " ", text)
    text = re.sub(r"[^a-z0-9@.+#/\-\u0900-\u097F ]+", "", text)
    return text.strip()


def alias_norm(text: str | None) -> str:
    """Normalization for learnable aliases, stricter than scoring.

    Terminal punctuation/capitalization are formatting, not STT aliases.
    """
    text = (text or "").lower()
    text = re.sub(r"[^\w@\u0900-\u097F ]+", " ", text, flags=re.UNICODE)
    text = re.sub(r"\s+", " ", text)
    return text.strip()


def tokens(text: str | None) -> list[str]:
    return TOKEN_RE.findall(text or "")


def phrase_norm(parts: list[str]) -> str:
    return norm(" ".join(parts))


def is_protected_target(text: str) -> bool:
    n = norm(text)
    if any(h in n for h in PROTECTED_HINTS):
        return True
    if any(ch.isdigit() for ch in text):
        return True
    if "@" in text or re.search(r"\b[a-z0-9_-]+\.[a-z0-9_-]+\b", text, re.I):
        return True
    # CamelCase, all-caps acronyms, or mixed casing usually indicate product/code terms.
    compact = re.sub(r"[^A-Za-z0-9]", "", text)
    return bool(
        re.search(r"[a-z][A-Z]", compact)
        or (len(compact) >= 2 and compact.isupper())
        or re.search(r"[A-Za-z]+[0-9]+|[0-9]+[A-Za-z]+", compact)
    )


def is_domain_target(text: str) -> bool:
    """True only for product/code/domain terms we can safely auto-promote."""
    n = alias_norm(text)
    if not n:
        return False
    words = n.split()
    compact = "".join(words)
    compact_hints = {h.replace(" ", "") for h in PROTECTED_HINTS}
    if compact in compact_hints:
        return True
    if len(words) <= 2 and any(h in words for h in PROTECTED_HINTS):
        return True
    return False


def reject_candidate(source: str, target: str) -> str | None:
    src = alias_norm(source)
    tgt = alias_norm(target)
    if not src or not tgt:
        return "empty"
    if src == tgt:
        return "formatting_only"
    src_words = src.split()
    tgt_words = tgt.split()
    if len(src_words) > 4 or len(tgt_words) > 4:
        return "too_many_words"
    if len(tgt_words) > 2 and not is_domain_target(target):
        return "target_phrase_too_broad"
    if len(src) <= 1 or len(tgt) <= 1:
        return "too_short"
    if len(src_words) == 1 and src in COMMON_SOURCE_WORDS:
        return "common_source"
    if any("\u0900" <= ch <= "\u097F" for ch in source) and not is_protected_target(target):
        return "script_romanization_not_alias"
    if len(tgt) > max(28, len(src) * 4):
        return "target_too_long"
    return None


def extract_candidates(row: dict[str, Any]) -> list[Candidate]:
    kept = row.get("user_kept") or ""
    sources = [
        row.get("polished_output") or "",
        row.get("raw_stt") or "",
        row.get("transcript") or "",
    ]
    out: dict[tuple[str, str], Candidate] = {}

    kept_tokens = tokens(kept)
    if not kept_tokens:
        return []

    for source_text in sources:
        source_tokens = tokens(source_text)
        if not source_tokens:
            continue
        matcher = difflib.SequenceMatcher(a=[norm(t) for t in source_tokens], b=[norm(t) for t in kept_tokens])
        for tag, i1, i2, j1, j2 in matcher.get_opcodes():
            if tag not in {"replace", "delete", "insert"}:
                continue
            source_phrase = " ".join(source_tokens[i1:i2]).strip()
            target_phrase = " ".join(kept_tokens[j1:j2]).strip()
            reason = reject_candidate(source_phrase, target_phrase)
            if reason:
                continue
            key = (alias_norm(source_phrase), alias_norm(target_phrase))
            out[key] = Candidate(
                source=source_phrase,
                target=target_phrase,
                source_norm=key[0],
                target_norm=key[1],
                protected=is_protected_target(target_phrase),
            )
    return list(out.values())


def similarity(a: str | None, b: str | None) -> float:
    return difflib.SequenceMatcher(None, norm(a), norm(b)).ratio()


def apply_aliases(text: str | None, aliases: dict[str, str]) -> str:
    out = text or ""
    # Longest source first prevents "post" from firing before "post grass".
    for source_norm, target in sorted(aliases.items(), key=lambda kv: len(kv[0]), reverse=True):
        if not source_norm:
            continue
        pattern = re.compile(r"(?<![A-Za-z0-9])" + re.escape(source_norm) + r"(?![A-Za-z0-9])", re.IGNORECASE)
        out = pattern.sub(target, out)
    return out


def should_learn_from_row(row: dict[str, Any]) -> bool:
    if not row.get("user_kept"):
        return False
    bucket = row.get("edit_bucket_lab") or row.get("edit_bucket") or ""
    if bucket in {"large_rewrite", "missing_user_kept", "missing_polished"}:
        return False
    if similarity(row.get("polished_output"), row.get("user_kept")) < 0.45:
        return False
    return True


def promote(memory: Memory, candidate: Candidate, *, policy: str) -> bool:
    if policy == "shadow":
        return False
    if policy == "aggressive":
        memory.aliases[candidate.source_norm] = candidate.target
        memory.promoted_by[candidate.source_norm] = "aggressive"
        return True
    if policy == "repeat2" and candidate.count >= 2:
        memory.aliases[candidate.source_norm] = candidate.target
        memory.promoted_by[candidate.source_norm] = "repeat2"
        return True
    if policy == "conservative" and is_domain_target(candidate.target) and candidate.count >= 2:
        memory.aliases[candidate.source_norm] = candidate.target
        memory.promoted_by[candidate.source_norm] = "conservative"
        return True
    return False


def parse_time(row: dict[str, Any]) -> float:
    if row.get("created_at_ms") is not None:
        try:
            return float(row["created_at_ms"]) / 1000.0
        except (TypeError, ValueError):
            pass
    value = row.get("created_at")
    if isinstance(value, str):
        try:
            return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
        except ValueError:
            return 0.0
    return 0.0


def load_rows(path: Path, limit: int | None) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as f:
        for line in f:
            if not line.strip():
                continue
            row = json.loads(line)
            if row.get("polished_output") and row.get("user_kept"):
                rows.append(row)
            if limit and len(rows) >= limit:
                break
    rows.sort(key=parse_time)
    return rows


def run_loop(rows: list[dict[str, Any]], *, policy: str) -> dict[str, Any]:
    memory = Memory()
    results: list[dict[str, Any]] = []

    for idx, row in enumerate(rows):
        polished = row.get("polished_output") or row.get("transcript") or row.get("raw_stt") or ""
        kept = row.get("user_kept") or ""
        predicted = apply_aliases(polished, memory.aliases)
        baseline_score = similarity(polished, kept)
        learned_score = similarity(predicted, kept)

        learned_candidates = []
        promoted = []
        if should_learn_from_row(row):
            for c in extract_candidates(row):
                key = (c.source_norm, c.target_norm)
                existing = memory.candidates.get(key)
                if existing:
                    existing.count += 1
                    existing.latest_sample = row.get("sample_id")
                    c = existing
                else:
                    c.count = 1
                    c.first_sample = row.get("sample_id")
                    c.latest_sample = row.get("sample_id")
                    memory.candidates[key] = c
                learned_candidates.append({"source": c.source, "target": c.target, "count": c.count, "protected": c.protected})
                if promote(memory, c, policy=policy):
                    promoted.append({"source": c.source, "target": c.target, "count": c.count, "protected": c.protected})

        results.append(
            {
                "idx": idx,
                "sample_id": row.get("sample_id"),
                "source": row.get("source"),
                "baseline_score": baseline_score,
                "learned_score": learned_score,
                "delta": learned_score - baseline_score,
                "baseline_95": baseline_score >= 0.95,
                "learned_95": learned_score >= 0.95,
                "baseline_90": baseline_score >= 0.90,
                "learned_90": learned_score >= 0.90,
                "candidates": learned_candidates,
                "promoted": promoted,
            }
        )

    baseline_scores = [r["baseline_score"] for r in results]
    learned_scores = [r["learned_score"] for r in results]
    regressions = [r for r in results if r["delta"] < -0.001]
    improvements = [r for r in results if r["delta"] > 0.001]
    return {
        "policy": policy,
        "rows": len(rows),
        "baseline_avg": sum(baseline_scores) / max(len(baseline_scores), 1),
        "learned_avg": sum(learned_scores) / max(len(learned_scores), 1),
        "baseline_95_rate": sum(1 for r in results if r["baseline_95"]) / max(len(results), 1),
        "learned_95_rate": sum(1 for r in results if r["learned_95"]) / max(len(results), 1),
        "baseline_90_rate": sum(1 for r in results if r["baseline_90"]) / max(len(results), 1),
        "learned_90_rate": sum(1 for r in results if r["learned_90"]) / max(len(results), 1),
        "improvements": len(improvements),
        "regressions": len(regressions),
        "candidate_count": len(memory.candidates),
        "alias_count": len(memory.aliases),
        "top_aliases": [
            {
                "source": c.source,
                "target": c.target,
                "count": c.count,
                "protected": c.protected,
                "promoted_by": memory.promoted_by.get(c.source_norm),
            }
            for c in sorted(memory.candidates.values(), key=lambda c: (c.source_norm not in memory.aliases, -c.count, c.source_norm))[:50]
        ],
        "results": results,
    }


def write_report(summary: dict[str, Any], output_dir: Path) -> tuple[Path, Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    json_path = output_dir / f"learning_loop_{summary['policy']}_{stamp}.json"
    md_path = output_dir / f"learning_loop_{summary['policy']}_{stamp}.md"
    json_path.write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    lines = [
        f"# Learning loop: {summary['policy']}",
        "",
        f"- Rows: {summary['rows']}",
        f"- Baseline avg similarity: {summary['baseline_avg']:.4f}",
        f"- Learned avg similarity: {summary['learned_avg']:.4f}",
        f"- Baseline >=95%: {summary['baseline_95_rate']:.1%}",
        f"- Learned >=95%: {summary['learned_95_rate']:.1%}",
        f"- Baseline >=90%: {summary['baseline_90_rate']:.1%}",
        f"- Learned >=90%: {summary['learned_90_rate']:.1%}",
        f"- Improvements: {summary['improvements']}",
        f"- Regressions: {summary['regressions']}",
        f"- Candidate aliases: {summary['candidate_count']}",
        f"- Promoted aliases: {summary['alias_count']}",
        "",
        "## Top Aliases",
        "",
        "| Source | Target | Count | Protected | Promoted By |",
        "|---|---|---:|---:|---|",
    ]
    for alias in summary["top_aliases"]:
        lines.append(
            f"| `{alias['source']}` | `{alias['target']}` | {alias['count']} | {alias['protected']} | {alias.get('promoted_by') or ''} |"
        )
    md_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return json_path, md_path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, default=None)
    parser.add_argument(
        "--policy",
        choices=["shadow", "repeat2", "conservative", "aggressive"],
        default="conservative",
    )
    parser.add_argument("--limit", type=int)
    parser.add_argument("--out-dir", type=Path, default=CORPUS_DIR / "learning_loop_runs")
    args = parser.parse_args()

    corpus = args.corpus or latest_corpus()
    rows = load_rows(corpus, args.limit)
    if not rows:
        raise SystemExit(f"No usable rows in {corpus}")

    summary = run_loop(rows, policy=args.policy)
    summary["corpus"] = str(corpus)
    json_path, md_path = write_report(summary, args.out_dir)
    print(
        json.dumps(
            {
                "corpus": str(corpus),
                "policy": args.policy,
                "rows": summary["rows"],
                "baseline_avg": round(summary["baseline_avg"], 4),
                "learned_avg": round(summary["learned_avg"], 4),
                "baseline_95_rate": round(summary["baseline_95_rate"], 4),
                "learned_95_rate": round(summary["learned_95_rate"], 4),
                "improvements": summary["improvements"],
                "regressions": summary["regressions"],
                "candidate_count": summary["candidate_count"],
                "alias_count": summary["alias_count"],
                "json": str(json_path),
                "report": str(md_path),
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
