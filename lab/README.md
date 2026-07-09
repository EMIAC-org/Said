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

## Codex Spark

`gpt-5.3-codex-spark` is a Codex research-preview model, not a generally
available public API model. Do not copy its Codex access or refresh tokens into
`.env` or this repository.

To authenticate the local Codex CLI through device login, run:

```bash
lab/codex_spark_device_login.sh
```

The CLI stores credentials in its own local auth store. Spark can be exercised
through `codex exec`, but that is an agent invocation with Codex system/tool
overhead, so it is not a fair replacement for this lab's raw provider API
latency benchmark.

For an explicit end-to-end comparison against the current production model,
run the dedicated harness. It measures Spark through the Codex CLI and
`gemma-4-31b` through Cerebras, and keeps those two transport paths distinct
in its report:

```bash
python lab/codex_agent_latency_bench.py --runs 5 --warmup 1
```

It requires `CEREBRAS_API_KEY` in the gitignored root `.env`. Results are
written under `lab/latency_runs/` and are not committed.

For correction quality, use the same curated stress suite and strict scorecard
as `server_bench.py`:

```bash
python3 lab/codex_correction_bench.py
```

Its default is the correction-critical subset: technical garble recovery and
over-correction traps. Results go to `lab/model_runs/` and include every raw
input and output for review.

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

## Learning corpus export

Export replay-safe `raw_stt/transcript -> polished_output -> user_kept` rows into
`lab/corpus/` for offline learning-loop experiments. `lab/corpus/` is gitignored.

```bash
# Local AirNote SQLite: recordings + permanent edit_events.
python lab/export_learning_corpus.py --source local

# Dev control-plane Postgres over SSH. Password is read from env, never hardcoded.
AIRNOTE_DEV_SSH_PASSWORD='...' python lab/export_learning_corpus.py --source remote-dev --days 90 --limit 1000

# Only Shivam Bhateja's dev data.
AIRNOTE_DEV_SSH_PASSWORD='...' python lab/export_learning_corpus.py --source remote-dev --remote-email-like shivam --days 90
```

The exporter is read-only and does not mutate local SQLite, dev Postgres, or
production learning memory.

## Learning-loop replay

Run offline memory-policy simulations against an exported corpus:

```bash
python lab/learning_loop.py --policy shadow
python lab/learning_loop.py --policy repeat2
python lab/learning_loop.py --policy conservative
python lab/learning_loop.py --policy aggressive
```

Reports are written under `lab/corpus/learning_loop_runs/`, also gitignored.
Use this to compare strategies before changing the production learning pipeline.

## Model-backed learning replay

Test the real product shape: learned memory is passed to the polish model, and
the model output is compared with `user_kept`.

```bash
# Dry-run selection/report without API calls.
python lab/model_backed_learning_replay.py --dry-run --limit 10

# Prompt variants against Cerebras GPT-OSS 120B.
python lab/model_backed_learning_replay.py --variant production --slug cerebras-gpt-oss --limit 20
python lab/model_backed_learning_replay.py --variant intent_v1 --slug cerebras-gpt-oss --limit 20
python lab/model_backed_learning_replay.py --variant literal_guard --slug cerebras-gpt-oss --limit 20
```

This is the benchmark for the under-correction vs over-correction problem:
whether the polish model understands intended text from evidence without
inventing unsupported meaning.

## Memory candidate judge

Classify extracted edit candidates before they become directive memory:

```bash
python lab/memory_candidate_judge.py --corpus lab/corpus/learning_corpus_full_20260703T0931Z.jsonl
```

Labels:

- `safe_directive`: strong enough to become explicit repair memory.
- `soft_hint_only`: usable as weak context, not as a directive.
- `needs_more_evidence`: plausible, wait for more matching corrections.
- `reject`: unsafe or not a memory candidate.
