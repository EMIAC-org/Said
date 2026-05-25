#!/usr/bin/env python3
"""Evaluate the cheap LLM alias-safety judge with rate limiting.

Writes JSONL under .context so repo-tracked files are not modified by a run.
Requires GROQ_API_KEY. This script intentionally runs serially.
"""

from __future__ import annotations

import json
import os
import time
import urllib.error
import urllib.request
from pathlib import Path


MODEL = "llama-3.1-8b-instant"
ENDPOINT = "https://api.groq.com/openai/v1/chat/completions"
OUT = Path(".context/alias_safety_llm_eval.jsonl")
MIN_DELAY_SECONDS = 0.4

SYSTEM = (
    "You are a conservative safety classifier for a speech dictation app. "
    "Decide whether SOURCE_ALIAS is a common Hindi, Hinglish, or English word/phrase "
    "that would be dangerous to auto-replace, or rare jargon/proper noun/code-like speech. "
    "Return strict JSON only: "
    '{"verdict":"common_block|safe_jargon|ambiguous_block","confidence":0.0-1.0,"reason":"short reason"}. '
    "Use common_block for normal words, particles, verbs, pronouns, question words, dictionary words, or ordinary phrases. "
    "Use safe_jargon only for rare ASR distortions, acronyms, proper nouns, code identifiers, brands, or product names. "
    "If unsure, use ambiguous_block."
)


COMMON = [
    "kaisa", "kaisi", "kaise", "कैसा", "यह कैसा लगा", "ye kaisa laga", "laga", "lagi", "lage",
    "main", "mein", "maine", "mujhe", "tum", "aap", "hum", "hai", "hain", "kya", "ye", "yeh",
    "aisa", "aisi", "aise", "ka", "ki", "ke", "ko", "se", "pe", "bhi", "hi", "nahi", "mat",
    "time", "can", "go", "return", "think", "meaning", "course", "corps", "accounts", "google",
    "prayer", "cancer", "capital", "house", "table", "next", "rest", "tell", "sent", "oath",
    "white", "post gray", "local house", "super base", "graph cool", "dock worker", "company",
    "color", "choices", "army", "division", "regular", "submitted", "worker", "good", "great",
    "nice", "said", "should", "would", "could", "through", "where", "which", "about", "before",
    "after", "just", "like", "please", "bahut", "thoda", "kam", "accha", "sahi", "galat",
]

JARGON = [
    "Macobs", "macos", "micobs", "mecobs", "mccorps", "EMIAC", "meac", "emiak", "n8n",
    "Kubernetes", "cubernetis", "kubectl", "GraphQL", "Supabase", "PostgreSQL", "OAuth",
    "JWT", "WebSocket", "TypeScript", "Vercel", "Razorpay", "Docker", "Prisma",
    "Redis", "Tauri", "SQLite", "Vite", "GitHub", "GitLab", "Next.js", "FastAPI",
    "LangChain", "OpenTelemetry", "Prometheus", "Grafana", "ClickHouse", "Elasticsearch",
    "DynamoDB", "RabbitMQ", "Kafka", "Terraform", "Cloudflare", "Sentry", "Stripe",
    "Clerk", "PostHog", "Meilisearch", "Qdrant", "Milvus", "ONNX", "LoRA", "RLHF",
]


def examples() -> list[dict[str, str]]:
    rows = []
    targets = ["Macobs", "EMIAC", "Kubernetes", "GraphQL"]
    for source in COMMON:
        for target in targets:
            rows.append({"source": source, "target": target, "expected": "block"})
    for source in JARGON:
        rows.append({"source": source, "target": source if source[0].isupper() else "Macobs", "expected": "allow"})
    return rows


def call_groq(api_key: str, source: str, target: str) -> dict:
    user = (
        f"SOURCE_ALIAS: {source}\n"
        f"SOURCE_NORMALIZED: {source.lower()}\n"
        f"TARGET_CANONICAL: {target}\n"
        "CONTEXT: test sentence\n\n"
        "Classify SOURCE_ALIAS only. Do not invent mappings."
    )
    body = json.dumps({
        "model": MODEL,
        "temperature": 0,
        "max_tokens": 120,
        "response_format": {"type": "json_object"},
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": user},
        ],
    }).encode("utf-8")
    req = urllib.request.Request(
        ENDPOINT,
        data=body,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=8) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
    content = payload["choices"][0]["message"]["content"]
    return json.loads(content)


def main() -> int:
    api_key = os.environ.get("GROQ_API_KEY", "").strip()
    if not api_key:
        print("GROQ_API_KEY is not set; skipping LLM alias safety eval")
        return 2

    OUT.parent.mkdir(parents=True, exist_ok=True)
    rows = examples()
    false_allows = 0
    false_blocks = 0
    last_call = 0.0
    with OUT.open("w", encoding="utf-8") as fh:
        for idx, row in enumerate(rows, start=1):
            elapsed = time.time() - last_call
            if elapsed < MIN_DELAY_SECONDS:
                time.sleep(MIN_DELAY_SECONDS - elapsed)
            last_call = time.time()
            try:
                verdict = call_groq(api_key, row["source"], row["target"])
            except urllib.error.HTTPError as exc:
                verdict = {"verdict": "error", "confidence": 0.0, "reason": f"http {exc.code}"}
            except Exception as exc:
                verdict = {"verdict": "error", "confidence": 0.0, "reason": str(exc)}
            got = verdict.get("verdict", "")
            expected = row["expected"]
            ok = (expected == "block" and got != "safe_jargon") or (
                expected == "allow" and got == "safe_jargon"
            )
            if expected == "block" and got == "safe_jargon":
                false_allows += 1
            if expected == "allow" and got != "safe_jargon":
                false_blocks += 1
            out = {**row, "got": got, "ok": ok, "llm": verdict}
            fh.write(json.dumps(out, ensure_ascii=False) + "\n")
            print(f"{idx:03d}/{len(rows)} {row['source']!r} -> {got} ok={ok}")

    print(f"wrote {OUT}")
    print(f"false_allows={false_allows} false_blocks={false_blocks}")
    return 0 if false_allows == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
