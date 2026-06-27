# Swift STT sidecar

Live streaming WebSocket server used by AirNote dictation on macOS (Oriserve Whisper-Hindi2Hinglish-Swift, ~290 MB).

Protocol: binary PCM in, `partial` / `final` JSON out while the hotkey is held.

## Setup

```bash
cd desktop/src-tauri/resources/swift-stt-sidecar
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

## Manual test

```bash
python server.py --model-dir "$HOME/Library/Application Support/VoicePolish/models/oriserve-swift" --port 8710
```
