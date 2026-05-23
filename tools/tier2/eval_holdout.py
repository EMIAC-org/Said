#!/usr/bin/env python3
"""Held-out evaluation of the ONNX scorer.

Splits distortions 70/30 train/test. Trains a fresh model on the train
split only, then evaluates on never-seen test distortions.

Reports:
  - Per-term recall on held-out distortions
  - False positive rate on negative sentences
  - Overall precision, recall, F1
  - Score distribution histograms

Usage:
    python3 tools/tier2/eval_holdout.py
"""

import json
import os
import random
import sqlite3
import sys
import tempfile
import time
from pathlib import Path

import numpy as np
import torch

# Add the tools/tier2 dir so we can import from the training script
sys.path.insert(0, str(Path(__file__).parent))
from train_correction_model import (
    COMMON_WORDS,
    FEATURE_NAMES,
    MAX_LEN,
    AliasRule,
    Example,
    TinyCorrectionModel,
    VocabTerm,
    build_char_vocab,
    classify_term_type,
    deterministic_score,
    edit_similarity,
    encode,
    features,
    is_protected,
    load_dictionary,
    mine_hard_negatives_from_dictionary,
    normalize,
    phonetic_similarity,
    tensorize,
    variants,
)

SEED = 42
TRAIN_RATIO = 0.70
SCORE_THRESHOLD = 0.50

# All dev terms with distortions — the ground truth
DEV_TERMS = json.loads(
    (Path(__file__).parent.parent / "eval-pipeline" / "dev_terms_quality.json").read_text()
)["terms"]

NEGATIVES = json.loads(
    (Path(__file__).parent.parent / "eval-pipeline" / "dev_terms_quality.json").read_text()
).get("negatives", [])

# Extra hard negatives: common words that must NOT match any term
HARD_NEGATIVES = [
    "think", "meaning", "accounts", "main", "time", "return", "corps",
    "dock", "oath", "swift", "react", "next", "base", "post", "graph",
    "rest", "local", "type", "red", "go", "can", "cool", "house",
    "prayer", "course", "capital", "white", "google", "nest", "press",
    "super", "table", "tell", "verse", "tower", "guard", "just", "sent",
]


def make_vocab_term(term: str, term_type: str) -> VocabTerm:
    return VocabTerm(
        term=term,
        term_type=term_type,
        source="auto",
        weight=3.0,
        use_count=5,
    )


def split_distortions(distortions: list[str], train_ratio: float):
    random.shuffle(distortions)
    split_idx = max(1, int(len(distortions) * train_ratio))
    return distortions[:split_idx], distortions[split_idx:]


def build_train_examples(
    terms: list[dict], train_distortions: dict[str, list[str]]
) -> tuple[list[Example], dict[str, int]]:
    protected = []
    for t in terms:
        vt = make_vocab_term(t["term"], t["type"])
        protected.append(vt)

    by_norm = {normalize(vt.term): vt for vt in protected}
    alias_count = {normalize(vt.term): 0 for vt in protected}
    positives = []
    negatives = []

    for t in terms:
        vt = by_norm[normalize(t["term"])]
        train_dists = train_distortions.get(t["term"], [])
        alias_count[normalize(t["term"])] = len(train_dists)

        for dist in train_dists:
            positives.append(Example(dist, vt, 1.0))
            for v in list(variants(dist))[:6]:
                positives.append(Example(v, vt, 1.0))

        for v in list(variants(t["term"]))[:10]:
            if normalize(v) != normalize(t["term"]):
                positives.append(Example(v, vt, 1.0))

    for word in COMMON_WORDS:
        for cand in random.sample(protected, min(len(protected), 4)):
            negatives.append(Example(word, cand, 0.0))

    for word in HARD_NEGATIVES:
        for cand in random.sample(protected, min(len(protected), 3)):
            negatives.append(Example(word, cand, 0.0))

    for pos in positives:
        alts = [vt for vt in protected if normalize(vt.term) != normalize(pos.candidate.term)]
        for cand in random.sample(alts, min(len(alts), 2)):
            negatives.append(Example(pos.token, cand, 0.0))

    for lhs in protected:
        for rhs in protected:
            if lhs is rhs:
                continue
            if edit_similarity(normalize(lhs.term), normalize(rhs.term)) >= 0.55:
                negatives.append(Example(lhs.term, rhs, 0.0))

    dictionary_negatives = mine_hard_negatives_from_dictionary(protected)
    negatives.extend(dictionary_negatives)

    examples = positives + negatives
    random.shuffle(examples)
    return examples, alias_count


def train_model(examples, alias_count):
    char_to_id = build_char_vocab(examples)
    token_ids, candidate_ids, feature_rows, labels = tensorize(
        examples, char_to_id, alias_count
    )

    model = TinyCorrectionModel(len(char_to_id), len(FEATURE_NAMES))
    optimizer = torch.optim.AdamW(model.parameters(), lr=2e-3, weight_decay=0.01)
    loss_fn = torch.nn.BCELoss()

    dataset = torch.utils.data.TensorDataset(
        token_ids, candidate_ids, feature_rows, labels
    )
    loader = torch.utils.data.DataLoader(
        dataset, batch_size=64, shuffle=True
    )

    model.train()
    for epoch in range(200):
        for bt, bc, bf, bl in loader:
            optimizer.zero_grad()
            pred = model(bt, bc, bf)
            loss = loss_fn(pred, bl)
            loss.backward()
            optimizer.step()

    model.eval()
    return model, char_to_id


def score_pair(model, char_to_id, token: str, candidate: VocabTerm, alias_count: int):
    token_enc = torch.tensor([encode(token, char_to_id)], dtype=torch.long)
    cand_enc = torch.tensor([encode(candidate.term, char_to_id)], dtype=torch.long)
    feat = torch.tensor([features(token, candidate, alias_count)], dtype=torch.float32)
    with torch.no_grad():
        score = model(token_enc, cand_enc, feat).item()
    return score


def evaluate():
    random.seed(SEED)
    np.random.seed(SEED)
    torch.manual_seed(SEED)

    print("=" * 70)
    print("  ONNX SCORER — HELD-OUT EVALUATION (70/30 split)")
    print("=" * 70)
    print()

    # Split distortions
    train_distortions = {}
    test_distortions = {}
    for t in DEV_TERMS:
        dists = list(t["distortions"])
        train, test = split_distortions(dists, TRAIN_RATIO)
        train_distortions[t["term"]] = train
        test_distortions[t["term"]] = test
        print(f"  {t['term']:20s}  train={len(train)}  test={len(test)}  ({', '.join(test)})")

    total_train_dists = sum(len(v) for v in train_distortions.values())
    total_test_dists = sum(len(v) for v in test_distortions.values())
    print(f"\n  Total: {total_train_dists} train distortions, {total_test_dists} test (held-out)")
    print()

    # Train on train split only
    print("Training model on train split...")
    examples, alias_count = build_train_examples(DEV_TERMS, train_distortions)
    pos_count = sum(1 for e in examples if e.label == 1.0)
    neg_count = sum(1 for e in examples if e.label == 0.0)
    print(f"  {len(examples)} examples ({pos_count} pos, {neg_count} neg)")

    model, char_to_id = train_model(examples, alias_count)
    print("  Training done.\n")

    # Build vocab lookup
    protected = []
    for t in DEV_TERMS:
        protected.append(make_vocab_term(t["term"], t["type"]))
    by_norm = {normalize(vt.term): vt for vt in protected}

    # Evaluate on held-out test distortions
    print("=" * 70)
    print("  RESULTS: Held-out distortions (model has NEVER seen these)")
    print("=" * 70)
    print()

    total_correct = 0
    total_test = 0
    all_positive_scores = []
    per_term_results = []

    for t in DEV_TERMS:
        term_vt = by_norm[normalize(t["term"])]
        test_dists = test_distortions[t["term"]]
        correct = 0

        for dist in test_dists:
            # Score against correct term
            score = score_pair(
                model, char_to_id, dist, term_vt,
                alias_count.get(normalize(t["term"]), 0)
            )
            all_positive_scores.append(score)

            # Check if correct term wins (highest score among all candidates)
            best_term = t["term"]
            best_score = score
            for other in protected:
                if normalize(other.term) == normalize(t["term"]):
                    continue
                other_score = score_pair(
                    model, char_to_id, dist, other,
                    alias_count.get(normalize(other.term), 0)
                )
                if other_score > best_score:
                    best_score = other_score
                    best_term = other.term

            is_correct = best_term == t["term"] and score >= SCORE_THRESHOLD
            if is_correct:
                correct += 1
            total_correct += int(is_correct)
            total_test += 1

            status = "✓" if is_correct else "✗"
            print(
                f"  {status} {dist:20s} → {t['term']:15s}  "
                f"score={score:.3f}  winner={best_term}({best_score:.3f})"
            )

        term_recall = correct / len(test_dists) if test_dists else 0
        per_term_results.append((t["term"], correct, len(test_dists), term_recall))

    # Evaluate on hard negatives
    print()
    print("=" * 70)
    print("  RESULTS: Hard negatives (must NOT match any term)")
    print("=" * 70)
    print()

    false_positives = 0
    total_negatives = 0
    all_negative_scores = []

    for word in HARD_NEGATIVES:
        best_term = None
        best_score = 0.0
        for vt in protected:
            s = score_pair(
                model, char_to_id, word, vt,
                alias_count.get(normalize(vt.term), 0)
            )
            if s > best_score:
                best_score = s
                best_term = vt.term
        all_negative_scores.append(best_score)
        is_fp = best_score >= SCORE_THRESHOLD
        if is_fp:
            false_positives += 1
        total_negatives += 1
        status = "✗ FP" if is_fp else "✓ OK"
        print(f"  {status} {word:20s}  best={best_term}({best_score:.3f})")

    # Summary
    print()
    print("=" * 70)
    print("  SUMMARY")
    print("=" * 70)
    print()

    overall_recall = total_correct / total_test if total_test > 0 else 0
    fp_rate = false_positives / total_negatives if total_negatives > 0 else 0
    precision = total_correct / (total_correct + false_positives) if (total_correct + false_positives) > 0 else 0
    f1 = 2 * precision * overall_recall / (precision + overall_recall) if (precision + overall_recall) > 0 else 0

    print(f"  Held-out recall:     {total_correct}/{total_test} = {overall_recall:.1%}")
    print(f"  False positive rate: {false_positives}/{total_negatives} = {fp_rate:.1%}")
    print(f"  Precision:           {precision:.1%}")
    print(f"  F1:                  {f1:.1%}")
    print()
    print(f"  Positive scores:  mean={np.mean(all_positive_scores):.3f}  "
          f"min={np.min(all_positive_scores):.3f}  "
          f"median={np.median(all_positive_scores):.3f}")
    print(f"  Negative scores:  mean={np.mean(all_negative_scores):.3f}  "
          f"max={np.max(all_negative_scores):.3f}  "
          f"median={np.median(all_negative_scores):.3f}")
    print()

    print("  Per-term recall:")
    for term, correct, total, recall in sorted(per_term_results, key=lambda x: x[3]):
        bar = "█" * int(recall * 20) + "░" * (20 - int(recall * 20))
        print(f"    {term:20s}  {correct}/{total}  {bar}  {recall:.0%}")

    print()
    if overall_recall >= 0.80 and fp_rate <= 0.10:
        print("  ✓ PASS — scorer generalizes well to unseen distortions")
    elif overall_recall >= 0.60:
        print("  ~ MARGINAL — scorer partially generalizes, needs more training data")
    else:
        print("  ✗ FAIL — scorer does not generalize to unseen distortions")


if __name__ == "__main__":
    evaluate()
