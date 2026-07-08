# Memory Directive Pipeline Findings

Last updated: 2026-07-03

This file tracks working findings from the AirNote learning-memory lab. It is lab-only and should not be treated as production implementation status.

## Current Verdict

The strongest architecture found so far:

```text
raw STT
  -> retrieve transcript-relevant learned repair memory
  -> filter aggressively with context gates
  -> render validated repairs as explicit user-message directives immediately before the transcript
  -> polish model produces final text
```

This is materially better than putting memory somewhere in the system prompt.

## Breakthrough Finding

The polish model was not reliably using retrieved memory when memory was only included in the system prompt. Moving the validated repair memory into the user message, adjacent to the transcript, made the model obey the repairs much more reliably.

Measured on mixed corpus 40-case run:

| Setup | Eligible repair targets hit |
|---|---:|
| Prompt-only memory | 19 / 26 = 73.1% |
| User-message repair directives | 24 / 26 = 92.3% |
| User-message directives after bad-gate cleanup | 21 / 21 = 100% |

Interpretation: the model can use memory, but the instruction shape matters. "Soft hints in system prompt" are too weak. "Validated repair directives in user message right before transcript" are much stronger.

## Retrieval Quality

After phonetic + fuzzy + context-aware retrieval cleanup:

| Corpus | Target recall |
|---|---:|
| Mixed local + dev corpus | top-5: 95.8%, top-10: 100% |
| Shivam-only export | too few eligible repeated-domain cases for strong stats |

Shivam-only had only one eligible repeated-domain retrieval gold case in the exported slice, so it is useful for manual examples but weak for statistical confidence.

## Evidence Reports

- Retrieval eval: `lab/corpus/retrieval_eval_runs/retrieval_eval_20260703T081638Z.md`
- 40-case prompt-only run: `lab/corpus/model_replay_runs/model_replay_intent_v4_cerebras-gpt-oss_20260703T090041Z.md`
- 40-case directive run after cleanup: `lab/corpus/model_replay_runs/model_replay_intent_v4_cerebras-gpt-oss_20260703T090501Z.md`
- Shivam-only prompt/directive comparison:
  - `lab/corpus/model_replay_runs/model_replay_intent_v4_cerebras-gpt-oss_20260703T085859Z.md`
  - `lab/corpus/model_replay_runs/model_replay_intent_v4_cerebras-gpt-oss_20260703T085908Z.md`

## Working Examples

These memory repairs worked when rendered as user-message directives:

- `GROC` / `growc` -> `Groq`
- `cerebrace` / `sharibra` -> `Cerebras`
- `AMEAC` / `MIA` / `MBI` -> `Emiac`
- `MECOPS` / `MACOPS` / `mere cops` -> `Macobs`
- `D0` / `D ko` -> `Divo`
- `ear note` -> `AirNote`
- `Doctor rebuild` -> `Docker rebuild`
- `century run ID` -> `Sentry run ID`
- `piettors` -> `PyTorch`
- `dust of changes` -> `desktop changes`

## Important Failure Mode

The model now obeys directives strongly. That means bad memory or bad retrieval becomes more dangerous.

Bad directives found during manual inspection:

- `site` -> `SQLite`
  - Wrong in "fine tuning waali site/side jaana".
  - Fix: SQLite now requires DB/schema/migration/table/sql context.
- `log.` -> `Lark`
  - Wrong in generic Hinglish "ham log".
  - Fix: Lark now requires CLI/thread/message/files/docs/conversation context.
- `deaf` -> `Docker`
  - Wrong in "dev container".
  - Fix: Docker no longer triggers from "container" alone; requires build/compose/docker/image/migration/rebuild/runtime context.
- `century` -> `Sentry` can be wrong outside error/log/run context.
  - Fix: Sentry requires crash/error/event/exception/log/monitoring/panic/run context.

Conclusion: directive compliance is good, but memory quality and retrieval gating are now the bottleneck.

## Current Confidence

For fixing "model saw memory but ignored it": 8.5 / 10.

For end-to-end AirNote quality improvement today: 6 / 10.

Reason: user-message directives solve the obedience layer, but the system still needs robust memory storage, bad-memory filtering, edit-watch correctness, and a better intent/style judge.

## Architecture Rule

Do not ship broad auto-replacement from memory.

Preferred production direction:

```text
1. Store candidate memory only after strong evidence.
2. Retrieve only transcript-relevant candidate memory.
3. Canonicalize targets before prompting.
4. Filter by target-specific context gates.
5. Put surviving repairs into the user message as explicit directives.
6. Let polish model handle wording/style.
7. Add telemetry for:
   - retrieved memory count
   - directive count
   - final output target-hit status
   - user edit after directive
```

## Still Unsolved

- User-kept text is useful evidence but not perfect truth.
- Edit watcher can miss or misclassify corrections because of app switching, focus changes, delayed edits, and partial selections.
- Memory storage must distinguish obvious STT repair from user rewrite.
- Some user-kept corrections are lazy or partial; judging only by string similarity is misleading.
- Polish model can still over-style, under-style, or miss intended message shape even after term repairs are correct.
- Need a judge layer for term correctness + intent preservation, not just similarity-to-user-kept.

## 2026-07-03: Memory Candidate Judge

Added lab-only storage-quality judge:

- Script: `lab/memory_candidate_judge.py`
- Full corpus: `lab/corpus/learning_corpus_full_20260703T0931Z.jsonl`
- Report: `lab/corpus/memory_judge_runs/memory_candidate_judge_20260703T093401Z.md`

The full read-only corpus now includes:

| Source | Rows |
|---|---:|
| Local SQLite recordings | 5 |
| Local SQLite edit events | 130 |
| Dev runtime history | 2,010 |
| Prod runtime history | 1,489 |
| Total | 3,634 |

Judge output on 1,553 extracted correction candidate pairs:

| Label | Count |
|---|---:|
| `safe_directive` | 13 |
| `soft_hint_only` | 34 |
| `needs_more_evidence` | 28 |
| `reject` | 1,478 |

Interpretation: a production-safe memory system should be extremely selective. Most observed user edits are not safe directive memory.

### First Safe Directive Set

The judge currently marks these as safe directive candidates:

- `click up` -> `ClickUp`
- `local speech` -> `Local speech`
- `air note` -> `AirNote`
- `ear note` -> `AirNote`
- `webbook` -> `webhook`
- `groc` -> `Groq`
- `cafka` -> `Kafka`
- `kodex` -> `Codex`
- `the click up` -> `ClickUp`
- `d ko` -> `Divo`
- `n 10` -> `n8n`
- `mia` -> `Emiac`
- `lax` -> `Lark`

Manual read: this list is broadly sane for a first pass because each item is repeated and/or high-signal with matching domain context.

### Soft / Needs More Evidence Examples

These are plausible but not safe enough yet:

- `doctor` -> `Docker`: repeated, but dangerous because `dev/deaf/container` can be confused with Docker.
- `macops` / `mecops` -> `Macobs`: likely correct but currently only one strong occurrence each.
- `ameac` / `mbi` -> `Emiac`: likely correct but single-user/single-context.
- `sharibras`, `cerebrace`, `suri brothers` -> `Cerebras`: plausible but needs more repeated evidence/context.
- `century` -> `Sentry`: dangerous outside error/log/run context; repeated but mixed context.
- `laakh` / `large` -> `Lark`: plausible only with CLI/message/thread context.
- `mere cops` -> `Macobs`: plausible but needs repeated evidence.

### Storage Rule Learned

Do not store phrase-level corrections as directive memory unless the target canonicalizes to one known term.

Examples rejected/demoted:

- `ClickUp n8n` as one memory target: should become separate term memories, not one phrase directive.
- `desktop Hermes`: phrase target, not safe as a generic directive.
- `Postgres function`: phrase target, should not become a global alias.

### Updated Risk

The directive pipeline has high obedience. Therefore the storage gate must be stricter than the retrieval gate.

Current production intuition:

```text
safe_directive:
  repeated/high-signal + target-specific context + canonical single target

soft_hint_only:
  plausible term correction, but single occurrence or weaker context

needs_more_evidence:
  plausible but risky; keep as candidate, wait for another matching correction

reject:
  common word, broad phrase, wrong context, or user rewrite
```

## Useful Commands

```bash
# Retrieval-only quality
python3 lab/model_backed_learning_replay.py --eval-retrieval --limit 500 --warmup 25 --retrieval-top-k 10

# Prompt-only memory run
python3 lab/model_backed_learning_replay.py --variant intent_v4 --limit 40 --warmup 25 --eval-offset 0

# User-message directive run
python3 lab/model_backed_learning_replay.py --variant intent_v4 --limit 40 --warmup 25 --eval-offset 0 --repair-directives-in-user

# Shivam-only comparison
python3 lab/model_backed_learning_replay.py --corpus lab/corpus/learning_corpus_remote-dev_20260702T213545Z.jsonl --variant intent_v4 --limit 20 --warmup 10 --eval-offset 0
python3 lab/model_backed_learning_replay.py --corpus lab/corpus/learning_corpus_remote-dev_20260702T213545Z.jsonl --variant intent_v4 --limit 20 --warmup 10 --eval-offset 0 --repair-directives-in-user

# Full read-only corpus export from local + dev + prod
env AIRNOTE_SSH_PASSWORD='...' python3 lab/export_learning_corpus.py --source all --days 180 --limit 10000 --out lab/corpus/learning_corpus_full_YYYYMMDD.jsonl

# Judge candidate memory storage quality
python3 lab/memory_candidate_judge.py --corpus lab/corpus/learning_corpus_full_20260703T0931Z.jsonl
```

## 2026-07-03: Overfit Gate Correction

Important correction: a previous lab replay briefly reached high learnable recall by adding target-specific retrieval gates such as `Lark`-specific handling for `laakh/lakh/lax` and target-specific threshold relaxations for a few terms. That result should not be treated as production evidence.

After removing target-specific rescue gates from the directive replay path and keeping only generic gates, the honest baseline is:

| Replay | Directives | Wrong directives | Learnable recall | Model target hit |
|---|---:|---:|---:|---:|
| Generic retrieval before precision cleanup | 33 | 27 / 33 = 81.8% | 4 / 10 = 40.0% | not run |
| Generic retrieval after acronym/case cleanup + no `target-char` directives | 4 | 0 / 4 = 0.0% | 4 / 10 = 40.0% | 4 / 4 = 100.0% |

Current report:

- Storage-only: `lab/corpus/memory_policy_runs/memory_policy_replay_20260703T113922Z.md`
- Model-backed: `lab/corpus/memory_policy_runs/memory_policy_replay_20260703T113947Z.md`

Interpretation:

- The strong part is still valid: when a correct directive is placed next to the transcript, the polish model follows it.
- Generic fuzzy/phonetic expansion is not safe enough yet. It produced false directives like ordinary `mac/tax/i am` becoming `Emiac` and `code/code x` becoming `Codex`.
- Precision can be restored by generic gates:
  - acronym memories only fire on acronym-like uppercase/digit spans in the original transcript
  - Devanagari plus Roman-tail phonetic matches are blocked
  - `target-char` retrieval is disabled for directives because it overfires on ordinary words that resemble a target
- The cost is low recall. The system currently catches exact/source-like learned aliases such as `growc -> Groq`, `MBI -> Emiac`, `ear note -> AirNote`, and `air note -> AirNote`, but misses useful variants like `Grop -> Groq`, `AMEAC -> Emiac`, `Laakh -> Lark`, `NEK -> Emiac`, and `anitain -> n8n`.

Production implication:

Do not ship a dictionary of term-specific `if target == ...` gates. The next lab milestone must build dynamic context profiles from user/vocabulary evidence:

```text
term memory = canonical term + learned aliases + positive contexts + negative contexts + ambiguity score
retrieval = source similarity + case/acronym signal + context overlap + ambiguity penalty
directive = only if combined confidence passes a high precision threshold
```

The lab is now measuring a stricter, more realistic baseline: safe but low-recall. Any future recall improvement must improve this generic/dynamic scorer, not add one-off gates for specific words.

## 2026-07-03: Dynamic Memory Profile Replay

Added a separate lab harness:

- Script: `lab/dynamic_memory_profile_replay.py`
- Storage-only report: `lab/corpus/dynamic_memory_runs/dynamic_memory_profile_replay_20260703T120209Z.md`
- Model-backed report: `lab/corpus/dynamic_memory_runs/dynamic_memory_profile_replay_20260703T120224Z.md`

What it tests:

```text
past rows only
  -> build alias/term profiles from observed corrections
  -> retrieve with one generic scorer
  -> generate dynamic prompt directives
  -> update positive/negative evidence after current row
```

The scorer uses generic evidence only:

- exact/source-surface similarity
- target-surface similarity only with learned context and strict span/length gates
- acronym case-signal gating for very short acronym-like aliases
- learned positive context overlap
- learned negative context penalty
- unique evidence count and account count

No `if target == "Lark"` / `if target == "Groq"` style rescue gate is used in this replay path.

Best current dynamic-profile result:

| Run | Directives | Wrong directives | Learnable recall | Model target hit |
|---|---:|---:|---:|---:|
| Storage-only dynamic profile | 3 | 0 / 3 = 0.0% | 3 / 7 = 42.9% | not run |
| Model-backed dynamic profile | 3 | 0 / 3 = 0.0% | 3 / 7 = 42.9% | 3 / 3 = 100.0% |

Correctly emitted examples:

- `growc -> Groq`
- `Grop -> Groq`
- `MBI -> Emiac`

Still missed:

- `AMEAC -> Emiac`: known term exists, but learned context from prior `MIA -> Emiac` examples is IPO/data-oriented, while current context is vocabulary/STT. Need better term-profile evidence before applying.
- `Laakh -> Lark`: has `CLI` context, but source/target similarity remains weak under the generic scorer. A lower weak-target threshold caused false positives like `deep -> Local speech`.
- `NEK -> Emiac`: too weak without explicit alias evidence.
- `anitain -> n8n`: too weak without explicit alias evidence.

Failed relaxation worth remembering:

- Lowering weak target-surface matching recovered some candidates but caused false positives like `deep/deep and -> Local speech` and `the stt -> STT`.
- Adding a generic length/span gate fixed those false positives but did not recover the harder misses.

Current verdict:

```text
Obedience layer: good.
Dynamic memory retrieval: safe at low recall.
Weak phonetic/target-surface expansion: not safe enough yet.
```

Next useful lab step:

Build a candidate generator that proposes low-confidence dynamic rules for review/evaluation, but does not emit them as directives until confirmed by another independent signal:

- explicit user vocabulary/profile term metadata
- repeated alias evidence
- stronger local context overlap
- better phonetic model for Hinglish spellings
- model-side judge that decides whether a proposed rule is contextually valid before prompting the polish model
