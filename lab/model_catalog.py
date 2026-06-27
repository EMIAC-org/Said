"""Curated polish models for parallel lab comparison.

Mirrors `said_core::polish::model::POLISH_MODEL_CATALOG` (production + beta).
Edit this file to swap candidates — compare_models.py and batch_compare_two_models.py read it.

Benchmark tiers (June 2026 shootout):
  - Groq: 8B, Scout, 20B/120B GPT-OSS, 70B (reference)
  - DeepInfra: Phi, Llama 4, Llama 3.x, Qwen, Mistral, Gemma, Nemotron, DeepSeek
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import Any

GROQ_BASE = "https://api.groq.com/openai/v1"
CEREBRAS_BASE = "https://api.cerebras.ai/v1"
DEEPINFRA_BASE = "https://api.deepinfra.com/v1/openai"


@dataclass(frozen=True)
class ModelSpec:
    slug: str
    provider: str  # groq | cerebras | deepinfra
    model: str
    label: str
    temperature: float = 0.0
    extra_payload: dict[str, Any] = field(default_factory=dict)
    bench_tier: str = "mid"  # tiny | small | mid | large — for report grouping

    def to_route(self, api_key: str) -> dict[str, str | float | dict[str, Any]]:
        base = {
            "groq": GROQ_BASE,
            "cerebras": CEREBRAS_BASE,
            "deepinfra": DEEPINFRA_BASE,
        }.get(self.provider, GROQ_BASE)
        return {
            "slug": self.slug,
            "label": self.label,
            "provider": self.provider,
            "base_url": base,
            "api_key": api_key,
            "model": self.model,
            "temperature": str(self.temperature),
            "extra_payload": dict(self.extra_payload),
            "bench_tier": self.bench_tier,
        }


_GROQ_OSS_EXTRA = {"max_tokens": 4096, "reasoning_effort": "low"}

# AirNote polish catalog — matches Rust production keys + DeepInfra shootout set.
LAB_MODEL_CATALOG: list[ModelSpec] = [
    # ── Groq (latency + production candidates) ───────────────────────────────
    ModelSpec(
        slug="fast",
        provider="groq",
        model="llama-3.1-8b-instant",
        label="Llama 3.1 8B Instant (Groq)",
        bench_tier="small",
    ),
    ModelSpec(
        slug="groq-scout",
        provider="groq",
        model="meta-llama/llama-4-scout-17b-16e-instruct",
        label="Llama 4 Scout 17B (Groq)",
        bench_tier="mid",
    ),
    ModelSpec(
        slug="groq-gpt-oss-20b",
        provider="groq",
        model="openai/gpt-oss-20b",
        label="GPT OSS 20B (Groq)",
        extra_payload=dict(_GROQ_OSS_EXTRA),
        bench_tier="mid",
    ),
    ModelSpec(
        slug="smart",
        provider="groq",
        model="openai/gpt-oss-120b",
        label="GPT OSS 120B (Groq)",
        extra_payload=dict(_GROQ_OSS_EXTRA),
        bench_tier="large",
    ),
    ModelSpec(
        slug="groq-70b",
        provider="groq",
        model="llama-3.3-70b-versatile",
        label="Llama 3.3 70B (Groq)",
        bench_tier="large",
    ),
    # ── Cerebras (production smart alt) ──────────────────────────────────────
    ModelSpec(
        slug="cerebras-gpt-oss",
        provider="cerebras",
        model="gpt-oss-120b",
        label="GPT OSS 120B (Cerebras)",
        extra_payload=dict(_GROQ_OSS_EXTRA),
        bench_tier="large",
    ),
    # ── DeepInfra — small / fast (dictation polish shootout) ───────────────
    ModelSpec(
        slug="phi4",
        provider="deepinfra",
        model="microsoft/phi-4",
        label="Phi-4 (DeepInfra)",
        bench_tier="mid",
    ),
    ModelSpec(
        slug="di-phi-3.5-mini",
        provider="deepinfra",
        model="microsoft/Phi-3.5-mini-instruct",
        label="Phi-3.5 Mini (DeepInfra)",
        bench_tier="tiny",
    ),
    ModelSpec(
        slug="di-llama-3.2-3b",
        provider="deepinfra",
        model="meta-llama/Llama-3.2-3B-Instruct",
        label="Llama 3.2 3B (DeepInfra)",
        bench_tier="tiny",
    ),
    ModelSpec(
        slug="di-llama-8b",
        provider="deepinfra",
        model="meta-llama/Meta-Llama-3.1-8B-Instruct",
        label="Llama 3.1 8B (DeepInfra)",
        bench_tier="small",
    ),
    ModelSpec(
        slug="di-nemotron-8b",
        provider="deepinfra",
        model="nvidia/Llama-3.1-Nemotron-8B-Instruct",
        label="Nemotron 8B (DeepInfra)",
        bench_tier="small",
    ),
    ModelSpec(
        slug="di-qwen2.5-7b",
        provider="deepinfra",
        model="Qwen/Qwen2.5-7B-Instruct",
        label="Qwen 2.5 7B (DeepInfra)",
        bench_tier="small",
    ),
    ModelSpec(
        slug="di-mistral-7b",
        provider="deepinfra",
        model="mistralai/Mistral-7B-Instruct-v0.3",
        label="Mistral 7B v0.3 (DeepInfra)",
        bench_tier="small",
    ),
    ModelSpec(
        slug="di-gemma-2-9b",
        provider="deepinfra",
        model="google/gemma-2-9b-it",
        label="Gemma 2 9B (DeepInfra)",
        bench_tier="small",
    ),
    # ── DeepInfra — mid (best quality/speed tradeoff) ──────────────────────
    ModelSpec(
        slug="di-scout",
        provider="deepinfra",
        model="meta-llama/Llama-4-Scout-17B-16E-Instruct",
        label="Llama 4 Scout 17B (DeepInfra)",
        bench_tier="mid",
    ),
    ModelSpec(
        slug="di-maverick-fp8",
        provider="deepinfra",
        model="meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8",
        label="Llama 4 Maverick FP8 (DeepInfra)",
        bench_tier="mid",
    ),
    ModelSpec(
        slug="di-qwen2.5-14b",
        provider="deepinfra",
        model="Qwen/Qwen2.5-14B-Instruct",
        label="Qwen 2.5 14B (DeepInfra)",
        bench_tier="mid",
    ),
    ModelSpec(
        slug="di-mistral-small",
        provider="deepinfra",
        model="mistralai/Mistral-Small-24B-Instruct-2501",
        label="Mistral Small 24B (DeepInfra)",
        bench_tier="mid",
    ),
    ModelSpec(
        slug="di-qwen3-32b",
        provider="deepinfra",
        model="Qwen/Qwen3-32B",
        label="Qwen3 32B (DeepInfra)",
        bench_tier="mid",
    ),
    # ── DeepInfra — large (quality reference, slower) ──────────────────────
    ModelSpec(
        slug="di-llama-3.3-70b",
        provider="deepinfra",
        model="meta-llama/Meta-Llama-3.3-70B-Instruct",
        label="Llama 3.3 70B (DeepInfra)",
        bench_tier="large",
    ),
    ModelSpec(
        slug="di-deepseek-v3",
        provider="deepinfra",
        model="deepseek-ai/DeepSeek-V3",
        label="DeepSeek V3 (DeepInfra)",
        bench_tier="large",
    ),
]

# Default slugs for a focused re-benchmark (user-requested set).
# Top 5 from batch_full_20260623T153104Z (balanced quality + speed).
ROUND1_WINNERS: list[str] = [
    "fast",
    "groq-gpt-oss-20b",
    "di-mistral-7b",
    "di-mistral-small",
    "di-maverick-fp8",
]

BENCHMARK_DEFAULT_SLUGS: list[str] = [
    "fast",
    "groq-scout",
    "groq-gpt-oss-20b",
    "smart",
    "phi4",
    "di-scout",
    "di-maverick-fp8",
    "di-llama-8b",
    "di-qwen2.5-7b",
    "di-qwen2.5-14b",
    "di-mistral-7b",
    "di-mistral-small",
    "di-gemma-2-9b",
    "di-nemotron-8b",
    "di-phi-3.5-mini",
]


def groq_api_key() -> str:
    return os.getenv("GROQ_API_KEY", "").strip() or os.getenv("GATEWAY_API_KEY", "").strip()


def cerebras_api_key() -> str:
    return os.getenv("CEREBRAS_API_KEY", "").strip()


def deepinfra_api_key() -> str:
    return os.getenv("DEEPINFRA_API_KEY", "").strip()


def available_lab_routes(
    catalog: list[ModelSpec] | None = None,
    *,
    providers: set[str] | None = None,
    slugs: set[str] | None = None,
) -> list[dict[str, str | float | dict[str, Any]]]:
    """Return routes for catalog entries whose provider API key is set."""
    items = catalog or LAB_MODEL_CATALOG
    g_key = groq_api_key()
    c_key = cerebras_api_key()
    d_key = deepinfra_api_key()
    routes: list[dict[str, str | float | dict[str, Any]]] = []
    for spec in items:
        if providers and spec.provider not in providers:
            continue
        if slugs and spec.slug not in slugs:
            continue
        if spec.provider == "groq":
            if not g_key:
                continue
            routes.append(spec.to_route(g_key))
        elif spec.provider == "cerebras":
            if not c_key:
                continue
            routes.append(spec.to_route(c_key))
        elif spec.provider == "deepinfra":
            if not d_key:
                continue
            routes.append(spec.to_route(d_key))
    return routes
