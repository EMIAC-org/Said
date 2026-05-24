#!/usr/bin/env python3
"""End-to-end eval for ONNX model activation.

Simulates the real learning journey:
1. Seed vocab terms (fresh — no aliases yet)
2. User corrects each term 1-2 times → creates aliases
3. Train the model on this LIMITED data
4. Evaluate on a comprehensive test battery:
   - Same distortions (model saw these)
   - New distortions (model must generalize)
   - Common English words (must NOT correct)
   - Common Hindi words (must NOT correct)
   - Cross-term confusion (must pick the right one)
   - Edge cases (short, numeric, identical, etc.)
"""

from __future__ import annotations

import json
import random
import sys
import textwrap
import time
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np
import torch

# Reuse everything from the training script
sys.path.insert(0, str(Path(__file__).parent))
from train_correction_model import (
    COMMON_WORDS,
    MAX_LEN,
    SEED,
    Example,
    TinyCorrectionModel,
    VocabTerm,
    AliasRule,
    PolicyWeight,
    build_char_vocab,
    build_examples,
    classify_term_type,
    deterministic_score,
    edit_similarity,
    encode,
    features,
    is_dictionary_word,
    is_protected,
    load_dictionary,
    normalize,
    phonetic_similarity,
    tensorize,
)

SCORE_THRESHOLD = 0.86
MARGIN_THRESHOLD = 0.18

# ── Dev terms: the vocabulary Said should learn ──────────────────────────────

DEV_VOCAB: list[VocabTerm] = [
    VocabTerm("AirNote", "brand", "auto", 1.5, 3),
    VocabTerm("EMIAC", "acronym", "manual", 2.0, 8),
    VocabTerm("Macobs", "proper_noun", "auto", 1.2, 5),
    VocabTerm("Kubernetes", "brand", "auto", 1.0, 2),
    VocabTerm("Semrush", "brand", "auto", 1.0, 1),
    VocabTerm("ChatGPT", "brand", "starred", 3.0, 12),
    VocabTerm("Grafana", "brand", "auto", 1.0, 2),
    VocabTerm("Terraform", "brand", "auto", 1.0, 1),
    VocabTerm("Datadog", "brand", "auto", 1.0, 1),
    VocabTerm("n8n", "code_identifier", "manual", 2.0, 4),
    VocabTerm("Vipassana", "proper_noun", "auto", 1.0, 1),
    VocabTerm("Abhishek", "proper_noun", "manual", 2.0, 6),
]

# Aliases: simulating 1-2 user corrections per term.
# These are what Deepgram actually outputs and the user corrected.
DEV_ALIASES: list[AliasRule] = [
    # AirNote — user corrected "air note" (2 corrections)
    AliasRule("airnote", "AirNote", 1.0, 2, "approved"),
    AliasRule("airnot", "AirNote", 1.0, 1, "approved"),
    # EMIAC — user corrected "amiac" and "emiac" (2 corrections)
    AliasRule("amiac", "EMIAC", 1.0, 3, "approved"),
    AliasRule("emiak", "EMIAC", 1.0, 1, "approved"),
    # Macobs — user corrected "macops" (1 correction)
    AliasRule("macops", "Macobs", 1.0, 2, "approved"),
    # Kubernetes — user corrected "cubinatis" (1 correction)
    AliasRule("cubinatis", "Kubernetes", 1.0, 1, "approved"),
    # Semrush — user corrected "sem rush" (1 correction)
    AliasRule("semrash", "Semrush", 1.0, 1, "approved"),
    # ChatGPT — user corrected "chat gpt" (1 correction)
    AliasRule("chatgpt", "ChatGPT", 1.0, 5, "approved"),
    AliasRule("chatjpt", "ChatGPT", 1.0, 1, "approved"),
    # Grafana — user corrected "grafanna" (1 correction)
    AliasRule("grafanna", "Grafana", 1.0, 1, "approved"),
    # Terraform — user corrected "teraform" (1 correction)
    AliasRule("teraform", "Terraform", 1.0, 1, "approved"),
    # Datadog — user corrected "data dog" (1 correction)
    AliasRule("datadag", "Datadog", 1.0, 1, "approved"),
    # n8n — user corrected "n a n" (1 correction)
    AliasRule("nan", "n8n", 0.8, 1, "approved"),
    # Vipassana — user corrected (1 correction)
    AliasRule("vipasana", "Vipassana", 1.0, 1, "approved"),
    # Abhishek — user corrected (2 corrections)
    AliasRule("abhishak", "Abhishek", 1.0, 2, "approved"),
    AliasRule("abishek", "Abhishek", 1.0, 1, "approved"),
]


# ── Test battery ─────────────────────────────────────────────────────────────

@dataclass
class TestCase:
    token: str
    expected_term: str | None  # None = should NOT correct
    category: str
    description: str


TEST_BATTERY: list[TestCase] = [
    # ── T1: Same distortions (model trained on these) ────────────────────────
    # Note: "airnote" is a case-insensitive near-identity of "AirNote" — the ONNX
    # scorer skips it (candidate.term_lower == token_norm). In production, Pass 1
    # (exact alias) catches it before ONNX ever runs. So the correct ONNX-only
    # expectation is: not applied.
    TestCase("airnote", None, "same_distortion", "near-identity, Pass 1 handles this"),
    TestCase("airnot", "AirNote", "same_distortion", "exact alias"),
    TestCase("amiac", "EMIAC", "same_distortion", "exact alias"),
    TestCase("emiak", "EMIAC", "same_distortion", "exact alias"),
    TestCase("macops", "Macobs", "same_distortion", "exact alias"),
    TestCase("cubinatis", "Kubernetes", "same_distortion", "exact alias"),
    TestCase("chatjpt", "ChatGPT", "same_distortion", "exact alias"),
    TestCase("abhishak", "Abhishek", "same_distortion", "exact alias"),

    # ── T2: New distortions (model must generalize) ──────────────────────────
    TestCase("airknot", "AirNote", "new_distortion", "phonetic variant"),
    TestCase("airnoit", "AirNote", "new_distortion", "vowel swap"),
    TestCase("arrnote", "AirNote", "new_distortion", "consonant double"),
    TestCase("emiag", "EMIAC", "new_distortion", "trailing consonant swap"),
    TestCase("ameac", "EMIAC", "new_distortion", "vowel shift"),
    TestCase("mecobs", "Macobs", "new_distortion", "vowel swap"),
    TestCase("macobs", None, "identity", "case-insensitive identity of Macobs"),
    TestCase("makobs", "Macobs", "new_distortion", "consonant swap"),
    TestCase("cubernetis", "Kubernetes", "new_distortion", "common Deepgram garble"),
    TestCase("kubernetis", "Kubernetes", "new_distortion", "k→c swap"),
    TestCase("chatgbt", "ChatGPT", "new_distortion", "p→b swap"),
    TestCase("chatjpd", "ChatGPT", "new_distortion", "consonant softening"),
    TestCase("grafena", "Grafana", "new_distortion", "vowel swap"),
    TestCase("terafoam", "Terraform", "new_distortion", "rm→m vowel"),
    TestCase("semrush", None, "identity", "case-insensitive identity of Semrush"),
    TestCase("vipashana", "Vipassana", "new_distortion", "aspirated variant"),
    TestCase("abisek", "Abhishek", "new_distortion", "h-drop + k"),
    TestCase("abhisek", "Abhishek", "new_distortion", "h-drop"),
    TestCase("datadig", "Datadog", "new_distortion", "vowel swap"),

    # ── T3: Common English words — MUST NOT correct ──────────────────────────
    TestCase("air", None, "english_common", "component of AirNote"),
    TestCase("note", None, "english_common", "component of AirNote"),
    TestCase("sprint", None, "english_common", "sounds like a brand"),
    TestCase("dock", None, "english_common", "sounds like Docker"),
    TestCase("graph", None, "english_common", "close to Grafana"),
    TestCase("tower", None, "english_common", "real word"),
    TestCase("react", None, "english_common", "real word, also tech brand"),
    TestCase("meaning", None, "english_common", "real word"),
    TestCase("return", None, "english_common", "real word"),
    TestCase("prayer", None, "english_common", "sounds vaguely like a brand"),
    TestCase("corps", None, "english_common", "close to Macobs phonetically?"),
    TestCase("nest", None, "english_common", "real word"),
    TestCase("spring", None, "english_common", "real word + tech brand"),
    TestCase("terraform", None, "english_common", "lowercase of vocab (identity)"),

    # ── T4: Common Hindi words — MUST NOT correct ────────────────────────────
    TestCase("bol", None, "hindi_common", "Hindi verb: speak (close to Bolt?)"),
    TestCase("sun", None, "hindi_common", "Hindi verb: listen"),
    TestCase("kaam", None, "hindi_common", "Hindi noun: work"),
    TestCase("din", None, "hindi_common", "Hindi noun: day"),
    TestCase("ghar", None, "hindi_common", "Hindi noun: home"),
    TestCase("raat", None, "hindi_common", "Hindi noun: night"),
    TestCase("banda", None, "hindi_common", "Hindi noun: person"),
    TestCase("baat", None, "hindi_common", "Hindi noun: talk"),
    TestCase("jagah", None, "hindi_common", "Hindi noun: place"),
    TestCase("mein", None, "hindi_common", "Hindi: in/I (appears every sentence)"),
    TestCase("kya", None, "hindi_common", "Hindi: what"),
    TestCase("dekh", None, "hindi_common", "Hindi verb: see"),
    TestCase("chal", None, "hindi_common", "Hindi verb: go"),
    TestCase("abhi", None, "hindi_common", "Hindi: now (close to Abhishek!)"),

    # ── T5: Cross-term confusion — must pick the RIGHT one ───────────────────
    TestCase("emiacs", "EMIAC", "cross_term", "EMIAC not Macobs"),
    TestCase("mecabs", "Macobs", "cross_term", "Macobs not EMIAC"),

    # ── T6: Edge cases ───────────────────────────────────────────────────────
    TestCase("AirNote", None, "identity", "the term itself — no correction"),
    TestCase("EMIAC", None, "identity", "the term itself — no correction"),
    TestCase("Macobs", None, "identity", "the term itself — no correction"),
    TestCase("ab", None, "too_short", "< 3 chars"),
    TestCase("12345", None, "numeric", "pure digits"),
    TestCase("xyzqwplm", None, "random_gibberish", "no candidate should match"),
    TestCase("brlmnsk", None, "random_gibberish", "consonant soup"),
]


# ── Scoring engine (mirrors tier2.rs logic) ──────────────────────────────────

@dataclass
class ScoringResult:
    token: str
    best_term: str | None
    best_score: float
    second_term: str | None
    second_score: float
    margin: float
    applied: bool
    det_score: float
    onnx_score: float
    kind: str  # "onnx", "deterministic", "none"


def score_token(
    token: str,
    candidates: list[VocabTerm],
    model: TinyCorrectionModel,
    char_to_id: dict[str, int],
    alias_count: dict[str, int],
) -> ScoringResult:
    token_norm = normalize(token)
    if not token_norm or len(token_norm) < 3:
        return ScoringResult(token, None, 0, None, 0, 0, False, 0, 0, "none")

    # Skip dictionary words (mirrors is_suspicious_token with is_in_dictionary)
    if is_dictionary_word(token_norm):
        return ScoringResult(token, None, 0, None, 0, 0, False, 0, 0, "none")

    scored = []
    for cand in candidates:
        cand_norm = normalize(cand.term)
        if cand_norm == token_norm:
            continue
        det = deterministic_score(token, cand, alias_count.get(cand_norm, 0))
        # ONNX scoring
        token_ids = torch.tensor([encode(token, char_to_id)], dtype=torch.long)
        cand_ids = torch.tensor([encode(cand.term, char_to_id)], dtype=torch.long)
        feat = features(token, cand, alias_count.get(cand_norm, 0))
        feat_tensor = torch.tensor([feat], dtype=torch.float32)
        with torch.no_grad():
            onnx = model(token_ids, cand_ids, feat_tensor).item()

        blended = max(det, onnx * 0.65 + det * 0.35)
        kind = "onnx" if blended > det else "deterministic"
        scored.append((cand, blended, det, onnx, kind))

    if not scored:
        return ScoringResult(token, None, 0, None, 0, 0, False, 0, 0, "none")

    scored.sort(key=lambda x: x[1], reverse=True)
    best = scored[0]
    second = scored[1] if len(scored) > 1 else None
    second_score = second[1] if second else 0.0
    margin = best[1] - second_score
    applied = best[1] >= SCORE_THRESHOLD and margin >= MARGIN_THRESHOLD

    return ScoringResult(
        token=token,
        best_term=best[0].term,
        best_score=best[1],
        second_term=second[0].term if second else None,
        second_score=second_score,
        margin=margin,
        applied=applied,
        det_score=best[2],
        onnx_score=best[3],
        kind=best[4],
    )


# ── Training ─────────────────────────────────────────────────────────────────

def train_model(
    vocab: list[VocabTerm],
    aliases: list[AliasRule],
    epochs: int = 60,
) -> tuple[TinyCorrectionModel, dict[str, int], dict[str, int]]:
    random.seed(SEED)
    np.random.seed(SEED)
    torch.manual_seed(SEED)

    examples, alias_count = build_examples(vocab, aliases, [])
    pos = sum(1 for e in examples if e.label > 0.5)
    neg = len(examples) - pos
    print(f"  Training: {len(examples)} examples ({pos} pos / {neg} neg)")

    char_to_id = build_char_vocab(examples)
    token_ids, candidate_ids, feature_rows, labels = tensorize(
        examples, char_to_id, alias_count
    )

    feature_count = feature_rows.shape[1]
    model = TinyCorrectionModel(len(char_to_id), feature_count)
    optimizer = torch.optim.Adam(model.parameters(), lr=1e-3, weight_decay=1e-5)
    loss_fn = torch.nn.BCELoss()

    dataset = torch.utils.data.TensorDataset(token_ids, candidate_ids, feature_rows, labels)
    loader = torch.utils.data.DataLoader(dataset, batch_size=64, shuffle=True)

    model.train()
    for epoch in range(epochs):
        total_loss = 0.0
        for batch_tok, batch_cand, batch_feat, batch_lbl in loader:
            optimizer.zero_grad()
            out = model(batch_tok, batch_cand, batch_feat)
            loss = loss_fn(out, batch_lbl)
            loss.backward()
            optimizer.step()
            total_loss += loss.item()
        if (epoch + 1) % 20 == 0:
            avg = total_loss / len(loader)
            print(f"  Epoch {epoch+1}/{epochs} — loss={avg:.4f}")

    model.eval()
    return model, char_to_id, alias_count


# ── Evaluation ───────────────────────────────────────────────────────────────

# ANSI colors
G = "\033[92m"  # green
R = "\033[91m"  # red
Y = "\033[93m"  # yellow
D = "\033[90m"  # dim
B = "\033[1m"   # bold
E = "\033[0m"   # reset


def run_eval():
    print(f"\n{B}═══ ONNX Model Activation — End-to-End Eval ═══{E}\n")

    # Step 1: Train
    print(f"{B}Phase 1: Training on {len(DEV_VOCAB)} terms, {len(DEV_ALIASES)} aliases (1-2 corrections each){E}")
    t0 = time.time()
    model, char_to_id, alias_count = train_model(DEV_VOCAB, DEV_ALIASES)
    elapsed = time.time() - t0
    print(f"  Trained in {elapsed:.1f}s\n")

    # Step 2: Build candidate list (mirrors build_candidates in tier2.rs)
    candidates = [v for v in DEV_VOCAB if is_protected(v) and len(v.term.split()) == 1]
    print(f"{B}Phase 2: Evaluating {len(TEST_BATTERY)} test cases against {len(candidates)} candidates{E}\n")

    # Step 3: Run test battery
    results_by_category: dict[str, list[tuple[TestCase, ScoringResult, bool]]] = {}
    total_pass = 0
    total_fail = 0

    for tc in TEST_BATTERY:
        result = score_token(tc.token, candidates, model, char_to_id, alias_count)

        if tc.expected_term is None:
            passed = not result.applied
        else:
            passed = result.applied and result.best_term == tc.expected_term

        if passed:
            total_pass += 1
        else:
            total_fail += 1

        cat = tc.category
        if cat not in results_by_category:
            results_by_category[cat] = []
        results_by_category[cat].append((tc, result, passed))

    # Step 4: Print results by category
    CATEGORY_LABELS = {
        "same_distortion": "T1: Same Distortions (trained aliases → SHOULD correct)",
        "new_distortion": "T2: New Distortions (unseen variants → SHOULD generalize)",
        "english_common": "T3: Common English Words → MUST NOT correct",
        "hindi_common": "T4: Common Hindi Words → MUST NOT correct",
        "cross_term": "T5: Cross-Term Confusion → must pick RIGHT term",
        "identity": "T6a: Identity (token = term) → MUST NOT correct",
        "too_short": "T6b: Too Short (< 3 chars) → MUST skip",
        "numeric": "T6c: Numeric → MUST skip",
        "random_gibberish": "T6d: Random Gibberish → SHOULD NOT match",
    }

    for cat, label in CATEGORY_LABELS.items():
        if cat not in results_by_category:
            continue
        items = results_by_category[cat]
        cat_pass = sum(1 for _, _, p in items if p)
        cat_total = len(items)
        status = f"{G}ALL PASS{E}" if cat_pass == cat_total else f"{R}{cat_total - cat_pass} FAIL{E}"
        print(f"  {B}{label}{E}  [{status}]")
        print(f"  {'─' * 100}")

        for tc, result, passed in items:
            icon = f"{G}✓{E}" if passed else f"{R}✗{E}"
            if result.kind == "none":
                score_str = f"{D}(filtered out){E}"
            else:
                scorer = f"[{result.kind[:3]}]"
                score_str = f"score={result.best_score:.3f} margin={result.margin:.3f} {scorer}"
                if result.best_term:
                    score_str += f" → {result.best_term}"
                    if result.second_term:
                        score_str += f" {D}(2nd: {result.second_term} @{result.second_score:.3f}){E}"

            exp = tc.expected_term or "NO correction"
            print(f"  {icon} {tc.token:20s} expect={exp:14s} {score_str}  {D}({tc.description}){E}")

        print()

    # Step 5: Summary
    total = total_pass + total_fail
    pct = (total_pass / total * 100) if total > 0 else 0
    bar_len = 40
    filled = int(bar_len * total_pass / total) if total > 0 else 0
    bar = f"{G}{'█' * filled}{E}{D}{'░' * (bar_len - filled)}{E}"

    print(f"  {B}═══ SUMMARY ═══{E}")
    print(f"  {bar}  {total_pass}/{total} ({pct:.0f}%)")
    if total_fail > 0:
        print(f"  {R}{total_fail} test(s) failed — review the results above{E}")
    else:
        print(f"  {G}All tests passed! Model is ready for activation.{E}")

    # Step 6: Safety audit
    print(f"\n  {B}Safety Audit:{E}")
    load_dictionary()
    common_en_safe = all(
        not score_token(w, candidates, model, char_to_id, alias_count).applied
        for w in ["air", "note", "dock", "graph", "sprint", "nest", "spring",
                   "tower", "guard", "press", "base", "react", "swift", "rust"]
    )
    common_hi_safe = all(
        not score_token(w, candidates, model, char_to_id, alias_count).applied
        for w in ["bol", "sun", "kaam", "din", "ghar", "raat", "banda", "baat",
                   "mein", "kya", "dekh", "chal", "abhi", "mil", "rakh",
                   "waqt", "baar", "jagah", "cheez", "log"]
    )
    identity_safe = all(
        not score_token(normalize(v.term), candidates, model, char_to_id, alias_count).applied
        for v in DEV_VOCAB
    )
    print(f"  {'✓' if common_en_safe else '✗'} English common words: {'SAFE' if common_en_safe else 'UNSAFE'}")
    print(f"  {'✓' if common_hi_safe else '✗'} Hindi common words:   {'SAFE' if common_hi_safe else 'UNSAFE'}")
    print(f"  {'✓' if identity_safe else '✗'} Identity (self→self):  {'SAFE' if identity_safe else 'UNSAFE'}")

    print()
    return total_fail == 0


if __name__ == "__main__":
    success = run_eval()
    sys.exit(0 if success else 1)
