# Mobile Privacy And Retention

## Defaults For Internal Alpha

- Audio retention: short TTL, default 24 hours unless disabled by policy.
- Transcript/history retention: user controlled.
- Event retention: redacted operational metadata only.
- Provider keys: server-side only.
- Device tokens: Keychain on iOS; never stored in App Group files.

## Redaction Rules

Do not log:

- raw audio
- raw transcript
- polished text
- provider tokens
- authorization headers
- refresh tokens
- App Group session token

Allowed in normal diagnostics:

- build number
- platform
- surface
- permission state
- host app label
- field hint
- redacted error code
- latency buckets
- session/request IDs

## Learning Rules

- Explicit only in v1.
- Never learn from secure, OTP, password, banking, payment, phone, or numeric-only fields.
- Never promote personal terms into org vocabulary without explicit approval.
- Learning always runs after insertion or copy/save recovery.
