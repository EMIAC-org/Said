# Polish prompt lab

Iterate on voice polish against a **fixed STT transcript** — fair A/B for prompts and models.

## Quick start

```bash
# 1. Record READ_SCRIPT.txt → WAV, then STT once + cache
python lab/polish_lab.py /path/to/recording.wav

# 2. Single-model polish (default route from .env)
python lab/polish_lab.py

# 3. Parallel shootout — ~10 models at once
python lab/compare_models.py
# or:
python lab/polish_lab.py --compare-models
```

## Architecture

```
                    READ_SCRIPT.txt → WAV
                              ↓
                    polish_lab.py (Swift STT once)
                              ↓
                    cache/session.json  ← fixed transcript
                              ↓
         ┌────────────────────┴────────────────────┐
         ↓                                         ↓
  polish_lab.py                           compare_models.py
  (1 model, prompt iterate)               (N models in parallel)
         ↓                                         ↓
  lab/runs/<ts>.md                        lab/model_runs/<ts>/
                                                    ├── report.md
                                                    ├── results.json
                                                    └── <slug>.md × N
```

| File | Role |
|---|---|
| `polish_lab.py` | STT cache, single polish, `--compare-models` entry |
| `compare_models.py` | Parallel shootout across catalog |
| `model_catalog.py` | **Curated models** (Cerebras production + Groq candidates) |
| `scoring.py` | Heuristic quality score for the 58-word dev clip |
| `prompt_system.md` | Editable system prompt |
| `run_prompt_matrix.py` | Many prompts × few routes (prompt A/B) |

## Model catalog

Edit `model_catalog.py` to change candidates.

| Slug | Provider | Model |
|---|---|---|
| `groq-gpt-oss` | Groq | `openai/gpt-oss-120b` (**production smart tier**) |
| `groq-8b-instant` | Groq | `llama-3.1-8b-instant` |
| `groq-scout` | Groq | `meta-llama/llama-4-scout-17b-16e-instruct` |
| `groq-maverick` | Groq | `meta-llama/llama-4-maverick-17b-128e-instruct` |
| `groq-70b` | Groq | `llama-3.3-70b-versatile` |
| `groq-qwen3-32b` | Groq | `qwen/qwen3-32b` |

Only models whose provider key is in `.env` run.

## Compare models commands

```bash
python lab/compare_models.py                    # all runnable models, 10 workers
python lab/compare_models.py --dry-run          # list models, no API calls
python lab/compare_models.py --workers 6
python lab/compare_models.py --provider groq
python lab/compare_models.py --slug groq-scout,groq-gpt-oss
python lab/compare_models.py --prompt lab/prompt_system.md
```

## Scoring

`scoring.py` checks the polished output against expected dev terms from `READ_SCRIPT.txt`:
Caps Lock, STT, DeepInfra Maverick test, Docker, SQLite, webhook, Sentry, PR, etc.

Penalizes leftover garbles: `app slot`, `STD`, `doctor rebuild`, `century`, etc.

Max score ≈ 30. Use `report.md` ranking + read outputs — heuristic only.

## Env keys

```
GROQ_API_KEY=          # production polish (GPT OSS smart + 8B fast) + lab models
```

## Single-model routing (unchanged)

`polish_lab.py` without `--compare-models` uses Groq GPT OSS 120B when `GROQ_API_KEY` is set.

## Latency benchmark (Groq Scout vs Groq GPT OSS)

Isolate LLM speed from STT / Tauri / streaming stack:

```bash
python lab/latency_bench.py              # 10 runs each, streaming (prod-like)
python lab/latency_bench.py --runs 15 --warmup 2
python lab/latency_bench.py --dry-run
python lab/latency_bench.py --no-stream  # non-streaming only
```

Uses:
- Cached transcript from `lab/cache/session.json`
- Production system prompt parsed live from `crates/core/src/polish/prompt.rs`
- Same payload as backend: `stream=true`, `temperature=0.0`, stop sequences, `max_tokens`

Output: `lab/latency_runs/<timestamp>/report.md` + `results.json`

Requires `GROQ_API_KEY` (or `GATEWAY_API_KEY`).

## STT

Local Oriserve Swift via `tools/stt-compare/transcribe_swift.py`. First run may download HF weights.
