# AirNote Mobile Gateway

The iOS app's **hosted runtime**. A standalone Axum service that owns the entire
server side of AirNote mobile dictation:

- **Auth** — self-contained email/password accounts with opaque bearer tokens.
- **Voice sessions** — short-lived, device-scoped, idempotent on `client_request_id`.
- **Voice pipeline** — Deepgram STT → Groq LLM polish → Hinglish script guard.
  - Streaming (`WS /v1/runtime/voice`): live `stt.interim/final` + `polish.delta` + a single insertable `final`.
  - Batch (`POST /v1/runtime/voice/batch`): one-shot fallback for unreliable networks / Action Button.
- **Events** — privacy-safe, redacted product/setup telemetry.
- **Vocabulary** — ETag-cacheable personal vocab snapshot + explicit "learn spelling".

## Isolation (important)

This service is **intentionally NOT** part of the desktop Cargo workspace and
has **no connection to `control-plane`** (the desktop/enterprise backend). It
has its own database, its own accounts, and deploys independently. Build it
standalone:

```bash
cd crates/mobile-gateway
cargo build
cargo test          # deterministic unit tests (script guard, prompt, stt parsing)
```

## Privacy

No raw audio and no raw transcript/polished text are persisted — only character
counts and redacted metadata (`voice_runs`, `voice_events`, `provider_usage`).
Provider keys live only on the server.

## Run

```bash
DATABASE_URL=postgres://... \
DEEPGRAM_API_KEY=... \
GATEWAY_API_KEY=...        # Groq key for polish
cargo run
```

If `DEEPGRAM_API_KEY` or `GATEWAY_API_KEY` is empty the pipeline runs a
**deterministic mock** (fixed Hinglish output) so the iOS app can be exercised
end-to-end against staging without provider keys.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET  | `/v1/health` | liveness + which providers are configured |
| GET  | `/v1/mobile/bootstrap` | public config (pre-login) |
| POST | `/v1/auth/mobile-email` | signup-or-login → access + refresh tokens |
| POST | `/v1/auth/mobile-refresh` | refresh → new access token |
| GET  | `/v1/runtime/config` | authed runtime config + vocab hash |
| POST | `/v1/runtime/sessions` · `/v1/mobile/sessions` | create a voice session |
| GET  | `/v1/runtime/voice` | streaming dictation WebSocket |
| POST | `/v1/runtime/voice/batch` · `/v1/mobile/dictate` | batch dictation |
| POST | `/v1/runtime/events` · `/v1/mobile/events` | event ingestion |
| GET  | `/v1/mobile/vocab/snapshot` | personal vocab snapshot (ETag) |
| POST | `/v1/mobile/vocab/terms` | add/update a vocab term |
| POST | `/v1/mobile/feedback` | explicit learn-spelling |

## Streaming WebSocket protocol

Client → server: a `voice.start` text frame, binary 16 kHz PCM16 mono frames,
then `audio.end`. Server → client:

```
session.ready → stt.interim* → stt.final* → polish.started → polish.delta* → final → runtime.done
```

`stt.interim` and `polish.delta` are for live preview only; insert from `final`
(it has passed the Hinglish guard). Errors arrive as
`{ "type": "error", "code", "retryable", "message" }`.
