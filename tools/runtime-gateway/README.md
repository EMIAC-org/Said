# AirNote Runtime Gateway Smoke Harness

This folder contains small scripts for testing the server-side runtime gateway before the desktop app is wired to it.

## WebSocket Audio Smoke

```bash
node tools/runtime-gateway/ws-smoke.mjs \
  --url https://airnote.emiactech.com \
  --token "$AIRNOTE_CLOUD_TOKEN" \
  --wav /path/to/mono-16k-linear16.wav \
  --model fast \
  --language hinglish
```

The script:

- opens `/v1/runtime/voice/ws?token=...`;
- sends `voice.start`;
- streams WAV PCM audio as binary WebSocket frames;
- sends `audio.end`;
- prints server events as JSON lines;
- exits successfully only after `runtime.done`.

Supported input is uncompressed PCM WAV. 16-bit mono is preferred. Stereo 16-bit PCM is downmixed to mono by the script.

This is intentionally a test harness, not a desktop runtime path.

## HTTP WAV Probe

For a simpler non-streaming smoke test, call the authenticated JSON route:

```bash
WAV_B64="$(base64 -i /path/to/sample.wav | tr -d '\n')"

curl -sS https://airnote.emiactech.com/v1/runtime/voice/wav \
  -H "Authorization: Bearer $AIRNOTE_CLOUD_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{
    \"wav_b64\":\"$WAV_B64\",
    \"selected_model\":\"fast\",
    \"output_language\":\"hinglish\"
  }"
```

This route runs server-side Deepgram batch STT and server-side polish, then returns the transcript, transcript hash, output, model, and latency breakdown. The server ledger still stores hashes and metadata, not the raw WAV/transcript/output by default.
