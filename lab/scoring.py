"""Heuristic quality scoring for lab polish outputs (58-word dev clip)."""

from __future__ import annotations

EXPECTED_TERMS = [
    "suno zara",
    "Caps Lock",
    "Swift",
    "STT",
    "DeepInfra Maverick test",
    "Docker",
    "SQLite",
    "webhook",
    "Sentry",
    "PR",
]

BAD_GARBLES = [
    "sonoo",
    "jara",
    "app slot",
    "STD",
    "deep infra, memory",
    "memory test karna",
    "doctor rebuild",
    "CQLite",
    "webbook",
    "century",
]


def score_output(text: str) -> dict[str, object]:
    lower = text.lower()
    expected_hits = [term for term in EXPECTED_TERMS if term.lower() in lower]
    bad_hits = [term for term in BAD_GARBLES if term.lower() in lower]
    preamble_penalty = int(
        lower.startswith(("here", "sure", "the polished", "output:", "polished:"))
    )
    non_latin_penalty = int(any(ord(ch) > 127 for ch in text))
    score = (
        len(expected_hits) * 3
        - len(bad_hits) * 4
        - preamble_penalty * 4
        - non_latin_penalty * 3
    )
    return {
        "score": score,
        "expected_hits": expected_hits,
        "missing_terms": [term for term in EXPECTED_TERMS if term not in expected_hits],
        "bad_hits": bad_hits,
        "preamble_penalty": preamble_penalty,
        "non_latin_penalty": non_latin_penalty,
    }
