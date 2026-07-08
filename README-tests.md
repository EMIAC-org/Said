# AirNote — Test Suite Guide

## Quick start

```bash
just check          # full CI gate (fmt-check + clippy + tests + typecheck)
just e2e-stress     # rapid-recording HTTP stress (needs just dev-backend running)
```

---

## Automated tests (always-on, run by `just test`)

All tests below run via `cargo test --workspace --all-targets` with no external
dependencies or environment variables.

### Safety & PII filtering

| File | Tests | What they catch |
|---|---|---|
| `crates/core/src/reporter.rs` | 6 | All 12 PII blocked keys (individually + nested + in arrays), oversized context, safe values |
| `crates/core/src/scrub.rs` | 6 | Path redaction logic, JSON tree walk (nested objects, arrays, non-strings), null safety |

### Hang-detection instrumentation

| File | Tests | What they catch |
|---|---|---|
| `desktop/src-tauri/src/diag.rs` | 4 | SharedApp lock-holder tracking (acquire → hold → release sequence), breadcrumb ring capacity (wraps at 32), limit parameter, entry schema |

### Audio / timing helpers

| File | Tests | What they catch |
|---|---|---|
| `crates/backend/src/routes/voice.rs` (module `audio_tests`) | 7 | WAV header parser (short buffer, zero byte_rate, 1s/3s valid headers), `estimated_secs` formula (0 words, 65 words → 30s, 130 words → 60s) |

### STT output scrubbing

| File | Tests | What they catch |
|---|---|---|
| `crates/backend/src/routes/voice.rs` (module `scrub_tests`) | 6 | Confidence-marker stripping (canonical, malformed LLM leaks, non-marker brackets, unclosed bracket, multiple markers), repair-output label scrubbing |

### Session isolation

| File | Tests | What they catch |
|---|---|---|
| `crates/backend/tests/session_isolation.rs` | 3 | 1 000 UUID4 IDs are collision-free; format is valid UUID v4; 50 rapid cycles yield distinct IDs (no consecutive match) |

### Learning pipeline

| File | Tests | What they catch |
|---|---|---|
| `crates/backend/tests/learning_eval.rs` | 3 | Golden-case pre-filter + promotion gate evaluation, diff-produces-no-hunks, single-token-swap isolation |
| `crates/backend/tests/romanizer_bench.rs` | — | Devanagari→Roman benchmarks |

### Control-plane diagnostics (requires Postgres, gated env)

| File | Tests | What they catch |
|---|---|---|
| `crates/control-plane/tests/meeting_scenarios.rs` | 32 scenarios | Includes s31 (diagnostics ingest) and s32 (transcript block at server boundary) |

---

## Live API tests (opt-in, gated by env var)

```bash
RUN_GROQ_HARDENING_TESTS=1 GROQ_API_KEY=<key> \
  cargo test -p said-backend -- polish_hardening
```

| File | Tests | What they catch |
|---|---|---|
| `crates/backend/tests/polish_hardening_groq.rs` | ~21 | Prompt injection resistance, content preservation, Hinglish/Hindi/Devanagari output, RAG exemplar leak prevention, adversarial stability |

---

## HTTP stress test (needs a live backend)

```bash
# Terminal 1 — start local backend
just dev-backend

# Terminal 2 — run 50 rapid-fire cycles
just e2e-stress

# Or increase load:
CYCLES=100 ./tools/e2e-stress/run.sh
```

**What it tests:** 50 sequential `POST /v1/voice/polish` requests with a 1-second
silence WAV. Each uses a fresh UUID as the logical recording ID. Asserts HTTP 200
for every request with no crashes or hangs.

**What it does not test** (see §Manual below).

---

## Manual QA scenarios (still human-required)

These scenarios require real hardware permissions and the local speech model.
Run them before every release milestone.

### 1. Rapid Caps Lock cycling (session isolation)
**Goal:** transcript from recording N must never appear in recording N+1.

1. Open any text field (Notes, Slack, etc.).
2. Hold Caps Lock, say "alpha bravo charlie", release.
3. Immediately hold Caps Lock again (< 200 ms gap), say "delta echo foxtrot", release.
4. Repeat 10 times as fast as possible.
5. **Pass:** each dictation types only its own words, no carry-over.
6. **Check logs:** macOS Console → filter the recording id — each cycle must have a distinct `recording_id` with local transcript state starting empty.

### 2. Status bar / AppKit main-thread safety (macOS)
**Goal:** no crash or hang when the status bar is dismissed/moved while dictating.

1. Start dictating (Caps Lock held).
2. While Caps Lock is held, drag the status bar to a new position.
3. Release Caps Lock.
4. **Pass:** text is typed correctly; status bar reappears at new position; no crash.

Also test: long idle (> 5 min), then dictate → status bar must reappear.

### 3. Fleet diagnostics pipeline
**Goal:** events reach the control plane without leaking PII.

1. Trigger a backend error (e.g. set an invalid gateway URL/key for text polish).
2. Wait 45 s for the flusher to fire.
3. Check the admin diagnostics list in the control-plane UI.
4. **Pass:** event appears with no `transcript`, `api_key`, or `token` fields.
5. **Opt-out:** set `sentry_disabled: true` in desktop prefs → no events sent.

### 4. EMIAC clean-profile behavior
**Goal:** fresh install must not auto-promote phonetically similar jargon.

1. Wipe the SQLite DB (`~/Library/Application Support/airnote-backend/airnote.db`).
2. Dictate: "I work at make" / "send email to acme" / "open Mac settings".
3. **Pass:** "make", "email", "Acme", "Mac" remain unchanged — NOT auto-corrected to "EMIAC".
4. Then explicitly teach EMIAC via Settings → Vocabulary → add "EMIAC".
5. Dictate: "I work at EMIAC".
6. **Pass:** output preserves "EMIAC" exactly.

---

## Resilience & longevity (chaos harness)

The crash/hang/HUD fixes can't be trusted unless we can *trigger* each failure on
demand and watch the app recover. The chaos harness does exactly that — on the
real (release) binary, not a mock. It is **completely inert** unless
`AIRNOTE_CHAOS=1` is set at launch; production installs never set it.

### Targeted single fault (manual / devtools)

Launch with `AIRNOTE_CHAOS=1`, then invoke `chaos_inject` from the devtools
console (or a dev panel):

```js
await window.__TAURI__.core.invoke('chaos_inject', { kind: 'main_panic' })
```

| `kind`            | Reproduces                      | Expected recovery (log + dashboard)             |
|-------------------|---------------------------------|-------------------------------------------------|
| `main_panic`      | panic in an AppKit callback     | app survives; `panic.recovered`, `guard:recovered:chaos.main_panic` |
| `pipeline_panic`  | finish task dies mid-polish     | process survives (tokio-caught); `state.healed` |
| `stick_processing`| wedged state machine            | recording works again; `state.healed`, `heal:reset_idle:*` |
| `plant_orphan`    | crash mid-dictation             | **on next launch**: `recovery:attempt`/`recovery:recovered`, recovery card |
| `drop_hud`        | HUD hidden while active          | pill returns; `hud_watchdog:recover`            |
| `emit_diag`       | —                               | `chaos.test_event` appears on the dashboard     |

### Full soak (automated torture)

```bash
# Terminal 1 — app self-injects faults on a loop:
AIRNOTE_CHAOS=1 AIRNOTE_CHAOS_SOAK=1 \
AIRNOTE_CHAOS_INTERVAL=15 AIRNOTE_HEAL_STUCK_SECS=12 \
just dev

# Terminal 2 — monitor for 20 min and assert survival + heal + no leak:
DURATION=1200 just soak
```

`soak.sh` watches the process for the whole duration and **fails** if: the PID
dies, any recovery breadcrumb (`panic.recovered`, `state.healed`, `chaos:inject`)
is missing, or RSS grows beyond `RSS_GROWTH_MAX_PCT` (default 50%). Tunables:
`AIRNOTE_CHAOS_INTERVAL` (fault cadence), `AIRNOTE_HEAL_STUCK_SECS` (watchdog
threshold — lower it so heals happen in seconds, not the 60s production default).

### What automated chaos covers vs what stays manual

| Path | Automated by chaos | Still manual |
|---|---|---|
| Main-thread panic seatbelt | ✓ `main_panic` + soak | — |
| Stuck-state self-heal | ✓ `stick_processing` / `pipeline_panic` + soak | — |
| HUD watchdog re-show | ✓ `drop_hud` (breadcrumb) | visual confirm pill reappears |
| Crash-orphan recovery plumbing | ✓ `plant_orphan` (silence → empty transcript) | **word fidelity**: crash during a *real* dictation, confirm the spoken text is recovered |
| Rapid Caps-Lock hotkey hang | partial (HTTP stress, session IDs) | real CGEventTap press/release storm |
| NSPanel level loss after sleep/Space switch | — | sleep the Mac / switch Spaces, confirm pill stays |

---

## Diagnostics event map (→ https://airnote.emiactech.com/admin/diagnostics)

Every failure and recovery emits a scrubbed event through the same pipeline
(`report_event` → on-disk `queue.ndjson` → `POST /v1/diagnostics` → admin list).
Use these `event_type`s to confirm a fix fired in the field. Breadcrumbs ride in
`context.trail`.

| event_type        | severity | Emitted by                          | Meaning |
|-------------------|----------|-------------------------------------|---------|
| `panic`           | fatal    | panic hook (unguarded)              | A panic the seatbelt did **not** catch (process likely aborted). `context.summary` has the `panicked at file:line`. |
| `panic.recovered` | error    | panic hook (guarded) + `guard_panics` | A main-thread panic was caught; app survived. |
| `state.healed`    | error    | `heal_stuck_state`                  | A wedged `processing` state was reset to idle so recording works again. |
| `hotkey.queued_finish_timeout` | error | queued-finish worker        | Caps-Lock finish stalled on the lock (existing hang instrumentation). |
| `backend.spawn_failed` | error | backend spawn                       | Sidecar failed to start. |
| `tracing.error`   | error    | any `tracing::error!`               | Generic error forwarded from logs. |
| `chaos.test_event`| info     | `chaos_inject emit_diag`            | Harness probe — verifies the dashboard path end-to-end. |

Relevant breadcrumbs (in `context.trail`): `guard:recovered:*`,
`heal:reset_idle:*`, `heal:words_recovered`, `lock:poison_recovered`,
`hud_watchdog:recover`, `recovery:attempt`, `recovery:recovered`, `chaos:inject:*`.

**Verify the path:** launch with `AIRNOTE_CHAOS=1`, run `emit_diag`, wait ≤45s
for the flusher, then check the admin diagnostics list for `chaos.test_event`.
Opt-out (`sentry_disabled: true` in desktop prefs) must suppress all of the above.

---

## Adding new tests (TDD workflow)

For each new feature:
1. Create a wiki sub-page under `Said — Weekly Updates` describing the feature.
2. Write a test in the appropriate crate BEFORE writing the feature code.
3. Gate live-API tests with `#[cfg(feature = "...")]` or an env-var guard so CI
   stays fast and offline.
4. Add manual scenarios to §Manual above if full automation is not feasible.
