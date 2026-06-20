#!/usr/bin/env python3
"""Benchmark AirNote ended-meeting AI on saved final transcripts.

The script mirrors the local Tauri meeting AI contract: draft MoM JSON, optional
strict verifier pass, transcript-evidence filtering for actions/decisions, and
grounded transcript Q&A. It is intentionally provider-agnostic and takes cases
as CLI inputs so benchmark recordings are not baked into the implementation.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


DEEPSEEK_URL = "https://api.deepseek.com/chat/completions"
GROQ_URL = "https://api.groq.com/openai/v1/chat/completions"
GATEWAY_URL = "https://gateway.outreachdeal.com/v1/chat/completions"

INTELLIGENCE_SYSTEM_PROMPT = """You are AirNote's meeting intelligence engine.

Use only the supplied transcript. Do not invent facts, attendees, dates, action items, or decisions.
The transcript may contain speaker labels and timestamps. Preserve uncertainty when the transcript is unclear.
Write the summary field as a detailed, client-ready Minutes of Meeting / MoM, not a short recap.
The MoM must be useful to someone who did not attend: explain the context, what the speakers were trying to do, what got clarified, what changed during the conversation, why it matters, and what remains unresolved.
Connect related points across the meeting when the transcript supports the connection, but do not invent facts beyond the transcript.
The summary field must be clean Markdown-compatible plain text, not HTML. Use numbered section headings and bullets. Do not return one giant paragraph.
For short/simple meetings, use fewer sections. For long, technical, sales, client, strategy, product, or operational meetings, make the MoM detailed and structured.
Prefer this numbered MoM structure when supported by the transcript:
1. Meeting Context
2. Participants / Stakeholders and Roles
3. Core Discussion
4. Important Background and Current State
5. Key Questions, Concerns, and Clarifications
6. Explanations / Options Discussed
7. Stakeholder Expectations and Success Criteria
8. Important Decisions or Alignments
9. Risks, Cautions, and Open Points
10. Agreed Action Items
11. Next Steps and Follow-Up Plan
12. Suggested Follow-Up Message
13. Final Interpretation
Do not force empty or irrelevant sections. If a section has no support, omit it or state the uncertainty briefly.
When the transcript is a client, sales, product, project, or consulting discussion, include practical implications where supported, such as client-side expectation, product-side implication, engineering-side implication, agency-side implication, timeline implication, or proposal implication.
Action-style sections in the summary may include tentative follow-ups and open possibilities, but label them as tentative unless the transcript confirms them.
Use specific nouns from the meeting instead of vague phrases like "they discussed various topics".
Summaries may mention proposals, debates, tentative follow-ups, tentative leanings, and unresolved questions.
Action items require an explicit firm commitment, assignment, or follow-up request in the transcript. If ownership is unclear, use null.
Do not include tentative follow-ups like "maybe", "probably", "I might", "we could", or "we can check" as action items. Mention them only in the summary.
Decisions require explicit agreement or a clear final choice. Do not convert brainstorms, preferences, suggestions, or tentative plans into decisions.
Phrases like "maybe", "should", "probably", "I think", "we could", or "we should" are not decisions unless a later turn clearly confirms agreement or commitment. When in doubt, leave decisions empty.
Every action item and decision must include an "evidence" field containing a short exact quote from the transcript line that supports it. If there is no exact quote, omit that item.
If an assignee is non-null, the evidence must clearly support that assignee by name, speaker label, or role. Otherwise set assignee to null.
Every action item must include "support": "firm". Every decision must include "support": "explicit". Omit items that cannot honestly use those support values.

Return only valid JSON with this exact shape:
{
  "summary": "Markdown-compatible detailed MoM with numbered section headings and bullets where supported",
  "action_items": [
    { "title": "specific action", "assignee": "speaker or person if explicit, else null", "due": "due date if explicit, else null", "evidence": "exact transcript quote", "support": "firm" }
  ],
  "decisions": [
    { "text": "specific decision if explicitly made", "evidence": "exact transcript quote", "support": "explicit" }
  ]
}"""

VERIFIER_SYSTEM_PROMPT = """You are AirNote's strict meeting intelligence verifier.

Use only the supplied transcript and draft JSON. Return only valid JSON with the same shape as the draft.

Rules:
- Rewrite the summary if it states tentative proposals as settled decisions.
- Preserve or improve the summary's detailed numbered MoM format. Do not collapse it into one paragraph or a short recap.
- The summary should remain useful to someone who did not attend: include supported context, core discussion, key questions, clarifications, implications, risks/open points, action-style follow-ups, and final interpretation where the transcript supports them.
- Remove or soften any unsupported implications, risks, deliverables, follow-up messages, or stakeholder expectations.
- Keep an action item only when the transcript contains an explicit firm commitment, assignment, or follow-up request.
- Remove tentative follow-ups like "maybe", "probably", "I might", "we could", or "we can check" from action items. They can stay in the summary.
- Keep a decision only when the transcript contains explicit agreement or a clear final choice.
- Remove brainstorms, preferences, suggestions, strong leanings, and tentative plans from decisions.
- Every kept action item and decision must include an "evidence" field copied from the transcript. Do not paraphrase evidence.
- Every kept action item must include "support": "firm"; every kept decision must include "support": "explicit".
- If a kept item's evidence does not directly support the item, remove the item.
- If uncertain, remove the action item or decision and mention the uncertainty only in the summary."""

CHAT_SYSTEM_PROMPT = """You are AirNote's meeting Q&A engine.

Answer using only the supplied transcript. Use meeting intelligence as a hint, not as authority.
If the answer is not present, say that the transcript does not contain it.
Do not infer owners, decisions, dates, or commitments beyond the transcript.
When asked about decisions, use only the provided decisions list; if it is empty, say no explicit decisions are captured.
When asked about risks or unresolved questions, do not label an accepted next step as a risk unless the transcript says it is uncertain, blocked, infeasible, or untested.
When writing briefs, keep proposals and leanings out of the Decisions section unless the provided decisions list contains them.
Be concise and cite timestamp/speaker labels when useful."""

JUDGE_SYSTEM_PROMPT = """You are a strict meeting AI evaluator.

Use only the transcript to judge the MoM and chat answers. Penalize invented facts, invented owners, invented decisions, unsupported commitments, and vague answers.
Return only JSON with integer scores from 1 to 10 and a short issue list:
{
  "summary_accuracy": 1,
  "summary_coverage": 1,
  "action_precision": 1,
  "decision_precision": 1,
  "chat_grounding": 1,
  "overall": 1,
  "issues": ["specific issue"]
}"""

DEFAULT_QUESTIONS = [
    "What was the main topic of this meeting?",
    "What concrete decisions were explicitly made? If none, say none.",
    "List only explicit action items or follow-ups, with owners only if the transcript gives them.",
    "What important unresolved questions or risks remain?",
    "Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.",
]


@dataclass
class ProviderConfig:
    provider: str
    model: str
    url: str
    auth_header_name: str
    auth_header_value: str


def load_env(path: Path) -> None:
    if not path.exists():
        return
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        value = value.strip().strip('"').strip("'")
        os.environ.setdefault(key.strip(), value)


def env_nonempty(name: str) -> str | None:
    value = os.environ.get(name, "").strip()
    return value or None


def default_model(provider: str) -> str:
    if provider == "deepseek":
        return "deepseek-v4-pro"
    if provider == "groq":
        return "llama-3.3-70b-versatile"
    if provider == "gateway":
        return "gemini-2.5-flash"
    raise ValueError(f"unsupported provider: {provider}")


def provider_config(args: argparse.Namespace) -> ProviderConfig:
    provider = (args.provider or env_nonempty("AIRNOTE_MEETING_AI_PROVIDER") or env_nonempty("AIRNOTE_MEETING_CLEANUP_PROVIDER") or "deepseek").lower()
    model = args.model or env_nonempty("AIRNOTE_MEETING_AI_MODEL") or env_nonempty("AIRNOTE_MEETING_CLEANUP_MODEL") or default_model(provider)
    override_key = args.api_key or env_nonempty("AIRNOTE_MEETING_AI_API_KEY") or env_nonempty("AIRNOTE_MEETING_CLEANUP_API_KEY")
    if provider == "deepseek":
        api_key = override_key or env_nonempty("DEEPSEEK_API_KEY")
        if not api_key:
            raise SystemExit("DEEPSEEK_API_KEY is required")
        return ProviderConfig(provider, model, DEEPSEEK_URL, "Authorization", f"Bearer {api_key}")
    if provider == "groq":
        api_key = override_key or env_nonempty("GROQ_API_KEY")
        if not api_key:
            raise SystemExit("GROQ_API_KEY is required")
        return ProviderConfig(provider, model, GROQ_URL, "Authorization", f"Bearer {api_key}")
    if provider == "gateway":
        api_key = override_key or env_nonempty("GATEWAY_API_KEY")
        if not api_key:
            raise SystemExit("GATEWAY_API_KEY is required")
        return ProviderConfig(provider, model, GATEWAY_URL, "X-API-Key", api_key)
    raise SystemExit(f"unsupported provider: {provider}")


def call_llm(config: ProviderConfig, system: str, user: str, timeout: int, max_tokens: int) -> dict[str, Any]:
    body: dict[str, Any] = {
        "model": config.model,
        "stream": False,
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    }
    if config.provider == "deepseek":
        body["thinking"] = {"type": "disabled"}
    request = urllib.request.Request(
        config.url,
        data=json.dumps(body).encode("utf-8"),
        headers={
            config.auth_header_name: config.auth_header_value,
            "Content-Type": "application/json",
        },
    )
    started = time.time()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")[:2000]
        raise RuntimeError(f"provider HTTP {exc.code}: {detail}") from exc
    content = payload["choices"][0]["message"]["content"].strip()
    return {
        "provider": config.provider,
        "model": config.model,
        "latency_ms": round((time.time() - started) * 1000),
        "content": content,
    }


def extract_json_object(text: str) -> dict[str, Any]:
    stripped = text.strip()
    if stripped.startswith("```"):
        stripped = re.sub(r"^```[a-zA-Z0-9_-]*\s*", "", stripped)
        stripped = re.sub(r"\s*```$", "", stripped)
    match = re.search(r"\{.*\}", stripped, flags=re.S)
    if not match:
        raise ValueError("LLM returned no JSON object")
    return json.loads(match.group(0))


def normalize_evidence(text: str) -> str:
    normalized = "".join(ch.lower() if ch.isalnum() or ch == "'" else " " for ch in text)
    return " ".join(normalized.split())


def evidence_matches(evidence: str | None, transcript: str) -> bool:
    if not evidence:
        return False
    evidence_norm = normalize_evidence(evidence)
    if len(evidence_norm.split()) < 4:
        return False
    transcript_norm = normalize_evidence(transcript)
    if evidence_norm in transcript_norm:
        return True
    tokens = evidence_norm.split()
    if len(tokens) < 6:
        return False
    matched = sum(1 for token in tokens if token in transcript_norm)
    return matched / len(tokens) >= 0.85


def support_matches(value: Any, expected: str) -> bool:
    return isinstance(value, str) and value.strip().lower() == expected


def filter_with_evidence(mom: dict[str, Any], transcript: str) -> dict[str, Any]:
    filtered_actions = []
    for item in mom.get("action_items") or []:
        if not isinstance(item, dict):
            continue
        title = str(item.get("title") or "").strip()
        evidence = str(item.get("evidence") or "").strip()
        if title and support_matches(item.get("support"), "firm") and evidence_matches(evidence, transcript):
            filtered_actions.append({
                "title": title,
                "assignee": item.get("assignee") or None,
                "due": item.get("due") or None,
                "evidence": evidence,
            })

    filtered_decisions = []
    for item in mom.get("decisions") or []:
        if isinstance(item, str):
            continue
        if not isinstance(item, dict):
            continue
        text = str(item.get("text") or "").strip()
        evidence = str(item.get("evidence") or "").strip()
        if text and support_matches(item.get("support"), "explicit") and evidence_matches(evidence, transcript):
            filtered_decisions.append({"text": text, "evidence": evidence})

    return {
        "summary": str(mom.get("summary") or "").strip(),
        "action_items": filtered_actions,
        "decisions": filtered_decisions,
    }


def parse_case(value: str) -> tuple[str, Path]:
    if "=" in value:
        name, path = value.split("=", 1)
        return name.strip(), Path(path).expanduser()
    path = Path(value).expanduser()
    return path.stem, path


def run_case(
    name: str,
    transcript_path: Path,
    config: ProviderConfig,
    args: argparse.Namespace,
    questions: list[str],
) -> dict[str, Any]:
    transcript = transcript_path.read_text(encoding="utf-8")
    draft = call_llm(
        config,
        INTELLIGENCE_SYSTEM_PROMPT,
        f"Transcript source: final\n\nTranscript:\n<<<TRANSCRIPT\n{transcript}\nTRANSCRIPT>>>",
        args.timeout,
        args.max_tokens,
    )
    draft_json = extract_json_object(draft["content"])
    verified = None
    verified_json = draft_json
    if args.verify:
        verified = call_llm(
            config,
            VERIFIER_SYSTEM_PROMPT,
            f"Transcript:\n<<<TRANSCRIPT\n{transcript}\nTRANSCRIPT>>>\n\nDraft JSON:\n<<<JSON\n{draft['content']}\nJSON>>>",
            args.timeout,
            args.max_tokens,
        )
        verified_json = extract_json_object(verified["content"])

    filtered_mom = filter_with_evidence(verified_json, transcript)
    chat_items = []
    summary_json = json.dumps(filtered_mom, ensure_ascii=False)
    for question in questions:
        answer = call_llm(
            config,
            CHAT_SYSTEM_PROMPT,
            f"Transcript source: final\n\nMeeting intelligence:\n<<<SUMMARY\n{summary_json}\nSUMMARY>>>\n\nTranscript:\n<<<TRANSCRIPT\n{transcript}\nTRANSCRIPT>>>\n\nQuestion:\n{question}",
            args.timeout,
            args.chat_max_tokens,
        )
        chat_items.append({
            "question": question,
            "answer": answer["content"],
            "latency_ms": answer["latency_ms"],
        })

    judge = None
    if args.judge:
        judge_response = call_llm(
            config,
            JUDGE_SYSTEM_PROMPT,
            "Transcript:\n<<<TRANSCRIPT\n"
            + transcript
            + "\nTRANSCRIPT>>>\n\nMoM JSON:\n<<<JSON\n"
            + json.dumps(filtered_mom, ensure_ascii=False)
            + "\nJSON>>>\n\nChat answers:\n<<<CHAT\n"
            + json.dumps(chat_items, ensure_ascii=False)
            + "\nCHAT>>>",
            args.timeout,
            args.chat_max_tokens,
        )
        judge = {
            "latency_ms": judge_response["latency_ms"],
            "result": extract_json_object(judge_response["content"]),
        }

    return {
        "case": name,
        "transcript_path": str(transcript_path),
        "provider": config.provider,
        "model": config.model,
        "draft_latency_ms": draft["latency_ms"],
        "verify_latency_ms": verified["latency_ms"] if verified else 0,
        "filtered_mom": filtered_mom,
        "raw_counts": {
            "draft_actions": len(draft_json.get("action_items") or []),
            "draft_decisions": len(draft_json.get("decisions") or []),
            "verified_actions": len(verified_json.get("action_items") or []),
            "verified_decisions": len(verified_json.get("decisions") or []),
            "filtered_actions": len(filtered_mom["action_items"]),
            "filtered_decisions": len(filtered_mom["decisions"]),
        },
        "chat": chat_items,
        "judge": judge,
    }


def write_markdown_report(results: list[dict[str, Any]], path: Path) -> None:
    lines = ["# AirNote Meeting AI Benchmark", ""]
    for result in results:
        mom = result["filtered_mom"]
        lines += [
            f"## {result['case']}",
            "",
            f"Provider/model: {result['provider']} / {result['model']}",
            f"Draft latency: {result['draft_latency_ms']} ms; verifier latency: {result['verify_latency_ms']} ms",
            f"Counts: `{json.dumps(result['raw_counts'])}`",
            "",
            "### Summary",
            mom["summary"] or "(empty)",
            "",
            "### Actions",
        ]
        if mom["action_items"]:
            for item in mom["action_items"]:
                lines.append(f"- {item['title']} | assignee: {item.get('assignee')} | due: {item.get('due')} | evidence: {item.get('evidence')}")
        else:
            lines.append("- None explicit.")
        lines += ["", "### Decisions"]
        if mom["decisions"]:
            for item in mom["decisions"]:
                lines.append(f"- {item['text']} | evidence: {item.get('evidence')}")
        else:
            lines.append("- None explicit.")
        if result.get("judge"):
            lines += ["", "### Judge", "```json", json.dumps(result["judge"]["result"], indent=2, ensure_ascii=False), "```"]
        lines += ["", "### Chat"]
        for index, item in enumerate(result["chat"], 1):
            lines += ["", f"Q{index}. {item['question']}", "", item["answer"]]
        lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", action="append", default=[], help="Case as NAME=/path/transcript.txt. Repeatable.")
    parser.add_argument("--out-dir", type=Path, default=Path("tools/stt-bench/meeting-ai-results"))
    parser.add_argument("--env-file", type=Path, default=Path(".env"))
    parser.add_argument("--provider", default="")
    parser.add_argument("--model", default="")
    parser.add_argument("--api-key", default="")
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--max-tokens", type=int, default=8192)
    parser.add_argument("--chat-max-tokens", type=int, default=1800)
    parser.add_argument("--no-verify", dest="verify", action="store_false", default=True)
    parser.add_argument("--judge", action="store_true")
    parser.add_argument("--questions", type=Path)
    args = parser.parse_args()

    load_env(args.env_file)
    config = provider_config(args)
    cases = [parse_case(value) for value in args.case]
    if not cases:
        raise SystemExit("provide at least one --case NAME=/path/to/transcript.txt")
    questions = DEFAULT_QUESTIONS
    if args.questions:
        questions = [line.strip() for line in args.questions.read_text(encoding="utf-8").splitlines() if line.strip()]

    args.out_dir.mkdir(parents=True, exist_ok=True)
    results = []
    for name, transcript_path in cases:
        if not transcript_path.is_file():
            raise SystemExit(f"missing transcript: {transcript_path}")
        print(f"Running {name}...")
        result = run_case(name, transcript_path, config, args, questions)
        result_path = args.out_dir / f"{name}.meeting-ai.json"
        result_path.write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8")
        results.append(result)
    summary_path = args.out_dir / "summary.json"
    report_path = args.out_dir / "summary.md"
    summary_path.write_text(json.dumps(results, indent=2, ensure_ascii=False), encoding="utf-8")
    write_markdown_report(results, report_path)
    print(json.dumps({
        "cases": [result["case"] for result in results],
        "summary_path": str(summary_path),
        "report_path": str(report_path),
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
