#!/usr/bin/env python3
"""Evaluate dynamic vocabulary retrieval strategies against AirNote STT history.

This lab is intentionally read-only. It loads *one user's* local vocabulary and
approved STT aliases, then replays real raw STT rows to compare selector
variants. The goal is to tune retrieval gates with measured recall/precision
before changing production code.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sqlite3
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

LAB = Path(__file__).resolve().parent
RUNS_DIR = LAB / "corpus" / "vocab_retrieval_runs"
DEFAULT_LOCAL_DB = Path.home() / "Library/Application Support/VoicePolish/db.sqlite"
DEFAULT_CORPUS = LAB / "corpus" / "learning_corpus_full_20260703T0931Z.jsonl"
DEFAULT_ADJUDICATION = LAB / "vocab_retrieval_adjudications.json"

TOKEN_RE = re.compile(r"[A-Za-z0-9_@.+#/-]+|[\u0900-\u097F]+", re.UNICODE)

COMMON_WORDS = {
    "a",
    "ab",
    "about",
    "again",
    "all",
    "am",
    "and",
    "app",
    "are",
    "be",
    "but",
    "do",
    "for",
    "from",
    "hai",
    "hain",
    "he",
    "i",
    "in",
    "is",
    "it",
    "ka",
    "kar",
    "ke",
    "ki",
    "ko",
    "main",
    "me",
    "mein",
    "mere",
    "mujhe",
    "my",
    "nahin",
    "not",
    "of",
    "on",
    "or",
    "so",
    "that",
    "the",
    "this",
    "to",
    "we",
    "what",
    "will",
    "with",
    "ye",
    "you",
}

# Confidence tiering (NOT a denylist). The retrieval layer surfaces candidates;
# whether an ambiguous fuzzy match ("macos" vs vocab "Macobs") is actually
# applied is a semantic, in-context call left to the polish LLM. To avoid prompt
# pollution we tag each selection so the LLM knows how much to trust it:
#   apply   = ground-truth string identity (exact / split-form / approved alias)
#   suggest = fuzzy near-miss (surface WITH the term's meaning, LLM decides)
APPLY_REASONS = {"exact_term", "exact_split_term", "exact_alias"}


def selection_tier(reason: str) -> str:
    return "apply" if reason in APPLY_REASONS else "suggest"


@dataclass
class VocabTerm:
    term: str
    source: str = "auto"
    weight: float = 1.0
    use_count: int = 0
    term_type: str | None = None
    meaning: str | None = None
    example_context: str | None = None
    aliases: list[str] = field(default_factory=list)

    @property
    def key(self) -> str:
        return norm(self.term)

    @property
    def has_meaning(self) -> bool:
        return bool((self.meaning or "").strip())

    @property
    def has_context(self) -> bool:
        return bool((self.example_context or "").strip())


@dataclass
class SttRow:
    sample_id: str
    source: str
    raw_stt: str
    user_kept: str | None = None
    polished_output: str | None = None
    created_at_ms: int | None = None


@dataclass
class SelectedTerm:
    term: str
    score: float
    reason: str
    evidence: str
    gate: str


@dataclass
class TermTrace:
    """Why the v3 selector accepted or rejected a single term on a single row.

    This is the review-loop primitive: every candidate exits at exactly one
    annotated branch, so a miss is never a silent `[]` — it carries the gate it
    died at plus the best surface/phonetic evidence it could muster.
    """
    term: str
    status: str      # "selected" | "rejected"
    gate: str        # branch that fired (e.g. hybrid_dynamic_v2 / guard / below_threshold)
    reason: str      # short machine label
    score: float
    surface: float
    phonetic: float
    evidence: str

    def as_dict(self) -> dict[str, Any]:
        return {
            "term": self.term,
            "status": self.status,
            "gate": self.gate,
            "reason": self.reason,
            "score": round(self.score, 4),
            "surface": round(self.surface, 4),
            "phonetic": round(self.phonetic, 4),
            "evidence": self.evidence,
        }


def norm(text: str | None) -> str:
    text = (text or "").lower()
    text = re.sub(r"[^\w@.+#/\-\u0900-\u097F ]+", " ", text, flags=re.UNICODE)
    text = re.sub(r"\s+", " ", text)
    return text.strip()


def compact(text: str | None) -> str:
    return "".join(ch for ch in norm(text) if ch.isalnum())


def tokens(text: str | None) -> list[str]:
    return [norm(t) for t in TOKEN_RE.findall(text or "") if norm(t)]


def phrase_windows(words: list[str], max_width: int = 4) -> list[str]:
    out: list[str] = []
    for width in range(1, min(max_width, len(words)) + 1):
        for start in range(0, len(words) - width + 1):
            out.append(" ".join(words[start : start + width]))
    return out


def surface_similarity(a: str | None, b: str | None) -> float:
    return difflib.SequenceMatcher(None, norm(a), norm(b)).ratio()


def phonetic_key(text: str | None) -> str:
    out: list[str] = []
    for chunk in re.findall(r"[A-Za-z]+", text or ""):
        out.append(_phonetic_chunk(chunk.lower()))
    return "".join(out)


def _phonetic_chunk(lower: str) -> str:
    if not lower:
        return ""
    buf: list[str] = []
    i = 0
    while i < len(lower):
        pair = lower[i : i + 2]
        if pair in {"wr", "kn", "gn"}:
            i += 1
            continue
        if pair == "gh":
            i += 2
            continue
        if pair == "ph":
            buf.append("F")
            i += 2
            continue
        if pair in {"sh", "ch"}:
            buf.append("X")
            i += 2
            continue
        if pair == "th":
            buf.append("0")
            i += 2
            continue
        if pair in {"ck", "qu"}:
            buf.append("K")
            i += 2
            continue
        ch = lower[i]
        buf.append(
            {
                "c": "K",
                "q": "K",
                "x": "K",
                "z": "S",
                "y": "I",
                "v": "F",
                "w": "W",
            }.get(ch, ch.upper())
        )
        i += 1

    no_vowels = [ch for idx, ch in enumerate(buf) if idx == 0 or ch not in {"A", "E", "I", "O", "U"}]
    deduped: list[str] = []
    for ch in no_vowels:
        if not deduped or deduped[-1] != ch:
            deduped.append(ch)
    return "".join(deduped)


def levenshtein(a: str, b: str) -> int:
    if not a:
        return len(b)
    if not b:
        return len(a)
    prev = list(range(len(b) + 1))
    curr = [0] * (len(b) + 1)
    for i, ca in enumerate(a, start=1):
        curr[0] = i
        for j, cb in enumerate(b, start=1):
            cost = 0 if ca == cb else 1
            curr[j] = min(curr[j - 1] + 1, prev[j] + 1, prev[j - 1] + cost)
        prev, curr = curr, prev
    return prev[-1]


def phonetic_similarity(a: str | None, b: str | None) -> float:
    ka = phonetic_key(a)
    kb = phonetic_key(b)
    if not ka and not kb:
        return 1.0
    if not ka or not kb:
        return 0.0
    return 1.0 - (levenshtein(ka, kb) / max(len(ka), len(kb)))


def jargon_score(text: str) -> float:
    t = re.sub(r"[^A-Za-z0-9_-]", "", text or "")
    if not t:
        return 0.0
    score = 0.0
    has_lower = any(ch.islower() for ch in t)
    has_upper = any(ch.isupper() for ch in t)
    has_digit = any(ch.isdigit() for ch in t)
    alpha_only = t.isalpha()
    if alpha_only and has_upper and not has_lower and 2 <= len(t) <= 8:
        score += 0.6
    if has_lower and any(i > 0 and ch.isupper() for i, ch in enumerate(t)):
        score += 0.4
    if has_digit:
        score += 0.4
    if "_" in text or "-" in text:
        score += 0.2
    if alpha_only and len(t) >= 4 and t[0].isupper() and t[1:].islower():
        score += 0.2
    return min(score, 1.0)


def term_words(term: VocabTerm) -> list[str]:
    return [w for w in re.split(r"[^a-z0-9]+", norm(term.term)) if w]


def contains_phrase(text_norm: str, phrase_norm: str) -> bool:
    if not text_norm or not phrase_norm:
        return False
    words = text_norm.split()
    phrase = phrase_norm.split()
    if not phrase:
        return False
    if len(phrase) == 1:
        return phrase[0] in words
    return any(words[i : i + len(phrase)] == phrase for i in range(0, len(words) - len(phrase) + 1))


def context_overlap(transcript_norm: str, term: VocabTerm) -> float:
    context = norm(f"{term.meaning or ''} {term.example_context or ''}")
    if not context:
        return 0.0
    tx = {w for w in transcript_norm.split() if len(w) >= 3 and w not in COMMON_WORDS}
    cx = {w for w in context.split() if len(w) >= 3 and w not in COMMON_WORDS and w not in term_words(term)}
    if not tx or not cx:
        return 0.0
    return len(tx & cx) / max(len(cx), 1)


def load_my_vocab(db_path: Path, user_id: str | None) -> tuple[str, list[VocabTerm]]:
    con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    if user_id is None:
        row = con.execute("SELECT id FROM local_user LIMIT 1").fetchone()
        if row is None:
            raise SystemExit("No local_user row found.")
        user_id = row["id"]

    vocab_rows = con.execute(
        """
        SELECT term, source, weight, use_count, term_type, meaning, example_context
          FROM vocabulary
         WHERE user_id = ?
         ORDER BY weight DESC, use_count DESC, term ASC
        """,
        (user_id,),
    ).fetchall()
    terms = [
        VocabTerm(
            term=r["term"],
            source=r["source"] or "auto",
            weight=float(r["weight"] or 0.0),
            use_count=int(r["use_count"] or 0),
            term_type=r["term_type"],
            meaning=r["meaning"],
            example_context=r["example_context"],
        )
        for r in vocab_rows
    ]
    by_key = {t.key: t for t in terms}
    alias_rows = con.execute(
        """
        SELECT correct_form, transcript_form, use_count, review_status, export_tier
          FROM stt_replacements
         WHERE user_id = ?
           AND review_status = 'approved'
         ORDER BY use_count DESC
        """,
        (user_id,),
    ).fetchall()
    for r in alias_rows:
        key = norm(r["correct_form"])
        if key in by_key:
            alias = norm(r["transcript_form"])
            if alias and alias not in by_key[key].aliases:
                by_key[key].aliases.append(alias)
    return user_id, terms


def load_local_rows(db_path: Path, user_id: str, limit: int) -> list[SttRow]:
    con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    rows: list[SttRow] = []
    for r in con.execute(
        """
        SELECT id, timestamp_ms, raw_transcript, transcript, polished_output, polished, final_text
          FROM recordings
         WHERE user_id = ?
           AND COALESCE(raw_transcript, transcript, '') <> ''
         ORDER BY timestamp_ms DESC
         LIMIT ?
        """,
        (user_id, limit),
    ):
        rows.append(
            SttRow(
                sample_id=f"local-recording:{r['id']}",
                source="local_recording",
                raw_stt=(r["raw_transcript"] or r["transcript"] or "").strip(),
                polished_output=(r["polished_output"] or r["polished"] or None),
                user_kept=r["final_text"],
                created_at_ms=r["timestamp_ms"],
            )
        )
    return rows


def load_corpus_rows(path: Path, limit: int) -> list[SttRow]:
    rows: list[SttRow] = []
    if not path.is_file():
        return rows
    with path.open("r", encoding="utf-8") as f:
        for line in f:
            if len(rows) >= limit:
                break
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            raw = (r.get("raw_stt") or r.get("transcript") or "").strip()
            if not raw:
                continue
            rows.append(
                SttRow(
                    sample_id=str(r.get("sample_id") or f"corpus:{len(rows)}"),
                    source=str(r.get("source") or "corpus"),
                    raw_stt=raw,
                    polished_output=r.get("polished_output"),
                    user_kept=r.get("user_kept"),
                    created_at_ms=r.get("created_at_ms"),
                )
            )
    return rows


def load_adjudications(path: Path) -> dict[str, list[str]]:
    if not path.is_file():
        return {}
    data = json.loads(path.read_text(encoding="utf-8"))
    out: dict[str, list[str]] = {}
    if not isinstance(data, list):
        raise SystemExit(f"Adjudication file must be a JSON array: {path}")
    for item in data:
        if not isinstance(item, dict):
            continue
        sample_id = str(item.get("sample_id") or "").strip()
        terms = item.get("valid_terms") or []
        if not sample_id or not isinstance(terms, list):
            continue
        out[sample_id] = [str(t) for t in terms if str(t).strip()]
    return out


def exact_or_alias_gold(row: SttRow, terms: list[VocabTerm]) -> dict[str, list[str]]:
    raw = norm(row.raw_stt)
    kept = norm(row.user_kept)
    gold: dict[str, list[str]] = {}
    for term in terms:
        reasons: list[str] = []
        if contains_phrase(raw, term.key):
            reasons.append("raw_exact_term")
        for alias in term.aliases:
            if contains_phrase(raw, alias):
                reasons.append(f"raw_exact_alias:{alias}")
        if kept and contains_phrase(kept, term.key) and not contains_phrase(raw, term.key):
            # Weak label only when raw has a plausible nearby surface/phonetic span.
            best = best_window_score(row.raw_stt, term)
            if best["score"] >= 0.78:
                reasons.append(f"kept_target_with_raw_similarity:{best['evidence']}")
        if reasons:
            gold[term.term] = reasons
    return gold


def apply_adjudicated_gold(gold: dict[str, list[str]], row: SttRow, adjudications: dict[str, list[str]]) -> dict[str, list[str]]:
    manual_terms = adjudications.get(row.sample_id)
    if not manual_terms:
        return gold
    merged = dict(gold)
    for term in manual_terms:
        merged.setdefault(term, []).append("manual_adjudication")
    return merged


def best_window_score(text: str, term: VocabTerm, *, max_width: int = 4) -> dict[str, Any]:
    words = tokens(text)
    best = {"score": 0.0, "evidence": "", "surface": 0.0, "phonetic": 0.0}
    target = norm(term.term)
    for window in phrase_windows(words, max_width=max_width):
        if window in COMMON_WORDS:
            continue
        surface = max(surface_similarity(window, target), surface_similarity(compact(window), compact(target)))
        phon = phonetic_similarity(window, target)
        score = max(surface, phon * 0.96)
        if score > best["score"]:
            best = {"score": score, "evidence": window, "surface": surface, "phonetic": phon}
    return best


def current_meaning_gate(row: SttRow, terms: list[VocabTerm], limit: int) -> list[SelectedTerm]:
    raw = norm(row.raw_stt)
    out: list[SelectedTerm] = []
    for term in terms:
        if term.source != "starred" and not term.has_meaning:
            continue
        if contains_phrase(raw, term.key):
            out.append(SelectedTerm(term.term, 1.0, "exact_term", term.term, "current_meaning_gate"))
            continue
        for alias in term.aliases:
            if contains_phrase(raw, alias):
                out.append(SelectedTerm(term.term, 0.96, "exact_alias", alias, "current_meaning_gate"))
                break
    return sorted(out, key=lambda x: -x.score)[:limit]


def exact_alias_open(row: SttRow, terms: list[VocabTerm], limit: int) -> list[SelectedTerm]:
    raw = norm(row.raw_stt)
    out: list[SelectedTerm] = []
    for term in terms:
        if contains_phrase(raw, term.key):
            out.append(SelectedTerm(term.term, 1.0, "exact_term", term.term, "exact_alias_open"))
            continue
        for alias in term.aliases:
            if contains_phrase(raw, alias):
                out.append(SelectedTerm(term.term, 0.96, "exact_alias", alias, "exact_alias_open"))
                break
    return sorted(out, key=lambda x: -x.score)[:limit]


def hybrid_dynamic_v1(row: SttRow, terms: list[VocabTerm], limit: int) -> list[SelectedTerm]:
    raw = norm(row.raw_stt)
    out: dict[str, SelectedTerm] = {}
    for term in terms:
        selected = score_term(row, term)
        if selected is None:
            continue
        prev = out.get(term.term)
        if prev is None or selected.score > prev.score:
            out[term.term] = selected
    return sorted(out.values(), key=lambda x: (-x.score, x.term.lower()))[:limit]


def hybrid_dynamic_v2(row: SttRow, terms: list[VocabTerm], limit: int) -> list[SelectedTerm]:
    """Stricter dynamic selector.

    Lessons from v1:
    - Short fuzzy aliases are dangerous (`do`/`did`/`dev` -> Divo, `site` -> STT).
    - Exact canonical and exact approved aliases are safe.
    - Approximate matching should be for longer precise terms only.
    - Camel/Pascal terms need split-form matching (`deep seek` -> DeepSeek).
    """
    raw = norm(row.raw_stt)
    out: dict[str, SelectedTerm] = {}
    for term in terms:
        selected = score_term_v2(raw, row.raw_stt, term)
        if selected is None:
            continue
        prev = out.get(term.term)
        if prev is None or selected.score > prev.score:
            out[term.term] = selected
    return sorted(out.values(), key=lambda x: (-x.score, x.term.lower()))[:limit]


def hybrid_dynamic_v3(row: SttRow, terms: list[VocabTerm], limit: int) -> list[SelectedTerm]:
    raw = norm(row.raw_stt)
    out: dict[str, SelectedTerm] = {}
    for term in terms:
        selected = score_term_v3(raw, row.raw_stt, term)
        if selected is None:
            continue
        prev = out.get(term.term)
        if prev is None or selected.score > prev.score:
            out[term.term] = selected
    return sorted(out.values(), key=lambda x: (-x.score, x.term.lower()))[:limit]


def split_camel(text: str) -> str:
    text = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", text or "")
    return re.sub(r"[_-]+", " ", text)


def score_term_v2(raw_norm: str, raw_text: str, term: VocabTerm) -> SelectedTerm | None:
    if not raw_norm:
        return None

    split_key = norm(split_camel(term.term))
    if contains_phrase(raw_norm, term.key):
        return SelectedTerm(term.term, 1.0, "exact_term", term.term, "hybrid_dynamic_v2")
    if split_key != term.key and contains_phrase(raw_norm, split_key):
        return SelectedTerm(term.term, 0.99, "exact_split_term", split_key, "hybrid_dynamic_v2")

    for alias in term.aliases:
        if contains_phrase(raw_norm, alias):
            return SelectedTerm(term.term, 0.98, "exact_alias", alias, "hybrid_dynamic_v2")

    precise = term.term_type in {"proper_noun", "brand", "code_identifier", "phrase"} or jargon_score(term.term) >= 0.45
    target_compact = compact(term.term)
    # Acronyms and very short targets are too collision-prone for fuzzy retrieval.
    if term.term_type == "acronym" or len(target_compact) < 5:
        return None

    best = best_window_score(raw_text, term)
    if not best["evidence"] or all(w in COMMON_WORDS for w in norm(best["evidence"]).split()):
        return None

    # Longer precise/domain terms can pass by strong surface similarity.
    if precise and best["surface"] >= 0.86:
        return SelectedTerm(
            term.term,
            0.86 + min(0.08, best["surface"] - 0.86),
            "strong_surface_term",
            best["evidence"],
            "hybrid_dynamic_v2",
        )

    # Phonetic is only a tie-breaker when surface already says "same family".
    if precise and best["phonetic"] >= 0.82 and best["surface"] >= 0.55:
        return SelectedTerm(
            term.term,
            0.82 + min(0.08, best["phonetic"] - 0.82),
            "strong_phonetic_term",
            best["evidence"],
            "hybrid_dynamic_v2",
        )

    # Lowercase vendor/tool names (`cerebras`) are still domain terms when the
    # surface form is very close. This is dynamic, based on string evidence,
    # not a target-specific rule.
    if len(target_compact) >= 7 and best["surface"] >= 0.84:
        return SelectedTerm(
            term.term,
            0.84 + min(0.08, best["surface"] - 0.84),
            "long_surface_term",
            best["evidence"],
            "hybrid_dynamic_v2",
        )
    return None


def score_term_v3(raw_norm: str, raw_text: str, term: VocabTerm) -> SelectedTerm | None:
    trace = score_term_v3_trace(raw_norm, raw_text, term)
    if trace.status == "selected":
        return SelectedTerm(trace.term, trace.score, trace.reason, trace.evidence, trace.gate)
    return None


def score_term_v3_trace(raw_norm: str, raw_text: str, term: VocabTerm) -> TermTrace:
    """Single source of truth for v3 — returns the annotated accept/reject path.

    Order mirrors the old score_term_v2 → v3 chain exactly (so metrics are
    unchanged): exact/split/alias, then the v2 strong-evidence accepts, then the
    v3 near-surface recovery accepts, else a `below_threshold` rejection that
    carries the closest evidence it found. `score_term_v3` is now a thin wrapper.
    """
    if not raw_norm:
        return TermTrace(term.term, "rejected", "input", "empty_raw", 0.0, 0.0, 0.0, "")

    split_key = norm(split_camel(term.term))
    if contains_phrase(raw_norm, term.key):
        return TermTrace(term.term, "selected", "hybrid_dynamic_v2", "exact_term", 1.0, 1.0, 1.0, term.term)
    if split_key != term.key and contains_phrase(raw_norm, split_key):
        return TermTrace(term.term, "selected", "hybrid_dynamic_v2", "exact_split_term", 0.99, 1.0, 1.0, split_key)
    for alias in term.aliases:
        if contains_phrase(raw_norm, alias):
            return TermTrace(term.term, "selected", "hybrid_dynamic_v2", "exact_alias", 0.98, 1.0, 1.0, alias)

    precise = term.term_type in {"proper_noun", "brand", "code_identifier", "phrase"} or jargon_score(term.term) >= 0.45
    target_compact = compact(term.term)
    # Acronyms and very short targets are too collision-prone for fuzzy retrieval.
    if term.term_type == "acronym" or len(target_compact) < 5:
        return TermTrace(term.term, "rejected", "guard", "acronym_or_short_target", 0.0, 0.0, 0.0, "")

    best = best_window_score(raw_text, term)
    surface = float(best["surface"])
    phon = float(best["phonetic"])
    evidence = str(best["evidence"])
    if not evidence or all(w in COMMON_WORDS for w in norm(evidence).split()):
        return TermTrace(term.term, "rejected", "guard", "no_content_evidence", 0.0, surface, phon, evidence)

    # ── v2 strong-evidence accepts ──────────────────────────────────────────
    if precise and surface >= 0.86:
        return TermTrace(term.term, "selected", "hybrid_dynamic_v2", "strong_surface_term",
                         0.86 + min(0.08, surface - 0.86), surface, phon, evidence)
    if precise and phon >= 0.82 and surface >= 0.55:
        return TermTrace(term.term, "selected", "hybrid_dynamic_v2", "strong_phonetic_term",
                         0.82 + min(0.08, phon - 0.82), surface, phon, evidence)
    if len(target_compact) >= 7 and surface >= 0.84:
        return TermTrace(term.term, "selected", "hybrid_dynamic_v2", "long_surface_term",
                         0.84 + min(0.08, surface - 0.84), surface, phon, evidence)

    # ── v3 near-surface recovery (anupra, cerebrace, macos) ─────────────────
    if precise and surface >= 0.82:
        return TermTrace(term.term, "selected", "hybrid_dynamic_v3", "near_surface_precise_term",
                         0.82 + min(0.08, surface - 0.82), surface, phon, evidence)
    if len(target_compact) >= 7 and surface >= 0.82:
        return TermTrace(term.term, "selected", "hybrid_dynamic_v3", "near_surface_long_term",
                         0.82 + min(0.08, surface - 0.82), surface, phon, evidence)

    # Rejected: the closest window sat below every accept threshold.
    reason = f"below_threshold(precise={int(precise)},len={len(target_compact)})"
    return TermTrace(term.term, "rejected", "below_threshold", reason, max(surface, phon), surface, phon, evidence)


def score_term(row: SttRow, term: VocabTerm) -> SelectedTerm | None:
    raw = norm(row.raw_stt)
    if not raw:
        return None

    if term.source == "starred":
        # Pinned terms get a low-priority soft pass only if there is at least
        # some surface/phonetic evidence. Starred is intent, not a global replace.
        best = best_window_score(row.raw_stt, term)
        if best["score"] >= 0.52:
            return SelectedTerm(term.term, 0.62, "starred_soft_evidence", best["evidence"], "hybrid_dynamic_v1")

    if contains_phrase(raw, term.key):
        return SelectedTerm(term.term, 1.0, "exact_term", term.term, "hybrid_dynamic_v1")

    for alias in term.aliases:
        if contains_phrase(raw, alias):
            return SelectedTerm(term.term, 0.98, "exact_alias", alias, "hybrid_dynamic_v1")

    best_alias: tuple[float, str, str] | None = None
    for alias in term.aliases:
        if len(compact(alias)) < 3:
            continue
        score = best_alias_window_score(row.raw_stt, alias, term)
        if best_alias is None or score[0] > best_alias[0]:
            best_alias = score
    if best_alias and best_alias[0] >= 0.82:
        return SelectedTerm(term.term, best_alias[0], best_alias[2], best_alias[1], "hybrid_dynamic_v1")

    best = best_window_score(row.raw_stt, term)
    is_precise_term = term.term_type in {"acronym", "proper_noun", "brand", "code_identifier", "phrase"} or jargon_score(term.term) >= 0.45
    ctx = context_overlap(raw, term)
    if is_precise_term and best["surface"] >= 0.86:
        return SelectedTerm(term.term, 0.86 + min(0.08, best["surface"] - 0.86), "surface_term", best["evidence"], "hybrid_dynamic_v1")
    if is_precise_term and best["phonetic"] >= 0.80 and best["surface"] >= 0.38:
        return SelectedTerm(term.term, 0.80 + min(0.08, best["phonetic"] - 0.80), "phonetic_term", best["evidence"], "hybrid_dynamic_v1")
    if ctx >= 0.18 and best["phonetic"] >= 0.68 and best["surface"] >= 0.32:
        return SelectedTerm(term.term, 0.74 + min(0.08, ctx), "context_plus_phonetic", best["evidence"], "hybrid_dynamic_v1")
    return None


def best_alias_window_score(text: str, alias: str, term: VocabTerm) -> tuple[float, str, str]:
    words = tokens(text)
    best = (0.0, "", "miss")
    alias_norm = norm(alias)
    for window in phrase_windows(words, max_width=max(1, min(4, len(alias_norm.split()) + 1))):
        if not window or all(w in COMMON_WORDS for w in window.split()):
            continue
        surface = max(surface_similarity(window, alias_norm), surface_similarity(compact(window), compact(alias_norm)))
        phon = phonetic_similarity(window, alias_norm)
        target_phon = phonetic_similarity(window, term.term)
        score = max(surface, phon * 0.96, target_phon * 0.90)
        reason = "surface_alias" if surface >= max(phon, target_phon) else "phonetic_alias"
        if score > best[0]:
            best = (score, window, reason)
    return best


def top_weight_soft(row: SttRow, terms: list[VocabTerm], limit: int) -> list[SelectedTerm]:
    return [
        SelectedTerm(t.term, 0.20, "top_weight_no_evidence", "", "top_weight_soft")
        for t in sorted(terms, key=lambda x: (-x.weight, -x.use_count, x.term.lower()))[:limit]
    ]


VARIANTS: dict[str, Callable[[SttRow, list[VocabTerm], int], list[SelectedTerm]]] = {
    "current_meaning_gate": current_meaning_gate,
    "exact_alias_open": exact_alias_open,
    "hybrid_dynamic_v1": hybrid_dynamic_v1,
    "hybrid_dynamic_v2": hybrid_dynamic_v2,
    "hybrid_dynamic_v3": hybrid_dynamic_v3,
    "top_weight_soft": top_weight_soft,
}


def evaluate_variant(
    name: str,
    selector: Callable[[SttRow, list[VocabTerm], int], list[SelectedTerm]],
    rows: list[SttRow],
    terms: list[VocabTerm],
    limit: int,
    adjudications: dict[str, list[str]],
) -> dict[str, Any]:
    cases: list[dict[str, Any]] = []
    gold_rows = 0
    gold_hits = 0
    selected_count = 0
    correct_selected = 0
    no_gold_rows = 0
    no_gold_with_selection = 0
    apply_selected = 0
    apply_correct = 0
    suggest_selected = 0
    suggest_correct = 0
    term_by_name = {t.term: t for t in terms}

    for row in rows:
        gold = apply_adjudicated_gold(exact_or_alias_gold(row, terms), row, adjudications)
        selected = selector(row, terms, limit)
        selected_names = {s.term for s in selected}
        selected_count += len(selected)
        # Split by confidence tier: apply (ground-truth string identity) vs
        # suggest (fuzzy near-miss deferred to the polish LLM). Apply-tier
        # precision is the number that actually matters — a wrong apply corrupts
        # output, whereas a wrong suggest is arbitrated away downstream.
        for s in selected:
            if selection_tier(s.reason) == "apply":
                apply_selected += 1
                apply_correct += 1 if (gold and s.term in gold) else 0
            else:
                suggest_selected += 1
                suggest_correct += 1 if (gold and s.term in gold) else 0
        if gold:
            gold_rows += 1
            hit = bool(set(gold) & selected_names)
            gold_hits += 1 if hit else 0
            correct_selected += sum(1 for s in selected if s.term in gold)
        else:
            no_gold_rows += 1
            if selected:
                no_gold_with_selection += 1
        if gold or selected:
            # Review trace: explain v3's decision for every gold term so a miss
            # says *why* (which gate, how close), not just an empty selection.
            raw_norm = norm(row.raw_stt)
            gold_trace = [
                score_term_v3_trace(raw_norm, row.raw_stt, term_by_name[g]).as_dict()
                for g in gold
                if g in term_by_name
            ]
            cases.append(
                {
                    "sample_id": row.sample_id,
                    "source": row.source,
                    "raw_stt": row.raw_stt,
                    "user_kept": row.user_kept,
                    "gold": gold,
                    "selected": [s.__dict__ for s in selected],
                    "hit": bool(set(gold) & selected_names) if gold else None,
                    "wrong_selected": [s.term for s in selected if s.term not in gold],
                    "v3_gold_trace": gold_trace,
                }
            )

    precision = correct_selected / max(selected_count, 1)
    recall = gold_hits / max(gold_rows, 1)
    false_positive_row_rate = no_gold_with_selection / max(no_gold_rows, 1)
    f1 = (2 * precision * recall / max(precision + recall, 1e-9)) if selected_count else 0.0
    apply_precision = apply_correct / max(apply_selected, 1)
    suggest_precision = suggest_correct / max(suggest_selected, 1)
    return {
        "variant": name,
        "rows": len(rows),
        "gold_rows": gold_rows,
        "gold_hits": gold_hits,
        "recall": recall,
        "selected_count": selected_count,
        "correct_selected": correct_selected,
        "precision": precision,
        "f1": f1,
        "no_gold_rows": no_gold_rows,
        "no_gold_with_selection": no_gold_with_selection,
        "false_positive_row_rate": false_positive_row_rate,
        "apply_selected": apply_selected,
        "apply_correct": apply_correct,
        "apply_precision": apply_precision,
        "suggest_selected": suggest_selected,
        "suggest_correct": suggest_correct,
        "suggest_precision": suggest_precision,
        "cases": cases,
    }


def write_report(summary: dict[str, Any]) -> tuple[Path, Path]:
    RUNS_DIR.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    json_path = RUNS_DIR / f"vocab_retrieval_{stamp}.json"
    md_path = RUNS_DIR / f"vocab_retrieval_{stamp}.md"
    json_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True), encoding="utf-8")

    lines = [
        "# Vocab Retrieval Lab",
        "",
        f"- Rows: {summary['row_count']}",
        f"- Vocab terms: {summary['vocab_count']}",
        f"- Alias count: {summary['alias_count']}",
        f"- Transcript source: {summary['transcript_source']}",
        "",
        "## Variant Summary",
        "",
        "| Variant | Recall | Precision | F1 | False-positive row rate | Gold rows | Selected |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    tier_lines = [
        "",
        "## Confidence Tiers",
        "",
        "`apply` = ground-truth string identity (safe to rewrite). `suggest` = "
        "fuzzy near-miss surfaced to the polish LLM with the term's meaning; the "
        "LLM arbitrates in context (macos vs Macobs). Apply-tier precision is the "
        "number that must stay ~100% — a wrong apply corrupts output.",
        "",
        "| Variant | Apply selected | Apply precision | Suggest selected | Suggest precision |",
        "|---|---:|---:|---:|---:|",
    ]
    for result in summary["results"]:
        tier_lines.append(
            "| {variant} | {apply_selected} | {apply_precision:.1%} | {suggest_selected} | {suggest_precision:.1%} |".format(
                **result
            )
        )

    for result in summary["results"]:
        lines.append(
            "| {variant} | {recall:.1%} | {precision:.1%} | {f1:.1%} | {false_positive_row_rate:.1%} | {gold_rows} | {selected_count} |".format(
                **result
            )
        )

    lines.extend(tier_lines)
    lines.extend(["", "## Misses And Wrong Selections", ""])
    for result in summary["results"]:
        misses = [c for c in result["cases"] if c.get("hit") is False]
        wrongs = [c for c in result["cases"] if c.get("wrong_selected")]
        lines.append(f"### {result['variant']}")
        lines.append("")
        lines.append(f"- Misses: {len(misses)}")
        lines.append(f"- Rows with wrong selections: {len(wrongs)}")
        for case in (misses[:8] + wrongs[:8])[:12]:
            lines.append("")
            lines.append(f"- `{case['sample_id']}`")
            lines.append(f"  - Raw: {case['raw_stt'][:220]}")
            lines.append(f"  - Gold: {list(case['gold'].keys())}")
            lines.append(
                "  - Selected: "
                + str([(s["term"], round(s["score"], 3), s["reason"], s["evidence"]) for s in case["selected"]])
            )
            for tr in case.get("v3_gold_trace", []):
                if tr["status"] == "rejected":
                    lines.append(
                        f"  - v3 rejected `{tr['term']}` at {tr['gate']}/{tr['reason']} "
                        f"(surface={tr['surface']:.2f}, phon={tr['phonetic']:.2f}, evidence='{tr['evidence']}')"
                    )
                else:
                    lines.append(
                        f"  - v3 would select `{tr['term']}` via {tr['reason']} "
                        f"(score={tr['score']:.2f}, evidence='{tr['evidence']}')"
                    )
            if case.get("wrong_selected"):
                lines.append(f"  - Wrong: {case['wrong_selected']}")
        lines.append("")

    # ── v3 review loop: borderline gold rejections — the "one more gate?" set ──
    v3 = next((r for r in summary["results"] if r["variant"] == "hybrid_dynamic_v3"), None)
    if v3:
        borderline: list[tuple[dict[str, Any], dict[str, Any]]] = []
        for case in v3["cases"]:
            if case.get("hit") is not False:
                continue
            for tr in case.get("v3_gold_trace", []):
                if tr["status"] == "rejected" and 0.68 <= tr["surface"] < 0.82:
                    borderline.append((case, tr))
        borderline.sort(key=lambda ct: -ct[1]["surface"])
        lines.extend(
            [
                "## v3 Borderline Gold Rejections (surface 0.68–0.82)",
                "",
                "Missed gold terms whose closest evidence sat just under the 0.82 accept "
                "line. These are the candidates for one more gate before production.",
                "",
                f"- Count: {len(borderline)}",
                "",
            ]
        )
        for case, tr in borderline[:25]:
            lines.append(
                f"- `{case['sample_id']}` — **{tr['term']}** surface={tr['surface']:.2f} "
                f"phon={tr['phonetic']:.2f} evidence='{tr['evidence']}'"
            )
            lines.append(f"  - Raw: {case['raw_stt'][:160]}")
        lines.append("")

    md_path.write_text("\n".join(lines), encoding="utf-8")
    return json_path, md_path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, default=DEFAULT_LOCAL_DB)
    parser.add_argument("--user-id", default=None)
    parser.add_argument("--transcript-source", choices=["local", "corpus", "both"], default="local")
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--adjudications", type=Path, default=DEFAULT_ADJUDICATION)
    parser.add_argument("--limit", type=int, default=500)
    parser.add_argument("--top-k", type=int, default=10)
    parser.add_argument("--variants", default=",".join(VARIANTS.keys()))
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    user_id, terms = load_my_vocab(args.db, args.user_id)
    selected_variants = [v.strip() for v in args.variants.split(",") if v.strip()]
    unknown = [v for v in selected_variants if v not in VARIANTS]
    if unknown:
        raise SystemExit(f"Unknown variants: {unknown}. Available: {sorted(VARIANTS)}")

    rows: list[SttRow] = []
    if args.transcript_source in {"local", "both"}:
        rows.extend(load_local_rows(args.db, user_id, args.limit))
    if args.transcript_source in {"corpus", "both"}:
        rows.extend(load_corpus_rows(args.corpus, args.limit))

    alias_count = sum(len(t.aliases) for t in terms)
    adjudications = load_adjudications(args.adjudications)
    results = [
        evaluate_variant(name, VARIANTS[name], rows, terms, args.top_k, adjudications)
        for name in selected_variants
    ]
    summary = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "db": str(args.db),
        "user_id_hash": user_id[:8],
        "transcript_source": args.transcript_source,
        "row_count": len(rows),
        "vocab_count": len(terms),
        "alias_count": alias_count,
        "adjudication_file": str(args.adjudications),
        "adjudicated_rows": len(adjudications),
        "top_k": args.top_k,
        "vocab_terms": [
            {
                "term": t.term,
                "source": t.source,
                "weight": t.weight,
                "use_count": t.use_count,
                "term_type": t.term_type,
                "has_meaning": t.has_meaning,
                "has_context": t.has_context,
                "alias_count": len(t.aliases),
            }
            for t in terms
        ],
        "results": results,
    }
    json_path, md_path = write_report(summary)
    print(f"Wrote {md_path}")
    print(f"Wrote {json_path}")
    for result in results:
        print(
            "{variant}: recall={recall:.1%} precision={precision:.1%} "
            "apply_prec={apply_precision:.1%} (n={apply_selected}) "
            "suggest_prec={suggest_precision:.1%} (n={suggest_selected}) "
            "fp_rows={false_positive_row_rate:.1%}".format(**result)
        )


if __name__ == "__main__":
    main()
