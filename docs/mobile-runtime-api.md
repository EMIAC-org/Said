# AirNote Mobile Runtime API Draft - Wave 15

Draft status: repo-only implementation draft, not wiki source of truth yet.

Source files checked:
- `crates/control-plane/src/lib.rs`
- `crates/control-plane/src/routes/runtime.rs`
- `crates/control-plane/src/routes/runtime_history.rs`

Audience: mobile UI/client teammate. This document describes the server contract that exists now in the control-plane. It does not describe future-only routes except where explicitly marked pending.

## Base Contract

Base URL:
- Production: `https://airnote.emiactech.com`
- Local/dev: whichever control-plane host is running.

All JSON endpoints use:

```http
Content-Type: application/json
Accept: application/json
```

Authenticated runtime endpoints require:

```http
Authorization: Bearer <token>
```

The bearer token can be either:
- a session UUID returned by email auth routes, or
- a signed JWT accepted by the control-plane auth layer.

WebSocket runtime endpoints authenticate with a query parameter:

```text
?token=<same-session-or-jwt-token>
```

Error bodies generally include `error` and/or `message`:

```json
{
  "error": "invalid or expired token",
  "message": "invalid or expired token"
}
```

## Auth

Mobile should use the existing email auth flow unless a platform-native auth flow is added later.

### POST `/v1/auth/signup`

Creates an account, creates a free license, and returns a 30-day session token.

Request:

```json
{
  "email": "user@example.com",
  "password": "at-least-8-chars"
}
```

Response:

```json
{
  "token": "session-uuid",
  "account": {
    "id": "account-uuid",
    "email": "user@example.com",
    "license_tier": "free"
  }
}
```

### POST `/v1/auth/login`

Same request/response shape as signup.

### POST `/v1/auth/desktop-email`

Also usable by mobile if the product wants the org-membership behavior currently used by desktop email auth.

Request:

```json
{
  "email": "user@example.com",
  "password": "at-least-8-chars",
  "signup": false
}
```

Response is the same `AuthResponse` shape.

### GET `/v1/auth/me`

Response:

```json
{
  "account": {
    "id": "account-uuid",
    "email": "user@example.com"
  },
  "license": {
    "tier": "free",
    "active": true,
    "features": {}
  }
}
```

### POST `/v1/auth/logout`

Deletes all sessions for the account. Response: `204 No Content`.

## Runtime Status

### GET `/v1/runtime/status`

Use this after login to decide whether the server has credentials and memory available.

Response:

```json
{
  "credential_encryption_configured": true,
  "active_credential_count": 2,
  "runtime_session_count": 10,
  "learning_event_count": 4,
  "personal_replacement_count": 3,
  "personal_vocab_count": 5,
  "personal_alias_count": 3,
  "active_edit_policy_count": 1,
  "server_memory_ready": true
}
```

## Credentials Vault

The current server can store encrypted BYOK/provider credentials. Secrets are encrypted with `RUNTIME_CREDENTIALS_KEY` using AES-256-GCM; clients never receive the secret back.

Supported `provider` values:
- `local_speech`
- `groq`
- `openai`
- `gemini`
- `gateway`

Supported `scope` values:
- `user`
- `org`
- `airnote_managed`

If no saved provider credential exists, runtime may fall back to server env keys for `local_speech` and `groq`.

### GET `/v1/runtime/credentials`

Response:

```json
[
  {
    "id": "credential-uuid",
    "provider": "local_speech",
    "scope": "user",
    "org_id": null,
    "account_id": "account-uuid",
    "display_name": "Local speech",
    "secret_last4": "abcd",
    "status": "active",
    "validated_at": "2026-06-08T12:00:00Z",
    "last_used_at": null,
    "last_error": null,
    "created_at": "2026-06-08T12:00:00Z",
    "updated_at": "2026-06-08T12:00:00Z"
  }
]
```

### POST `/v1/runtime/credentials`

Request:

```json
{
  "provider": "local_speech",
  "secret": "provider-api-key",
  "scope": "user",
  "org_id": null,
  "display_name": "Local speech"
}
```

Response: one `CredentialSummary`.

Validation:
- `secret` must be at least 8 characters.
- `org_id` is required for `scope: "org"`.
- user must be a member of the org for org-scoped credentials.

### POST `/v1/runtime/credentials/:id/validate`

Current implementation decrypts the secret and marks it active if it is non-empty. It does not perform a live provider API probe yet.

Response: one `CredentialSummary`.

### DELETE `/v1/runtime/credentials/:id`

Revokes the credential. Response: `204 No Content`.

## Runtime Polish Flow

There are three current runtime paths:
- transcript-only polish: `POST /v1/runtime/voice/polish`
- WAV batch STT + polish: `POST /v1/runtime/voice/wav`
- streaming PCM voice runtime: `WS /v1/runtime/voice/ws`

The implemented server flow is:

1. Create a `runtime_sessions` row.
2. For audio routes, transcribe with Local speech `whisper.cpp`.
3. Apply server number formatting before prompt assembly.
4. Build a literal dictation normalizer prompt.
5. Polish with Groq:
   - `selected_model: "fast"` -> `llama-3.1-8b-instant`
   - `selected_model: "smart"` -> `meta-llama/llama-4-scout-17b-16e-instruct`
6. Restore protected literal/product-like tokens.
7. Apply number formatting and email recovery after polish.
8. Apply exact resolver from server memory for safe STT replacements only.
9. Mark the runtime session completed and write history.

Important current behavior:
- Raw audio is not persisted by the server routes.
- Runtime ledgers store hashes and metadata, not raw transcript/audio by default.
- History endpoints do store transcript/output/final text for signed-in users.
- `safe_vocab_terms` are hints only; the server also merges in server-side personal vocab.
- `screen_context` is clipped and should be treated as sensitive. Mobile should omit it unless a future mobile UX intentionally sends visible context.

### POST `/v1/runtime/voice/polish`

Use this when mobile already has a transcript and wants server polish.

Request:

```json
{
  "transcript": "Macobs ka pachas percent growth hai",
  "output_language": "hinglish",
  "selected_model": "fast",
  "screen_context": null,
  "safe_vocab_terms": ["Macobs"],
  "client_run_id": "mobile-run-001"
}
```

Defaults:
- `output_language`: `hinglish`
- `selected_model`: `fast`
- `safe_vocab_terms`: `[]`

Response:

```json
{
  "run_id": "server-runtime-uuid",
  "output": "Macobs ka 50% growth hai.",
  "model_used": "llama-3.1-8b-instant",
  "prompt_version": "server-runtime-probe-2026-06-07-literal-fidelity",
  "latency_ms": {
    "prompt": 1,
    "model": 340,
    "total": 360
  }
}
```

Known errors:
- `400` when `transcript` is empty.
- `503` when server Groq key is not configured.
- `502` for model/provider failures.

### POST `/v1/runtime/voice/wav`

Use this for a simple mobile integration before WebSocket streaming. Send a complete WAV file as base64.

Request:

```json
{
  "wav_b64": "base64-wav-bytes",
  "output_language": "hinglish",
  "selected_model": "fast",
  "screen_context": null,
  "safe_vocab_terms": ["Macobs"],
  "client_run_id": "mobile-run-002",
  "device_id": "ios-device-id",
  "platform": "ios",
  "app_version": "0.1.0"
}
```

Response:

```json
{
  "run_id": "server-runtime-uuid",
  "transcript": "Macobs ka pachas percent growth hai",
  "transcript_hash": "sha256-hex",
  "output": "Macobs ka 50% growth hai.",
  "model_used": "llama-3.1-8b-instant",
  "prompt_version": "server-runtime-wav-probe-2026-06-07",
  "latency_ms": {
    "stt": 900,
    "polish": 350,
    "total": 1300
  }
}
```

Known errors:
- `400` when `wav_b64` is invalid or empty.
- `503` when Local speech/Groq credentials are missing.
- `502` when Local speech batch STT fails.

### WS `/v1/runtime/voice/ws?token=<token>`

Use this for low-latency mobile dictation. The server accepts JSON messages and binary PCM frames.

Connection welcome:

```json
{
  "type": "runtime.connected",
  "version": 1,
  "account_id": "account-uuid",
  "email": "user@example.com",
  "audio_runtime": "local_speech_mvp"
}
```

Start message:

```json
{
  "type": "voice.start",
  "run_id": "mobile-run-003",
  "mode": "normal_voice",
  "selected_model": "fast",
  "output_language": "hinglish",
  "screen_context": null,
  "safe_vocab_terms": ["Macobs"],
  "device_id": "ios-device-id",
  "platform": "ios",
  "app_version": "0.1.0",
  "audio": {
    "encoding": "linear16",
    "sample_rate": 16000,
    "channels": 1
  }
}
```

Audio frames can be sent either as JSON:

```json
{
  "type": "audio.frame",
  "pcm_b64": "base64-linear16-pcm"
}
```

or as raw WebSocket binary frames containing linear16 PCM bytes.

End message:

```json
{
  "type": "audio.end"
}
```

Ping:

```json
{
  "type": "ping"
}
```

Pong:

```json
{
  "type": "pong",
  "version": 1,
  "client_run_id": "mobile-run-003"
}
```

Runtime status event:

```json
{
  "type": "runtime.status",
  "version": 1,
  "run_id": "server-runtime-uuid",
  "client_run_id": "mobile-run-003",
  "phase": "stt_connected"
}
```

Other current `phase` values:
- `stt_connected`
- `polishing`

Transcript partial event:

```json
{
  "type": "transcript.partial",
  "version": 1,
  "run_id": "server-runtime-uuid",
  "client_run_id": "mobile-run-003",
  "text": "Macobs ka"
}
```

Transcript final event:

```json
{
  "type": "transcript.final",
  "version": 1,
  "run_id": "server-runtime-uuid",
  "client_run_id": "mobile-run-003",
  "text": "Macobs ka pachas percent growth hai"
}
```

Done event:

```json
{
  "type": "runtime.done",
  "version": 1,
  "run_id": "server-runtime-uuid",
  "client_run_id": "mobile-run-003",
  "output": "Macobs ka 50% growth hai.",
  "transcript_hash": "sha256-hex",
  "model_used": "llama-3.1-8b-instant",
  "latency_ms": {
    "stt": 900,
    "polish": 350,
    "total": 1250
  }
}
```

Warning event:

```json
{
  "type": "runtime.warning",
  "version": 1,
  "client_run_id": "mobile-run-003",
  "message": "invalid audio.frame pcm_b64"
}
```

Error event:

```json
{
  "type": "runtime.error",
  "version": 1,
  "run_id": "server-runtime-uuid",
  "client_run_id": "mobile-run-003",
  "error_kind": "local_speech_connect_failed",
  "status": 503,
  "message": "failed to connect to Local speech"
}
```

Current `error_kind` values observed in implementation:
- `recording_already_active`
- `runtime_session_create_failed`
- `local_speech_credential_missing`
- `local_speech_connect_failed`
- `empty_transcript`
- `polish_failed`

Audio expectations:
- Linear16 PCM is the implemented path.
- Default sample rate is `16000`.
- Server clamps sample rate to `8000..48000`.
- Channels default to `1`; Local speech connection is currently opened with `channels=1`.

Mobile UI contract:
- Show live partial transcript from `transcript.partial`.
- Replace/append confirmed text from `transcript.final`.
- Show polish progress on `phase: "polishing"`.
- Insert or display final text only after `runtime.done`.
- On `runtime.error`, stop recording UI, preserve any local draft, and offer retry.

## Notification WebSocket

### WS `/v1/runtime/notifications/ws?token=<token>`

This is the current account-scoped notification channel. It is useful for learning confirmations and future runtime UI nudges.

Connection welcome:

```json
{
  "type": "notification.connected",
  "version": 1,
  "account_id": "account-uuid",
  "email": "user@example.com"
}
```

Ping:

```json
{
  "type": "ping"
}
```

Pong:

```json
{
  "type": "pong",
  "version": 1
}
```

Notification payloads are emitted as:

```json
{
  "type": "vocab-learned",
  "payload": {
    "term": "Macobs",
    "message": "Saved 1 correction"
  }
}
```

The notification envelope is intentionally generic:
- `type`: string
- `payload`: arbitrary JSON object

Current server-generated notification:
- `vocab-learned`, emitted after `POST /v1/runtime/learning/confirm-batch` accepts at least one item.

`POST /v1/runtime/client-events` can also emit a caller-provided notification object.

## Learning Analyze and Confirm

Mobile should treat learning as an async follow-up. Do not block text insertion on learning analysis.

### POST `/v1/runtime/learning/analyze-edit`

Use after the user edits AI output. The server compares the pasted output with the kept text and returns learnable candidates.

Request:

```json
{
  "recording_id": "mobile-recording-001",
  "transcript": "mac ops ka growth hai",
  "ai_output": "Mac ops ka growth hai.",
  "user_kept": "Macobs ka growth hai.",
  "candidates": []
}
```

If `candidates` is empty, the server generates deterministic candidates and also asks the learning judge model when available. If candidates are provided, the server validates/refines them.

Response:

```json
{
  "candidates": [
    {
      "original": "Mac ops",
      "corrected": "Macobs",
      "term_type": "proper_noun",
      "learnable": true,
      "tag": "user_edit_span"
    }
  ],
  "changed": true,
  "source": "server_llm_learning_judge"
}
```

Possible `source` values in current code:
- `server_deterministic_alignment`
- `server_llm_learning_judge`

### POST `/v1/runtime/learning/confirm-batch`

Use when the user confirms one or more learning candidates.

Request:

```json
{
  "recording_id": "mobile-recording-001",
  "items": [
    {
      "original": "Mac ops",
      "corrected": "Macobs",
      "term_type": "proper_noun"
    }
  ]
}
```

Response:

```json
{
  "learned_count": 1,
  "blocked_count": 0,
  "learned_terms": ["Macobs"],
  "server_judgment": {
    "status": "accepted",
    "accepted_terms": 1,
    "accepted_aliases": 1,
    "blocked_terms": 0,
    "blocked_aliases": 0,
    "ignored": 0,
    "reasons": []
  }
}
```

Server behavior:
- Inserts accepted terms into `personal_vocab_terms`.
- Inserts accepted aliases into `personal_stt_replacements`.
- Inserts or increments edit-policy candidates.
- Blocks common words, identical pairs, formatter-only memory, unsupported term types, and long terms.
- Emits `vocab-learned` over the notification WebSocket when at least one item is learned.

Allowed term types:
- `brand`
- `acronym`
- `code_identifier`
- `proper_noun`

### POST `/v1/runtime/client-events`

General event ingestion. This is also the route that can upsert learning memory when `event_type` is `classify_edit_result` and `payload.learned` is `true`.

Request:

```json
{
  "event_type": "classify_edit_result",
  "client_run_id": "mobile-run-003",
  "recording_id": "mobile-recording-001",
  "run_id": "server-runtime-uuid",
  "classification": "STT_ERROR",
  "input_hash": "sha256-hex",
  "output_hash": "sha256-hex",
  "corrected_hash": "sha256-hex",
  "payload": {
    "learned": true,
    "memory": {
      "accepted_terms": [
        {
          "term": "Macobs",
          "term_type": "proper_noun",
          "weight": 1.0,
          "source": "mobile_learning"
        }
      ],
      "accepted_aliases": [
        {
          "transcript_form": "Mac ops",
          "correct_form": "Macobs",
          "edit_type": "replace",
          "source": "mobile_learning"
        }
      ]
    }
  },
  "notification": {
    "type": "vocab-learned",
    "payload": {
      "term": "Macobs",
      "message": "Saved 1 correction"
    }
  }
}
```

Response:

```json
{
  "stored": true,
  "notified": true
}
```

Recommendation for mobile:
- Prefer `analyze-edit` then `confirm-batch` for user-visible correction review.
- Use `client-events` for lower-level telemetry/learning events only when the UI already has validated memory payloads.

## History Flow

History stores signed-in user transcript/output/edit text. Raw audio and screen context are not stored by these endpoints.

### GET `/v1/runtime/history`

Query parameters:
- `limit`: default `50`, clamped to `1..200`
- `before`: ISO timestamp cursor; returns rows older than this timestamp
- `include_deleted`: default `false`

Response: array of `RuntimeHistoryItem`.

```json
[
  {
    "id": "history-uuid",
    "account_id": "account-uuid",
    "org_id": "org-uuid",
    "run_id": "server-runtime-uuid",
    "client_run_id": "mobile-run-003",
    "recording_id": "mobile-recording-001",
    "device_id": "ios-device-id",
    "platform": "ios",
    "app_version": "0.1.0",
    "source": "server_wav",
    "raw_transcript": null,
    "transcript": "Macobs ka pachas percent growth hai",
    "local_corrected_transcript": null,
    "polished_output": "Macobs ka 50% growth hai.",
    "final_text": "Macobs ka 50% growth hai.",
    "model_used": "llama-3.1-8b-instant",
    "word_count": 5,
    "recording_seconds": null,
    "transcribe_ms": 900,
    "embed_ms": null,
    "polish_ms": 350,
    "target_app": null,
    "formatter_trace_json": {},
    "resolver_trace_json": {},
    "edit_feedback_json": {},
    "privacy_json": {},
    "created_at": "2026-06-08T12:00:00Z",
    "updated_at": "2026-06-08T12:00:00Z",
    "deleted_at": null
  }
]
```

### GET `/v1/runtime/history/:id`

Returns one `RuntimeHistoryItem` owned by the authenticated account.

### PATCH `/v1/runtime/history/:id`

Request:

```json
{
  "final_text": "Edited final text",
  "edit_feedback_json": {
    "source": "mobile_manual_edit"
  },
  "deleted": false
}
```

All fields are optional. `deleted: true` soft-deletes; `deleted: false` restores.

Response: updated `RuntimeHistoryItem`.

### DELETE `/v1/runtime/history/:id`

Soft-deletes the item. Response: `204 No Content`.

### POST `/v1/runtime/history/sync`

Batch upsert from a client-local history store.

Request:

```json
{
  "items": [
    {
      "client_run_id": "mobile-run-003",
      "recording_id": "mobile-recording-001",
      "source": "mobile_sync",
      "raw_transcript": null,
      "transcript": "Macobs ka pachas percent growth hai",
      "local_corrected_transcript": null,
      "polished_output": "Macobs ka 50% growth hai.",
      "final_text": "Macobs ka 50% growth hai.",
      "model_used": "llama-3.1-8b-instant",
      "word_count": 5,
      "recording_seconds": 3.2,
      "transcribe_ms": 900,
      "embed_ms": null,
      "polish_ms": 350,
      "target_app": "mobile_keyboard",
      "created_at": "2026-06-08T12:00:00Z",
      "device_id": "ios-device-id",
      "platform": "ios",
      "app_version": "0.1.0",
      "edit_feedback_json": {}
    }
  ]
}
```

Response:

```json
{
  "accepted": 1,
  "skipped": 0,
  "failed": 0
}
```

Notes:
- Empty batches return all zero counts.
- `source` defaults to `desktop_sync` if omitted; mobile should explicitly send `mobile_sync`.
- `created_at` falls back to server time if invalid or omitted.
- `word_count` is inferred from `final_text` or `polished_output` if omitted.

## Memory Sync

### POST `/v1/runtime/memory/sync`

Batch sync personal memory from a client.

Request:

```json
{
  "vocab_terms": [
    {
      "term": "Macobs",
      "term_type": "proper_noun",
      "weight": 1.0
    }
  ],
  "stt_replacements": [
    {
      "transcript_form": "Mac ops",
      "correct_form": "Macobs",
      "edit_type": "replace"
    }
  ],
  "edit_policy_rules": [
    {
      "variant_form": "Mac ops",
      "correct_form": "Macobs",
      "edit_type": "replace"
    }
  ],
  "email_memory": [
    {
      "email": "person@example.com"
    }
  ]
}
```

Response:

```json
{
  "accepted_vocab": 1,
  "accepted_aliases": 1,
  "accepted_policies": 1,
  "accepted_emails": 1,
  "blocked_vocab": 0,
  "blocked_aliases": 0,
  "skipped": 0
}
```

Server blocks:
- empty terms/pairs
- common words
- identical alias pairs
- terms/pairs longer than 4 words
- unsupported term types

## Runtime Runs and Learning Event Inspection

These are useful for internal/debug UI, not the primary mobile dictation UI.

### GET `/v1/runtime/runs?limit=50`

Returns visible runtime sessions for the account/org.

Response item:

```json
{
  "id": "server-runtime-uuid",
  "account_id": "account-uuid",
  "account_email": "user@example.com",
  "client_run_id": "mobile-run-003",
  "mode": "normal_voice",
  "source": "desktop_voice",
  "platform": "ios",
  "app_version": "0.1.0",
  "status": "completed",
  "error_kind": null,
  "input_hash": "sha256-hex",
  "output_hash": "sha256-hex",
  "provider_summary": {},
  "latency_json": {},
  "metadata_json": {},
  "created_at": "2026-06-08T12:00:00Z",
  "updated_at": "2026-06-08T12:00:01Z"
}
```

### GET `/v1/runtime/runs/:id`

Returns:

```json
{
  "run": {},
  "stages": [
    {
      "id": "stage-uuid",
      "stage": "prompt_built",
      "status": "ok",
      "latency_ms": 1,
      "error_kind": null,
      "metadata_json": {},
      "created_at": "2026-06-08T12:00:00Z"
    }
  ],
  "provider_usage": [
    {
      "id": "usage-uuid",
      "provider": "groq",
      "model": "llama-3.1-8b-instant",
      "credential_scope": "airnote_env",
      "request_ms": null,
      "ttft_ms": null,
      "stream_ms": null,
      "total_ms": 340,
      "timeout_ms": null,
      "status": "ok",
      "error_kind": null,
      "fallback_reason": null,
      "created_at": "2026-06-08T12:00:00Z"
    }
  ]
}
```

### GET `/v1/runtime/learning-events?limit=50`

Returns stored learning events visible to the account/org.

## Dry Run

### POST `/v1/runtime/voice/dry-run`

Creates and completes a runtime session without provider calls. Useful for smoke testing auth and runtime ledger writes.

Request:

```json
{
  "client_run_id": "mobile-smoke-001",
  "mode": "normal_voice",
  "source": "mobile_voice",
  "device_id": "ios-device-id",
  "platform": "ios",
  "app_version": "0.1.0",
  "metadata": {
    "screen": "runtime-smoke"
  }
}
```

Response:

```json
{
  "run_id": "server-runtime-uuid",
  "status": "completed",
  "message": "server runtime dry-run accepted"
}
```

## Recommended Mobile MVP Wiring

For a UI-only teammate, implement in this order:

1. Login/signup and persist bearer token in Keychain/Keystore.
2. Call `GET /v1/runtime/status`.
3. Open `WS /v1/runtime/notifications/ws` after login.
4. For first voice MVP, use `POST /v1/runtime/voice/wav` with a complete WAV.
5. For lower latency, switch to `WS /v1/runtime/voice/ws` with 16 kHz mono linear16 PCM.
6. Show partial/final transcript and done/error states from WS events.
7. Write and read history with `/v1/runtime/history`.
8. After user edits a result, call `analyze-edit`; show candidates; call `confirm-batch` only after user confirmation.

## Privacy and Local Client Responsibilities

Mobile should:
- keep the bearer token in Keychain/Keystore, not normal app storage;
- keep provider secrets out of local logs;
- avoid sending `screen_context` until there is an explicit UX and privacy copy;
- avoid sending raw audio to history or diagnostics routes;
- use `client_run_id` and `recording_id` generated client-side for correlation;
- hash local sensitive comparisons before sending `input_hash`, `output_hash`, or `corrected_hash`;
- treat server history as user-visible retained data.

Server currently:
- does not persist raw audio in runtime routes;
- stores transcript/output/final text in history;
- stores runtime input/output hashes in `runtime_sessions`;
- stores stage/provider metadata for observability;
- stores encrypted provider credentials only when `RUNTIME_CREDENTIALS_KEY` is configured.

## Runtime Settings

Wave 11 adds server-owned cross-device settings. Mobile should use these APIs instead of creating its own local settings source of truth.

### GET `/v1/runtime/settings`

Returns the signed-in user's runtime settings and credential summaries. Raw provider secrets are never returned.

Response:

```json
{
  "selected_model": "fast",
  "output_language": "hinglish",
  "tone_preset": "professional",
  "custom_prompt": null,
  "auto_paste": true,
  "edit_capture": true,
  "learning_enabled": true,
  "server_runtime_enabled": true,
  "server_audio_runtime_enabled": false,
  "message_polish_mode": false,
  "notification_prefs": {},
  "privacy_prefs": {},
  "version": 1,
  "updated_at": "2026-06-08T12:00:00Z",
  "credentials": [
    {
      "id": "credential-uuid",
      "provider": "groq",
      "display_name": "Groq API key",
      "secret_last4": "abcd",
      "status": "active",
      "updated_at": "2026-06-08T12:00:00Z"
    }
  ]
}
```

### PATCH `/v1/runtime/settings`

Partial update. Send only changed fields.

Request:

```json
{
  "selected_model": "smart",
  "output_language": "english",
  "message_polish_mode": true,
  "notification_prefs": {
    "learned": true,
    "updates": true,
    "error": true
  }
}
```

Validation:
- `selected_model`: `fast` or `smart`
- `output_language`: `hinglish` or `english`
- unknown/unsupported values return `422`

### POST `/v1/runtime/settings/sync`

First-launch or cross-device merge endpoint. Mobile usually should not need this unless it had offline-local settings before sign-in.

Request:

```json
{
  "settings": {
    "selected_model": "fast",
    "output_language": "hinglish"
  },
  "local_updated_at": "2026-06-08T12:00:00Z",
  "source": "mobile"
}
```

Behavior:
- if server has no settings, create from request;
- if local timestamp is newer, update server;
- on tie or older local timestamp, server wins.
