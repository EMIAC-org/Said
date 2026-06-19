# AirNote Swift STT sidecar

Local WebSocket inference server for `Oriserve/Whisper-Hindi2Hinglish-Swift`.

## Dev setup

```bash
cd desktop/src-tauri/resources/swift-stt-sidecar
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

Download the model from AirNote **Settings → Models → Speech recognition**, or:

```bash
python3 server.py --model-dir ~/.local/share/VoicePolish/models/oriserve-swift --port 8710
```

## Protocol

- `GET http://127.0.0.1:<health-port>/health` → `200 ok` when loaded
- `ws://127.0.0.1:<port>/stream` — binary linear16 mono 16 kHz PCM in, JSON partials out

Bundled with the macOS app under `Resources/swift-stt-sidecar/`. Tauri spawns `python3` on this script when Swift local STT is selected.
