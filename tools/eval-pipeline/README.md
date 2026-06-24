# Said — Learning Pipeline Eval Harness

A dataset-driven test suite that validates the learning pipeline's accuracy at scale.
Runs 14,793 real Hindi/English/Hinglish transcripts through the deterministic pipeline
stages and asserts zero false injections.

## Why this exists

Users reported that Said was picking up learned vocabulary words too aggressively:

> "128 GB RAM bola, 8GB RAM likha" — Because "8GB" was saved in vocabulary  
> "RT ko 8GB liya hai isne"  
> "Dictionary wale words ko bahut zyada use karna shuru kar deta hai"  
> "Thode bahut bhi milte julte words milte hain isko to"

Root causes found:
1. **Substring matching** — `"128gb".contains("8gb")` returned true, so "8GB" entered the prompt
2. **Phonetic false positives** — short terms like "RT", "8GB" phonetically matched unrelated words at threshold 0.70
3. **Corrections over-matching** — `"there → their"` fired on "three", "through", "the" via phonetic fallback
4. **Candidate vocab injection** — unresolved vocab terms were included as "hints" and the LLM force-applied them

## What was fixed

| Bug | Fix | File |
|-----|-----|------|
| Substring match `contains()` | Whole-word token matching | `store/vocab_embeddings.rs` |
| Short-term phonetic collisions | Block phonetic match for terms < 4 chars | `store/vocab_embeddings.rs` |
| Loose phonetic threshold (0.70) | Raised to 0.80 | `store/vocab_embeddings.rs` |
| Corrections phonetic fallback | Exact match only, no phonetic | `store/corrections.rs` |
| Candidate terms in prompt | Dropped entirely, only resolved terms | `llm/prompt.rs` |
| k=2 promotion threshold | Raised to k=3 | `store/pending_promotions.rs` |
| No temporal decay | 14-day window, stale sightings reset | `store/pending_promotions.rs` |
| Weak demotion (-0.5) | Doubled to -1.0 per removal | `routes/classify.rs` |
| Demotion capped at top-200 | Expanded to 1000 | `routes/classify.rs` |
| Vocab resolver thresholds | Tightened for code_identifiers (0.60), brands (0.70), generic (0.75) | `llm/vocab_resolver.rs` |

## Architecture

The pipeline has two sides — **storage** (what gets learned) and **retrieval** (what enters the prompt).
This harness tests both, plus their interaction (pollution).

```
User edits text ──► extract_diffs ──► promotion gates ──► k-threshold ──► STORED
                                                                            │
Next recording ──► BM25 lexical gate ──► phonetic gate ──► vocab resolver ──► PROMPT
                   ▲                     ▲                  ▲
                   │                     │                  │
              whole-word only      min 4 chars, ≥0.80   tightened thresholds
```

### What the eval tests (no API calls, pure Rust + SQLite)

**Layer 1 — Retrieval sweep (14,793 transcripts)**
- Seeds 30 vocab terms (names, brands, acronyms, code identifiers, common words)
- Seeds 3 corrections (badhiya→badiya, there→their, recieve→receive)
- Runs every transcript through `select_for_prompt` → `resolve_for_prompt` → `filter_relevant`
- Asserts: 0 false injections (term entered prompt for an unrelated transcript)

**Layer 2 — Storage correctness**
- `extract_diffs`: positional alignment, word-count mismatch rejection, punctuation stripping, case handling
- K-threshold: 3 sightings required, not 2
- Temporal decay: sighting from 15 days ago resets count to 1
- Demotion: weight drops by 1.0 per removal, term deleted at weight 0

**Layer 3 — Pollution + adversarial**
- Store correction "there→their", sweep 14K transcripts — 0 pollution
- Store common words (time/can/go), sweep 14K — 0 pollution
- 10 adversarial cases:
  - `8GB` must NOT inject into "128 GB RAM hai mere laptop mein"
  - `8GB` must NOT inject into "8 ghante baad aana"
  - `RT` must NOT inject into "return the value please"
  - `RAM` must NOT inject into "ramadan mubarak bhai"
  - `API` must NOT inject into "capital city of India"
  - `can` must NOT inject into "cancer treatment is expensive"
  - `go` must NOT inject into "google search karo"
  - `PR` must NOT inject into "prayer time ho gaya"
  - `SQL` must NOT inject into "sequel to the movie"
  - `EMI` must NOT inject into "emission control system"
- 4 positive cases (terms that SHOULD inject when present):
  - `MACOBS` SHOULD inject into "MACOBS ka stock price kya hai"
  - `Anish` SHOULD inject into "Anish ko call karo please"
  - `localhost` SHOULD inject into "localhost pe server start karo"
  - `kubectl` SHOULD inject into "kubectl apply karo deployment"

### Mutation testing (proves the tests are real)

We deliberately reintroduce each fixed bug and verify the eval catches it:

| Mutation | What it breaks | Eval catches it? |
|----------|---------------|-----------------|
| M1: Revert to `.contains()` | "8GB" matches inside "128GB" | YES |
| M2: k=1 threshold | Single typo promotes | YES |
| M3: Remove min-length phonetic gate | Short terms like "RT" phonetically match random words | YES |
| M4: Loosen phonetic to 0.50 | Many unrelated words match vocab terms | YES |

If a mutation passes (eval doesn't catch it), the test suite has a blind spot.

## Dataset

14,793 transcripts from HuggingFace (text only, ~3 MB JSONL):

| Source | Language | Count |
|--------|----------|-------|
| cfilt/iitb-english-hindi | Hindi + English | ~5,000 |
| LingoIITGN/HinGE | Hinglish + Hindi + English | ~9,800 |

Downloaded via `download_transcripts.py` using the HuggingFace datasets-server parquet API (no audio, no heavy libraries).

## How to run

```bash
# One-time: download transcripts (~30 seconds, ~3 MB)
python3 tools/eval-pipeline/download_transcripts.py

# Run the eval (builds + runs, ~10 seconds)
./tools/eval-pipeline/run.sh

# Run mutation tests (proves test quality, ~2 minutes)
bash tools/eval-pipeline/mutation_test.sh

# Run the grand intake + HITL learning simulation against real local AirNote history
./tools/eval-pipeline/run-learning-intake-grand.sh
```

## Grand intake + HITL learning simulation

`run-learning-intake-grand.sh` is the end-to-end stress test for the learning
intake loop:

```
real local edit history
  -> /v1/classify-edit
  -> approve the review candidates in /v1/confirm-batch
  -> inspect vocabulary + stt_replacements
  -> run second-pass alias probes through apply_exact_safe()
```

It reads the current app DB in read-only mode:

```
~/Library/Application Support/VoicePolish/db.sqlite
```

and writes only to an isolated temporary eval DB under:

```
.context/learning-intake-grand/
```

The generated reports are:

```
.context/learning-intake-grand/latest.json
.context/learning-intake-grand/latest.md
```

Useful options:

```bash
./tools/eval-pipeline/run-learning-intake-grand.sh --max-history-cases 30
./tools/eval-pipeline/run-learning-intake-grand.sh --keep-db
./tools/eval-pipeline/run-learning-intake-grand.sh --fail-fast
./tools/eval-pipeline/run-learning-intake-grand.sh --source-db /path/to/db.sqlite
```

This eval is intentionally API-backed for classification/alias safety. Set the
same learning keys the app uses, especially `DEEPSEEK_API_KEY`, before running
it. If DeepSeek is unavailable, alias safety fails closed and the report will
show blocked aliases instead of silently passing.

### Expected output

```
══ LAYER 1: RETRIEVAL ══
  PASS  Vocab false injections = 0
  PASS  Correction false injections = 0

══ LAYER 2: STORAGE ══
  PASS  extract_diffs: ...
  PASS  k-threshold: ...
  PASS  temporal decay: ...
  PASS  demotion: ...

══ LAYER 3: POLLUTION ══
  PASS  Pollution: 'there→their' on 14K transcripts
  PASS  Pollution: common words on 14K
  PASS  Adversarial: '8GB' must NOT inject into '128 GB RAM...'
  ...
  PASS  Positive: 'MACOBS' SHOULD inject into '...'

  RESULTS: 31 passed, 0 failed
  PASS
```

## Files

```
tools/eval-pipeline/
├── README.md                  ← this file
├── download_transcripts.py    ← fetches transcripts from HuggingFace
├── transcripts.jsonl          ← 14,793 transcripts (gitignored, regenerate with above)
├── vocab_seed.json            ← 30 vocab terms for testing
├── run.sh                     ← builds + runs the eval binary
└── mutation_test.sh           ← proves test quality via deliberate breakage

crates/backend/src/bin/
└── eval_pipeline.rs           ← the Rust eval binary (3-layer test suite)
```

## Adding new test cases

**New adversarial case** (term should NOT match):
Add to the `adversarial_cases` vec in `eval_pipeline.rs`:
```rust
("NEW_TERM", "transcript where it should NOT appear"),
```

**New positive case** (term SHOULD match):
Add to `positive_cases`:
```rust
("NEW_TERM", "transcript where it SHOULD appear"),
```

**New vocab seed**:
Add to `vocab_seed.json`:
```json
{"term": "NewTerm", "type": "proper_noun", "meaning": "What it is", "context": "Example usage", "source": "auto"}
```

**New mutation**:
Add to `mutation_test.sh`:
```bash
run_mutation \
    "Description of what breaks" \
    "path/to/file.rs" \
    "correct code" \
    "broken code"
```

## Design decisions

1. **No API calls** — The entire eval runs locally against SQLite. The bugs live in the deterministic pipeline stages (lexical gate, phonetic matching, vocab resolution, corrections filtering), not in the LLM. Testing these doesn't need Groq/Gemini.

2. **Real transcripts, not synthetic** — Synthetic test data misses the diversity of real speech (numbers mixed with Hindi, common English words in Hindi sentences, abbreviations in conversational context). The HuggingFace datasets cover this naturally.

3. **Mutation testing over coverage metrics** — Code coverage tells you "this line ran" but not "this test would catch a regression." Mutation testing proves each test has teeth by deliberately breaking the code and verifying the test fails.

4. **Adversarial cases from user reviews** — Every user complaint becomes a permanent regression test. "128 GB → 8GB" is now in the test suite forever.
