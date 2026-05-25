#!/usr/bin/env python3
"""Train the Tier 2 local correction scorer.

This tool is intentionally local-only: it reads Said's SQLite database,
creates candidate-pair examples from `stt_replacements` and `vocabulary`, and
exports a tiny ONNX scorer. The released app can consume the artifact but does
not run this trainer automatically.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import sqlite3
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

import numpy as np
import torch
from torch import nn
from torch.utils.data import DataLoader, TensorDataset


MAX_LEN = 32
SEED = 42
FEATURE_NAMES = [
    "edit_similarity",
    "phonetic_similarity",
    "length_ratio",
    "term_type_acronym",
    "term_type_brand",
    "term_type_proper_noun",
    "term_type_code_identifier",
    "source_manual",
    "source_starred",
    "alias_count",
    "use_count_log",
    "weight",
    "deterministic_score",
    "is_dictionary_word",
]
COMMON_WORDS = {
    "a",
    "an",
    "and",
    "are",
    "can",
    "go",
    "i",
    "in",
    "is",
    "main",
    "mein",
    "of",
    "on",
    "or",
    "return",
    "the",
    "time",
    "to",
    "with",
    "hai",
    "ka",
    "ki",
    "ke",
    # Hindi common words
    "yeh", "woh", "ye", "wo", "toh", "nahi", "nhi", "kya", "kaisa", "kaisi", "kaise",
    "aisa", "aisi", "aise", "laga", "lagi", "lage", "lagta", "lagti", "abhi", "bas", "aur", "bhi",
    "kuch", "sab", "haan", "accha", "achha", "bahut", "jab", "tab", "phir", "suno",
    "dekho", "bolo", "chalo", "theek", "kahan", "kaun", "kyun", "lekin",
    "magar", "agar", "isliye", "sirf", "bilkul", "zaroor", "pehle", "baad", "upar",
    "neeche", "andar", "bahar", "saamne", "peeche", "hota", "hoti", "hote", "karo",
    "karta", "karti", "karte", "bola", "boli", "bole", "dekh", "dekhna", "sun",
    "sunna", "bol", "bolna", "chal", "chalna", "rakh", "rakhna", "mil", "milna",
    "ban", "baith", "uth", "khol", "rok", "raha", "rahi", "rahe", "rehna",
    "wala", "wali", "wale", "dono", "dusra", "teesra", "yaar", "bhai",
    "din", "raat", "ghar", "log", "banda", "cheez", "taraf", "jagah", "waqt",
    "baar", "baat", "kaam", "tu", "tum", "aap", "hum", "mujhe", "tumhe",
    "apna", "apni", "apne", "mera", "meri", "mere", "tera", "teri", "tere",
    "uska", "uski", "unka", "unki", "isko", "usko", "sabko", "sabne",
    "tha", "thi", "the", "hoga", "hogi", "karega", "karenge", "dena", "lena",
    "jaana", "aana", "hona", "batao", "batana", "mat", "shayad",
    "ko", "se", "pe", "par", "ne", "ho", "ja", "le", "de", "aa",
    "kyunki", "warna", "zyada", "kam", "thoda", "bada", "badi", "chhota", "chhoti",
    "naya", "purana", "sahi", "galat", "please", "zara",
}

DICTIONARY_WORDS: set[str] = set()


def load_dictionary() -> None:
    global DICTIONARY_WORDS
    if DICTIONARY_WORDS:
        return
    dict_path = Path("/usr/share/dict/words")
    if dict_path.is_file():
        DICTIONARY_WORDS = {
            line.strip().lower()
            for line in dict_path.read_text().splitlines()
            if 3 <= len(line.strip()) <= 20
        }
    DICTIONARY_WORDS.update(COMMON_WORDS)
    DICTIONARY_WORDS.update({
        "dock", "oath", "white", "just", "verse", "tower", "guard", "cool",
        "press", "super", "base", "graph", "local", "red", "post", "type",
        "strip", "fire", "cell", "hub", "lab", "queue", "prism", "jot",
        "meaning", "react", "swift", "rust", "go", "dart", "flask", "spring",
        "nest", "angular", "corps", "capital", "course", "prayer", "house",
        "sent", "table", "tell", "next", "rest", "google",
    })
    # Hindi common words — must also be in dictionary for hard negative mining
    HINDI_COMMON = {
        "yeh", "woh", "ye", "wo", "toh", "nahi", "nhi", "kya", "kaisa", "kaisi", "kaise",
        "aisa", "aisi", "aise", "laga", "lagi", "lage", "lagta", "lagti", "abhi", "bas", "aur", "bhi",
        "kuch", "sab", "haan", "accha", "achha", "bahut", "jab", "tab", "phir", "dekho",
        "bolo", "chalo", "theek", "kahan", "kaun", "kyun", "lekin", "magar", "agar",
        "hota", "hoti", "hote", "karo", "karta", "karti", "bola", "boli", "bole", "dekh",
        "dekhna", "sun", "sunna", "bol", "bolna", "chal", "chalna", "rakh", "mil", "milna",
        "ban", "baith", "uth", "khol", "rok", "raha", "rahi", "rahe", "rehna", "wala", "wali",
        "wale", "dono", "dusra", "yaar", "bhai", "din", "raat", "ghar", "log", "banda",
        "cheez", "jagah", "waqt", "baar", "baat", "kaam", "mujhe", "tumhe", "apna", "apni",
        "mera", "meri", "uska", "uski", "unka", "sabko", "tha", "thi", "hoga", "karega",
        "dena", "lena", "jaana", "aana", "hona", "batao", "mat", "shayad", "kyunki", "warna",
        "zyada", "thoda", "bada", "chhota", "naya", "purana", "sahi", "galat", "zara",
    }
    DICTIONARY_WORDS.update(HINDI_COMMON)


def is_dictionary_word(token: str) -> bool:
    load_dictionary()
    return normalize(token) in DICTIONARY_WORDS


def is_common_alias_source(text: str) -> bool:
    load_dictionary()
    tokens = [normalize(part) for part in text.strip().split() if normalize(part)]
    if not tokens:
        return False
    joined = "".join(tokens)
    return joined in COMMON_WORDS or all(token in COMMON_WORDS or token in DICTIONARY_WORDS for token in tokens)


def mine_hard_negatives_from_dictionary(
    protected: list["VocabTerm"],
    cache_path: Path | None = None,
) -> list["Example"]:
    # Try loading from cache first
    if cache_path and cache_path.is_file():
        term_norms = {normalize(t.term) for t in protected}
        try:
            cached = json.loads(cache_path.read_text())
            if set(cached.get("terms", [])) == term_norms:
                by_norm = {normalize(t.term): t for t in protected}
                negatives = []
                for pair in cached.get("pairs", []):
                    cand = by_norm.get(pair["term"])
                    if cand:
                        negatives.append(Example(pair["word"], cand, 0.0))
                return negatives
        except Exception:
            pass

    load_dictionary()
    negatives = []
    pairs = []
    for term in protected:
        term_norm = normalize(term.term)
        tlen = len(term_norm)
        if tlen == 0:
            continue
        for word in DICTIONARY_WORDS:
            if word == term_norm:
                continue
            wlen = len(word)
            if wlen < 3:
                continue
            ratio = min(wlen, tlen) / max(wlen, tlen)
            if ratio < 0.40:
                continue
            sim = edit_similarity(word, term_norm)
            if sim >= 0.45:
                negatives.append(Example(word, term, 0.0))
                pairs.append({"word": word, "term": term_norm})

    # Save cache
    if cache_path:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        cache_data = {
            "terms": sorted({normalize(t.term) for t in protected}),
            "pairs": pairs,
        }
        cache_path.write_text(json.dumps(cache_data), encoding="utf-8")

    return negatives


@dataclass(frozen=True)
class VocabTerm:
    term: str
    term_type: str
    source: str
    weight: float
    use_count: int


@dataclass(frozen=True)
class AliasRule:
    transcript_form: str
    correct_form: str
    weight: float
    use_count: int
    review_status: str


@dataclass(frozen=True)
class PolicyWeight:
    token_norm: str
    correct_form: str
    correct_form_norm: str
    positive_count: int
    negative_count: int
    learned_weight: float


@dataclass(frozen=True)
class Example:
    token: str
    candidate: VocabTerm
    label: float


class TinyCorrectionModel(nn.Module):
    def __init__(self, vocab_size: int, feature_count: int) -> None:
        super().__init__()
        self.embedding = nn.Embedding(vocab_size, 32, padding_idx=0)
        self.trunk = nn.Sequential(
            nn.Linear(32 * 4 + feature_count, 96),
            nn.ReLU(),
            nn.Dropout(0.30),
            nn.Linear(96, 32),
            nn.ReLU(),
            nn.Dropout(0.15),
        )
        self.head = nn.Linear(32, 1)
        self.skip = nn.Linear(feature_count, 1)
        self.det_gate_idx = FEATURE_NAMES.index("deterministic_score")

    def _pool(self, ids: torch.Tensor) -> torch.Tensor:
        emb = self.embedding(ids)
        mask = (ids != 0).float().unsqueeze(-1)
        denom = mask.sum(dim=1).clamp_min(1.0)
        return (emb * mask).sum(dim=1) / denom

    def forward(
        self,
        token_ids: torch.Tensor,
        candidate_ids: torch.Tensor,
        features: torch.Tensor,
    ) -> torch.Tensor:
        token = self._pool(token_ids)
        candidate = self._pool(candidate_ids)
        joined = torch.cat(
            [token, candidate, torch.abs(token - candidate), token * candidate, features],
            dim=1,
        )
        logit = self.head(self.trunk(joined)) + self.skip(features)
        det_score = features[:, self.det_gate_idx : self.det_gate_idx + 1]
        gate = torch.sigmoid(det_score * 10.0 - 5.0)
        return torch.sigmoid(logit) * gate


def normalize(text: str) -> str:
    return "".join(ch.lower() for ch in text.strip() if ch.isalnum() or ch in "_-")


def classify_term_type(term: str) -> str:
    text = term.strip()
    if not text:
        return "other"
    if any(ch.isspace() for ch in text):
        return "phrase"
    alpha_only = all(ch.isalpha() and ch.isascii() for ch in text)
    has_lower = any(ch.islower() for ch in text)
    has_upper = any(ch.isupper() for ch in text)
    has_digit = any(ch.isdigit() for ch in text)
    upper_after_first = any(idx > 0 and ch.isupper() for idx, ch in enumerate(text))
    if alpha_only and has_upper and not has_lower and 2 <= len(text) <= 8:
        return "acronym"
    if has_digit or "_" in text or "-" in text:
        return "code_identifier"
    if has_lower and upper_after_first:
        return "brand"
    if alpha_only and len(text) >= 4 and text[0].isupper() and text[1:].islower():
        return "proper_noun"
    return "other"


def is_protected(term: VocabTerm) -> bool:
    if term.source in {"manual", "starred"}:
        return True
    if normalize(term.term) in COMMON_WORDS:
        return False
    return term.term_type in {"brand", "acronym", "proper_noun", "code_identifier"}


def edit_similarity(lhs: str, rhs: str) -> float:
    if lhs == rhs:
        return 1.0
    if not lhs or not rhs:
        return 0.0
    prev = list(range(len(rhs) + 1))
    curr = [0] * (len(rhs) + 1)
    for i, left in enumerate(lhs, start=1):
        curr[0] = i
        for j, right in enumerate(rhs, start=1):
            cost = 0 if left == right else 1
            curr[j] = min(curr[j - 1] + 1, prev[j] + 1, prev[j - 1] + cost)
        prev, curr = curr, prev
    return max(0.0, 1.0 - prev[len(rhs)] / max(len(lhs), len(rhs)))


def phonetic_key(text: str) -> str:
    cleaned = "".join(ch for ch in text.lower() if ch.isalpha())
    if not cleaned:
        return ""
    out: list[str] = []
    prev = ""
    for idx, ch in enumerate(cleaned):
        mapped = {
            "c": "k",
            "q": "k",
            "x": "k",
            "z": "s",
            "v": "f",
            "y": "i",
        }.get(ch, ch)
        if idx > 0 and mapped in "aeiou":
            continue
        if mapped != prev:
            out.append(mapped)
            prev = mapped
    return "".join(out)


def phonetic_similarity(lhs: str, rhs: str) -> float:
    return edit_similarity(phonetic_key(lhs), phonetic_key(rhs))


def deterministic_score(token: str, candidate: VocabTerm, alias_count: int) -> float:
    token_norm = normalize(token)
    cand_norm = normalize(candidate.term)
    base = max(edit_similarity(token_norm, cand_norm), phonetic_similarity(token_norm, cand_norm))
    if edit_similarity(token_norm, cand_norm) >= 0.70 and abs(len(token_norm) - len(cand_norm)) <= 2:
        base += 0.08
    if phonetic_similarity(token_norm, cand_norm) >= 0.65:
        base += 0.06
    if token_norm[:1] == cand_norm[:1]:
        base += 0.03
    type_boost = 0.07 if candidate.term_type in {"acronym", "code_identifier"} else 0.05
    source_boost = 0.07 if candidate.source == "starred" else 0.05 if candidate.source == "manual" else 0.0
    evidence = min(np.log1p(max(candidate.use_count, 0)) * 0.015, 0.06)
    alias_boost = min(alias_count * 0.012, 0.08)
    return float(min(1.0, base + type_boost + source_boost + evidence + alias_boost))


def features(token: str, candidate: VocabTerm, alias_count: int) -> list[float]:
    token_norm = normalize(token)
    cand_norm = normalize(candidate.term)
    lhs = max(len(token_norm), 1)
    rhs = max(len(cand_norm), 1)
    det = deterministic_score(token, candidate, alias_count)
    return [
        edit_similarity(token_norm, cand_norm),
        phonetic_similarity(token_norm, cand_norm),
        min(lhs, rhs) / max(lhs, rhs),
        1.0 if candidate.term_type == "acronym" else 0.0,
        1.0 if candidate.term_type == "brand" else 0.0,
        1.0 if candidate.term_type == "proper_noun" else 0.0,
        1.0 if candidate.term_type == "code_identifier" else 0.0,
        1.0 if candidate.source == "manual" else 0.0,
        1.0 if candidate.source == "starred" else 0.0,
        min(alias_count, 10) / 10.0,
        np.log1p(max(candidate.use_count, 0)) / 5.0,
        max(0.0, min(candidate.weight / 5.0, 1.0)),
        det,
        1.0 if is_dictionary_word(token) else 0.0,
    ]


ASR_CONFUSIONS: dict[str, list[str]] = {
    "b": ["p", "v"], "p": ["b", "f"], "d": ["t", "th"], "t": ["d", "th"],
    "g": ["k", "gh"], "k": ["g", "c", "q"], "v": ["f", "b", "w"], "f": ["v", "ph"],
    "s": ["z", "sh", "c"], "z": ["s", "j"], "sh": ["s", "ch"], "ch": ["sh", "j"],
    "j": ["z", "g", "ch"], "th": ["t", "d"], "ph": ["f", "p"],
    "c": ["k", "s"], "q": ["k"], "w": ["v"],
    "n": ["m", "ng"], "m": ["n"],
}


def variants(text: str) -> Iterable[str]:
    base = normalize(text)
    if len(base) < 3:
        return []
    vowels = "aeiou"
    out = {base}

    # Original generators
    for idx in range(len(base)):
        out.add(base[:idx] + base[idx + 1 :])
        if base[idx] in vowels:
            for vowel in vowels:
                out.add(base[:idx] + vowel + base[idx + 1 :])
    for idx in range(len(base) - 1):
        out.add(base[:idx] + base[idx + 1] + base[idx] + base[idx + 2 :])

    # ASR confusion substitutions (voiced↔unvoiced, aspirated, etc.)
    for idx in range(len(base)):
        ch = base[idx]
        for replacement in ASR_CONFUSIONS.get(ch, []):
            out.add(base[:idx] + replacement + base[idx + 1 :])
        # Digraph confusions: check 2-char windows
        if idx + 1 < len(base):
            digraph = base[idx : idx + 2]
            for replacement in ASR_CONFUSIONS.get(digraph, []):
                out.add(base[:idx] + replacement + base[idx + 2 :])

    # Gemination: double/undouble consonants
    for idx in range(len(base)):
        ch = base[idx]
        if ch not in vowels:
            out.add(base[:idx] + ch + ch + base[idx + 1 :])
            if idx + 1 < len(base) and base[idx + 1] == ch:
                out.add(base[:idx] + ch + base[idx + 2 :])

    # Schwa insertion/deletion between consonant clusters
    for idx in range(len(base) - 1):
        if base[idx] not in vowels and base[idx + 1] not in vowels:
            out.add(base[: idx + 1] + "a" + base[idx + 1 :])

    # Trailing vowel add/drop
    if base[-1] in vowels:
        out.add(base[:-1])
    else:
        for v in "aei":
            out.add(base + v)

    return [item for item in out if len(item) >= 3 and item != base]


def load_data(
    conn: sqlite3.Connection,
    user_id: str,
) -> tuple[list[VocabTerm], list[AliasRule], list[PolicyWeight]]:
    vocab_rows = conn.execute(
        """
        SELECT term, COALESCE(term_type, ''), source, weight, use_count
          FROM vocabulary
         WHERE user_id = ?
        """,
        (user_id,),
    ).fetchall()
    vocab = [
        VocabTerm(
            term=row[0],
            term_type=row[1] or classify_term_type(row[0]),
            source=row[2] or "auto",
            weight=float(row[3] or 1.0),
            use_count=int(row[4] or 0),
        )
        for row in vocab_rows
    ]

    rules_rows = conn.execute(
        """
        SELECT transcript_form, correct_form, weight, use_count,
               COALESCE(review_status, 'pending')
          FROM stt_replacements
         WHERE user_id = ?
        """,
        (user_id,),
    ).fetchall()
    rules = [
        AliasRule(
            transcript_form=row[0],
            correct_form=row[1],
            weight=float(row[2] or 1.0),
            use_count=int(row[3] or 0),
            review_status=row[4] or "pending",
        )
        for row in rules_rows
    ]
    policy_rows = []
    has_policy = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tier2_policy_weights'"
    ).fetchone()
    if has_policy:
        policy_rows = conn.execute(
            """
            SELECT token_norm, correct_form, correct_form_norm,
                   positive_count, negative_count, learned_weight
              FROM tier2_policy_weights
             WHERE user_id = ?
            """,
            (user_id,),
        ).fetchall()
    policy = [
        PolicyWeight(
            token_norm=row[0],
            correct_form=row[1],
            correct_form_norm=row[2],
            positive_count=int(row[3] or 0),
            negative_count=int(row[4] or 0),
            learned_weight=float(row[5] or 0.0),
        )
        for row in policy_rows
    ]
    return vocab, rules, policy


def build_examples(
    vocab: list[VocabTerm],
    rules: list[AliasRule],
    policy: list[PolicyWeight],
    *,
    cache_dir: Path | None = None,
    micro: bool = False,
) -> tuple[list[Example], dict[str, int]]:
    protected = [term for term in vocab if is_protected(term) and len(term.term.split()) == 1]
    by_norm = {normalize(term.term): term for term in protected}
    alias_count: dict[str, int] = {normalize(term.term): 0 for term in protected}
    positives: list[Example] = []
    negatives: list[Example] = []

    for rule in rules:
        if rule.review_status != "approved":
            continue
        token = normalize(rule.transcript_form)
        correct = normalize(rule.correct_form)
        if len(token) < 3 or correct not in by_norm:
            continue
        if is_dictionary_word(token) or is_common_alias_source(rule.transcript_form):
            # Dictionary words become strong negatives — teaches model not to replace them
            candidate = by_norm[correct]
            negatives.append(Example(token, candidate, 0.0))
            continue
        candidate = by_norm[correct]
        # G3: Cross-term collision — skip if alias is closer to a different vocab term
        collision = False
        token_sim = edit_similarity(token, correct)
        for other_norm, other_term in by_norm.items():
            if other_norm == correct:
                continue
            if edit_similarity(token, other_norm) > token_sim:
                collision = True
                break
        if collision:
            continue
        alias_count[correct] = alias_count.get(correct, 0) + 1
        for variant in variants(token):
            positives.append(Example(variant, candidate, 1.0))

    for weight in policy:
        token = normalize(weight.token_norm)
        correct = normalize(weight.correct_form_norm)
        if len(token) < 3 or correct not in by_norm:
            continue
        if is_dictionary_word(token) or is_common_alias_source(weight.token_norm):
            # Dictionary words become strong negatives — teaches model not to replace them
            candidate = by_norm[correct]
            negatives.append(Example(token, candidate, 0.0))
            continue
        candidate = by_norm[correct]
        # G3: Cross-term collision — skip if alias is closer to a different vocab term
        collision = False
        token_sim = edit_similarity(token, correct)
        for other_norm, other_term in by_norm.items():
            if other_norm == correct:
                continue
            if edit_similarity(token, other_norm) > token_sim:
                collision = True
                break
        if collision:
            continue
        net = weight.positive_count - int(weight.negative_count * 1.5)
        if net > 0 and weight.learned_weight > 0:
            alias_count[correct] = alias_count.get(correct, 0) + max(1, weight.positive_count)
            positives.append(Example(token, candidate, 1.0))
            for variant in list(variants(token))[:8]:
                positives.append(Example(variant, candidate, 1.0))
        elif weight.negative_count > 0:
            negatives.append(Example(token, candidate, 0.0))

    # Canonical near-spelling positives — expanded from 12 to 30 variants per term
    # with ASR confusion patterns for realistic STT distortions.
    for term in protected:
        term_norm = normalize(term.term)
        for variant in list(variants(term.term))[:30]:
            var_norm = normalize(variant)
            if var_norm == term_norm:
                continue
            if is_dictionary_word(variant):
                negatives.append(Example(variant, term, 0.0))
                continue
            sim = edit_similarity(var_norm, term_norm)
            if sim > 0.90 or sim < 0.30:
                continue
            positives.append(Example(variant, term, 1.0))

    # Common words → negatives for every candidate
    for word in COMMON_WORDS:
        for candidate in random.sample(protected, min(len(protected), 4)):
            negatives.append(Example(word, candidate, 0.0))

    # Cross-candidate negatives: each positive paired with 5 wrong candidates
    for positive in positives:
        alternatives = [term for term in protected if normalize(term.term) != normalize(positive.candidate.term)]
        for candidate in random.sample(alternatives, min(len(alternatives), 5)):
            negatives.append(Example(positive.token, candidate, 0.0))

    # Cross-term confusion negatives
    for lhs in protected:
        for rhs in protected:
            if lhs is rhs:
                continue
            if edit_similarity(normalize(lhs.term), normalize(rhs.term)) >= 0.55:
                negatives.append(Example(lhs.term, rhs, 0.0))

    # Length-mismatch negatives: short random tokens paired with long candidates.
    # Teaches the model that "ipo" (3 chars) should never match "Macobs" (6 chars).
    short_tokens = ["ipo", "api", "sdk", "url", "npm", "git", "dev", "ops",
                    "run", "pod", "log", "tab", "tag", "bug", "fix", "hub"]
    for tok in short_tokens:
        for candidate in protected:
            if len(normalize(candidate.term)) >= 5:
                negatives.append(Example(tok, candidate, 0.0))

    # Random substring negatives: take 3-4 char slices of candidate terms
    for term in protected:
        term_norm = normalize(term.term)
        if len(term_norm) < 5:
            continue
        for _ in range(5):
            start = random.randint(0, len(term_norm) - 3)
            length = random.randint(3, min(4, len(term_norm) - start))
            fragment = term_norm[start : start + length]
            if not is_dictionary_word(fragment):
                for other in random.sample(protected, min(len(protected), 3)):
                    if normalize(other.term) != term_norm:
                        negatives.append(Example(fragment, other, 0.0))

    neg_cache = cache_dir / "neg_cache.json" if cache_dir else None
    skip_neg_regen = False
    if micro and neg_cache and neg_cache.exists():
        cache_age = time.time() - neg_cache.stat().st_mtime
        if cache_age < 3600:  # 1 hour — reuse cached negatives in micro mode
            skip_neg_regen = True
    if skip_neg_regen:
        # Micro mode: load cached negatives without regenerating from dictionary
        by_norm = {normalize(t.term): t for t in protected}
        try:
            cached = json.loads(neg_cache.read_text())
            for pair in cached.get("pairs", []):
                cand = by_norm.get(pair["term"])
                if cand:
                    negatives.append(Example(pair["word"], cand, 0.0))
        except Exception:
            # Cache unreadable — fall back to full generation
            dictionary_negatives = mine_hard_negatives_from_dictionary(protected, neg_cache)
            negatives.extend(dictionary_negatives)
    else:
        dictionary_negatives = mine_hard_negatives_from_dictionary(protected, neg_cache)
        negatives.extend(dictionary_negatives)

    examples = positives + negatives
    random.shuffle(examples)
    return examples, alias_count


def build_char_vocab(examples: list[Example]) -> dict[str, int]:
    chars = {"<pad>": 0, "<unk>": 1}
    for example in examples:
        for text in (example.token, example.candidate.term):
            for ch in normalize(text):
                if ch not in chars:
                    chars[ch] = len(chars)
    return chars


def encode(text: str, char_to_id: dict[str, int]) -> list[int]:
    ids = [0] * MAX_LEN
    unknown = char_to_id["<unk>"]
    for idx, ch in enumerate(normalize(text)[:MAX_LEN]):
        ids[idx] = char_to_id.get(ch, unknown)
    return ids


def tensorize(
    examples: list[Example],
    char_to_id: dict[str, int],
    alias_count: dict[str, int],
) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor, torch.Tensor]:
    token_ids = []
    candidate_ids = []
    feature_rows = []
    labels = []
    for example in examples:
        candidate_key = normalize(example.candidate.term)
        token_ids.append(encode(example.token, char_to_id))
        candidate_ids.append(encode(example.candidate.term, char_to_id))
        feature_rows.append(features(example.token, example.candidate, alias_count.get(candidate_key, 0)))
        labels.append([example.label])
    return (
        torch.tensor(token_ids, dtype=torch.long),
        torch.tensor(candidate_ids, dtype=torch.long),
        torch.tensor(feature_rows, dtype=torch.float32),
        torch.tensor(labels, dtype=torch.float32),
    )


def fingerprint(vocab: list[VocabTerm], rules: list[AliasRule], policy: list[PolicyWeight]) -> str:
    payload = {
        "vocab": sorted([term.__dict__ for term in vocab], key=lambda row: row["term"].lower()),
        "rules": sorted([rule.__dict__ for rule in rules], key=lambda row: (row["correct_form"].lower(), row["transcript_form"].lower())),
        "policy": sorted([weight.__dict__ for weight in policy], key=lambda row: (row["correct_form_norm"], row["token_norm"])),
    }
    raw = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(raw).hexdigest()


def ensure_metadata_table(conn: sqlite3.Connection) -> None:
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS tier2_model_metadata (
            user_id           TEXT PRIMARY KEY,
            artifact_path     TEXT NOT NULL,
            vocab_index_path  TEXT NOT NULL,
            trained_at        INTEGER NOT NULL,
            alias_count       INTEGER NOT NULL DEFAULT 0,
            vocab_count       INTEGER NOT NULL DEFAULT 0,
            data_fingerprint  TEXT,
            metrics_json      TEXT,
            last_error        TEXT,
            updated_at        INTEGER NOT NULL
        )
        """
    )


def write_metadata(
    conn: sqlite3.Connection,
    user_id: str,
    artifact_path: Path,
    vocab_index_path: Path,
    trained_at: int,
    alias_count: int,
    vocab_count: int,
    data_fingerprint: str,
    metrics: dict[str, float],
) -> None:
    ensure_metadata_table(conn)
    metrics_json = json.dumps(metrics, sort_keys=True)
    conn.execute(
        """
        INSERT INTO tier2_model_metadata
             (user_id, artifact_path, vocab_index_path, trained_at,
              alias_count, vocab_count, data_fingerprint, metrics_json,
              last_error, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)
        ON CONFLICT(user_id) DO UPDATE SET
             artifact_path = excluded.artifact_path,
             vocab_index_path = excluded.vocab_index_path,
             trained_at = excluded.trained_at,
             alias_count = excluded.alias_count,
             vocab_count = excluded.vocab_count,
             data_fingerprint = excluded.data_fingerprint,
             metrics_json = excluded.metrics_json,
             last_error = NULL,
             updated_at = excluded.updated_at
        """,
        (
            user_id,
            str(artifact_path),
            str(vocab_index_path),
            trained_at,
            alias_count,
            vocab_count,
            data_fingerprint,
            metrics_json,
            trained_at,
        ),
    )
    conn.commit()


def train_xgboost(
    examples: list[Example],
    alias_count: dict[str, int],
    artifact_path: Path,
) -> dict[str, float]:
    """Train an XGBoost classifier on the 14 hand-crafted features."""
    import xgboost as xgb
    from onnxmltools import convert_xgboost
    from onnxmltools.convert.common.data_types import FloatTensorType

    X = np.array([
        features(ex.token, ex.candidate, alias_count.get(normalize(ex.candidate.term), 0))
        for ex in examples
    ], dtype=np.float32)
    y = np.array([ex.label for ex in examples], dtype=np.float32)

    pos_count = float(y.sum())
    neg_count = float(len(y) - pos_count)
    scale = neg_count / max(pos_count, 1.0)

    model = xgb.XGBClassifier(
        n_estimators=200,
        max_depth=4,
        learning_rate=0.1,
        min_child_weight=5,
        subsample=0.8,
        colsample_bytree=0.8,
        scale_pos_weight=scale,
        eval_metric="logloss",
        random_state=SEED,
        use_label_encoder=False,
    )
    model.fit(X, y, verbose=False)

    probs = model.predict_proba(X)[:, 1]
    pred_labels = (probs >= 0.5).astype(float)
    accuracy = float((pred_labels == y).mean())
    avg_positive = float(probs[y == 1].mean()) if pos_count > 0 else 0.0
    avg_negative = float(probs[y == 0].mean()) if neg_count > 0 else 0.0

    initial_type = [("features", FloatTensorType([None, len(FEATURE_NAMES)]))]
    onnx_model = convert_xgboost(model, initial_types=initial_type, target_opset=15)
    with open(artifact_path, "wb") as f:
        f.write(onnx_model.SerializeToString())

    print(f"  XGBoost: acc={accuracy:.4f} avg_pos={avg_positive:.3f} avg_neg={avg_negative:.3f}")
    return {
        "train_accuracy": accuracy,
        "avg_positive_score": avg_positive,
        "avg_negative_score": avg_negative,
    }


def focal_bce(pred: torch.Tensor, target: torch.Tensor, gamma: float = 2.0, alpha: float = 0.25) -> torch.Tensor:
    bce = torch.nn.functional.binary_cross_entropy(pred, target, reduction="none")
    pt = target * pred + (1 - target) * (1 - pred)
    return (alpha * (1 - pt) ** gamma * bce).mean()


def train_mlp(
    examples: list[Example],
    alias_count: dict[str, int],
    artifact_path: Path,
    epochs: int,
    batch_size: int,
    lr: float,
) -> tuple[dict[str, int], dict[str, float]]:
    """Train the MLP with focal loss, label smoothing, deterministic gate, and early stopping."""
    char_to_id = build_char_vocab(examples)
    token_ids, candidate_ids, feature_rows, labels = tensorize(examples, char_to_id, alias_count)

    # 80/20 train/val split for early stopping
    n = len(examples)
    indices = list(range(n))
    random.shuffle(indices)
    val_size = max(int(n * 0.2), 1)
    val_idx = indices[:val_size]
    train_idx = indices[val_size:]

    train_dataset = TensorDataset(
        token_ids[train_idx], candidate_ids[train_idx],
        feature_rows[train_idx], labels[train_idx],
    )
    val_tok, val_cand = token_ids[val_idx], candidate_ids[val_idx]
    val_feat, val_lbl = feature_rows[val_idx], labels[val_idx]
    loader = DataLoader(train_dataset, batch_size=min(batch_size, len(train_dataset)), shuffle=True)

    model = TinyCorrectionModel(len(char_to_id), len(FEATURE_NAMES))
    optimizer = torch.optim.AdamW(model.parameters(), lr=lr, weight_decay=0.01)

    best_val_loss = float("inf")
    patience = 0
    best_state = None

    model.train()
    for epoch in range(epochs):
        for batch_token, batch_candidate, batch_features, batch_labels in loader:
            optimizer.zero_grad()
            pred = model(batch_token, batch_candidate, batch_features)
            smoothed = batch_labels * 0.9 + 0.05
            loss = focal_bce(pred, smoothed)
            loss.backward()
            optimizer.step()

        # Validation check for early stopping
        model.eval()
        with torch.no_grad():
            val_pred = model(val_tok, val_cand, val_feat)
            val_loss = float(focal_bce(val_pred, val_lbl * 0.9 + 0.05).item())
        model.train()

        if val_loss < best_val_loss:
            best_val_loss = val_loss
            patience = 0
            best_state = {k: v.clone() for k, v in model.state_dict().items()}
        else:
            patience += 1

        if patience >= 10 and epoch >= 20:
            print(f"  Early stop at epoch {epoch + 1} (val_loss={val_loss:.4f}, best={best_val_loss:.4f})")
            break

    if best_state is not None:
        model.load_state_dict(best_state)

    model.eval()
    with torch.no_grad():
        predictions = model(token_ids, candidate_ids, feature_rows)
        pred_labels = (predictions >= 0.5).float()
        accuracy = float((pred_labels == labels).float().mean().item())
        avg_positive = float(predictions[labels.squeeze(1) == 1].mean().item())
        avg_negative = float(predictions[labels.squeeze(1) == 0].mean().item())
        max_negative = float(predictions[labels.squeeze(1) == 0].max().item())

    dummy_token = torch.zeros((1, MAX_LEN), dtype=torch.long)
    dummy_candidate = torch.zeros((1, MAX_LEN), dtype=torch.long)
    dummy_features = torch.zeros((1, len(FEATURE_NAMES)), dtype=torch.float32)
    torch.onnx.export(
        model,
        (dummy_token, dummy_candidate, dummy_features),
        artifact_path,
        input_names=["token_ids", "candidate_ids", "features"],
        output_names=["score"],
        dynamic_axes={
            "token_ids": {0: "batch"},
            "candidate_ids": {0: "batch"},
            "features": {0: "batch"},
            "score": {0: "batch"},
        },
        opset_version=17,
    )
    metrics = {
        "train_accuracy": accuracy,
        "avg_positive_score": avg_positive,
        "avg_negative_score": avg_negative,
        "max_negative_score": max_negative,
    }
    print(f"  MLP: acc={accuracy:.4f} avg_pos={avg_positive:.3f} avg_neg={avg_negative:.3f}")
    return char_to_id, metrics


def train(args: argparse.Namespace) -> None:
    random.seed(SEED)
    np.random.seed(SEED)

    db_path = Path(args.db).expanduser().resolve()
    conn = sqlite3.connect(db_path)
    vocab, rules, policy = load_data(conn, args.user_id)

    out_dir = Path(args.out_dir).expanduser().resolve() if args.out_dir else db_path.parent / "tier2" / args.user_id
    out_dir.mkdir(parents=True, exist_ok=True)

    examples, alias_count = build_examples(vocab, rules, policy, cache_dir=out_dir, micro=args.micro)
    if len(examples) < 20 or not any(example.label == 1.0 for example in examples):
        raise SystemExit("not enough Tier 2 examples; add more approved local corrections first")
    artifact_path = out_dir / "correction_model.onnx"
    vocab_index_path = out_dir / "vocab_index.json"
    metadata_path = out_dir / "model_metadata.json"

    use_xgboost = args.xgboost
    mode = "xgboost" if use_xgboost else "mlp"
    pos = sum(1 for e in examples if e.label > 0.5)
    neg = len(examples) - pos
    print(f"Training: {mode} — {len(examples)} examples ({pos} pos / {neg} neg)")

    char_to_id: dict[str, int] = {}
    if use_xgboost:
        metrics = train_xgboost(examples, alias_count, artifact_path)
    else:
        torch.manual_seed(SEED)
        epochs = 15 if args.micro else (40 if args.incremental else args.epochs)
        char_to_id, metrics = train_mlp(examples, alias_count, artifact_path, epochs, args.batch_size, args.lr)

    protected_vocab = [term for term in vocab if is_protected(term) and len(term.term.split()) == 1]
    vocab_index: dict = {
        "version": 2,
        "model_type": mode,
        "feature_names": FEATURE_NAMES,
        "candidates": [
            {
                "term": term.term,
                "term_type": term.term_type,
                "source": term.source,
                "weight": term.weight,
                "use_count": term.use_count,
                "alias_count": alias_count.get(normalize(term.term), 0),
            }
            for term in protected_vocab
        ],
    }
    if not use_xgboost:
        vocab_index["max_len"] = MAX_LEN
        vocab_index["char_to_id"] = char_to_id

    vocab_index_path.write_text(json.dumps(vocab_index, indent=2, sort_keys=True), encoding="utf-8")

    trained_at = int(time.time() * 1000)
    data_fingerprint = fingerprint(vocab, rules, policy)
    metrics["example_count"] = float(len(examples))
    metrics["positive_count"] = float(pos)
    metrics["negative_count"] = float(neg)
    metadata = {
        "user_id": args.user_id,
        "artifact_path": str(artifact_path),
        "vocab_index_path": str(vocab_index_path),
        "trained_at": trained_at,
        "alias_count": sum(alias_count.values()),
        "vocab_count": len(protected_vocab),
        "data_fingerprint": data_fingerprint,
        "metrics_json": json.dumps(metrics, sort_keys=True),
        "last_error": None,
    }
    metadata_path.write_text(json.dumps(metadata, indent=2, sort_keys=True), encoding="utf-8")

    if not args.no_db_metadata:
        write_metadata(
            conn,
            args.user_id,
            artifact_path,
            vocab_index_path,
            trained_at,
            sum(alias_count.values()),
            len(protected_vocab),
            data_fingerprint,
            metrics,
        )

    print(json.dumps({"artifact": str(artifact_path), "vocab_index": str(vocab_index_path), "metrics": metrics}, indent=2))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", required=True, help="Path to Said's local SQLite database")
    parser.add_argument("--user-id", required=True, help="local_user.id to train for")
    parser.add_argument("--out-dir", help="Artifact directory; defaults to <db_dir>/tier2/<user-id>")
    parser.add_argument("--epochs", type=int, default=120)
    parser.add_argument("--batch-size", type=int, default=64)
    parser.add_argument("--lr", type=float, default=2e-3)
    parser.add_argument("--no-db-metadata", action="store_true", help="Do not upsert tier2_model_metadata")
    parser.add_argument("--incremental", action="store_true", help="Fast retrain: 80 epochs, uses cached negatives")
    parser.add_argument("--micro", action="store_true",
                        help="Ultra-fast retrain for single-term additions (15 epochs)")
    parser.add_argument("--xgboost", action="store_true",
                        help="Use XGBoost instead of MLP neural model (default: MLP)")
    return parser.parse_args()


if __name__ == "__main__":
    train(parse_args())
