"""Render the exact production voice polish system prompt (hinglish, neutral)."""

from __future__ import annotations

import re
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
PROMPT_RS = REPO / "crates/core/src/polish/prompt.rs"

LANGUAGE_RULE_HINGLISH = """- Output language: Roman Hinglish.
- Use ONLY Latin letters (A-Z, a-z), digits (0-9), and standard punctuation.
- Script rendering is required: convert all Devanagari to Roman word-by-word: "यह" = "Yeh", "बहुत" = "bahut".
- Script rendering is not translation: "hello भाई कैसे हो" = "hello bhai kaise ho", not "Namaste bhai kaise ho".
- Convert all non-Latin scripts (Japanese, Chinese, Korean, Arabic, Cyrillic) to Latin equivalents.
- Hindi words become Roman Hinglish. English words stay English. Preserve the speaker's mix.

Input: "यह बहुत सही बात है yaar. Please check this tomorrow."
Output: "Yeh bahut sahi baat hai yaar. Please check this tomorrow.\""""

PERSONA_NEUTRAL = "Be faithful to the spoken words first, then make them clear."
TONE_NEUTRAL = "Tone: neutral and clear. No strong stylistic lean."


def load_voice_template_from_rust() -> str:
    """Parse default_voice_prompt_template() raw string from prompt.rs."""
    text = PROMPT_RS.read_text(encoding="utf-8")
    match = re.search(
        r'pub fn default_voice_prompt_template\(\) -> String \{\s*r#"(.*)"#\s*\.to_string\(\)',
        text,
        re.DOTALL,
    )
    if not match:
        raise RuntimeError(f"Could not parse voice template from {PROMPT_RS}")
    return match.group(1)


def render_production_system_prompt(
    *,
    output_language: str = "hinglish",
    tone_preset: str = "neutral",
) -> str:
    """Match build_system_prompt_with_vocab_entries() with empty vocab/corrections."""
    if output_language != "hinglish":
        raise ValueError("latency bench only supports hinglish today")
    if tone_preset != "neutral":
        raise ValueError("latency bench only supports neutral tone today")

    template = load_voice_template_from_rust()
    return (
        template.replace("{{language_rule}}", LANGUAGE_RULE_HINGLISH)
        .replace("{{vocab_block}}", "")
        .replace("{{corrections_block}}", "")
        .replace("{{format_prefs_block}}", "")
        .replace("{{prefs_block}}", "")
        .replace("{{persona}}", PERSONA_NEUTRAL)
        .replace("{{tone}}", TONE_NEUTRAL)
    )
