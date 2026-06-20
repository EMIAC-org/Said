#!/usr/bin/env python3
"""Local Oriserve Swift STT WebSocket server for AirNote dictation.

Protocol (ws://127.0.0.1:<port>/stream):
  - Server sends {"type":"ready"} on connect
  - Client sends binary PCM (linear16, mono, 16 kHz)
  - Server sends {"type":"partial","text":"..."} after VAD-chunked inference
  - Client sends {"type":"finalize"} as UTF-8 text
  - Server sends {"type":"final","text":"..."} then keeps connection open
  - HTTP GET /health → 200 when model is loaded.

Live partials decode a bounded rolling window on a debounced background worker
so the WS loop never blocks on Whisper inference. Final decode chunks long
sessions because this Whisper checkpoint cannot process >30s in one pass
without timestamp mode.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import re
import sys
import time
from collections import Counter
from http.server import BaseHTTPRequestHandler, HTTPServer
from threading import Thread

import numpy as np

try:
    import webrtcvad
except ImportError:
    webrtcvad = None

try:
    from transformers import pipeline
except ImportError as exc:
    print(f"transformers not installed: {exc}", file=sys.stderr)
    sys.exit(1)

SAMPLE_RATE = 16_000
FRAME_MS = 30
BYTES_PER_FRAME = int(SAMPLE_RATE * FRAME_MS / 1000) * 2
# First partial after ~1s of speech; re-decode at most every 1.2s while speaking.
MIN_BUFFER_SECS = 1.0
MIN_BUFFER_BYTES = int(MIN_BUFFER_SECS * SAMPLE_RATE) * 2
DECODE_INTERVAL_SECS = 1.2
MAX_DECODE_SECS = 28.0
MAX_DECODE_BYTES = int(MAX_DECODE_SECS * SAMPLE_RATE) * 2
SPECIAL_TOKEN_RE = re.compile(r"<\|[^|]+?\|>")
GENERATE_KWARGS = {
    "task": "transcribe",
    "language": "hi",
    "no_repeat_ngram_size": 3,
}
# Pad the final decode with trailing silence so Whisper doesn't clip the last
# word when the stream closes a frame after the final syllable.
FINAL_TAIL_PAD_BYTES = int(0.4 * SAMPLE_RATE) * 2
# Whisper hallucinates fluent text on silence/noise. Skip the final decode when
# the captured audio is essentially silent. Kept low so genuinely quiet single
# words still pass — only true near-silence is dropped.
SILENCE_RMS_THRESHOLD = 45.0


def _pcm_rms(pcm: bytes) -> float:
    if len(pcm) < 2:
        return 0.0
    samples = np.frombuffer(pcm, dtype=np.int16).astype(np.float32)
    if samples.size == 0:
        return 0.0
    return float(np.sqrt(np.mean(samples * samples)))


class HealthHandler(BaseHTTPRequestHandler):
    model_ready = False

    def log_message(self, *_args):
        return

    def do_GET(self):
        if self.path != "/health":
            self.send_response(404)
            self.end_headers()
            return
        if HealthHandler.model_ready:
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
        else:
            self.send_response(503)
            self.end_headers()
            self.wfile.write(b"loading")


class SwiftEngine:
    def __init__(self, model_dir: str):
        device = "mps" if _has_mps() else "cpu"
        logging.info("loading Swift model from %s on %s", model_dir, device)
        self.pipe = pipeline(
            "automatic-speech-recognition",
            model=model_dir,
            device=device,
        )
        self.vad = webrtcvad.Vad(2) if webrtcvad else None
        self._warm_up()
        HealthHandler.model_ready = True
        logging.info("Swift model ready")

    def build_prompt_ids(self, terms):
        """Whisper initial-prompt biasing: prime the decoder with the user's
        personal vocabulary (Kubernetes, n8n, EMIAC, ...) so the frozen weights
        emit those as whole tokens instead of inventing a new phonetic garble
        each utterance. Best-effort — returns None if unsupported."""
        if not terms:
            return None
        try:
            text = ", ".join(t.strip() for t in terms if t and t.strip())[:400]
            if not text:
                return None
            return self.pipe.tokenizer.get_prompt_ids(text, return_tensors="np")
        except Exception:
            logging.exception("failed to build vocab prompt_ids; biasing disabled")
            return None

    def transcribe_pcm(self, pcm: bytes, prompt_ids=None) -> str:
        if len(pcm) < BYTES_PER_FRAME:
            return ""
        if len(pcm) > MAX_DECODE_BYTES:
            texts: list[str] = []
            for start in range(0, len(pcm), MAX_DECODE_BYTES):
                chunk = pcm[start : start + MAX_DECODE_BYTES]
                if len(chunk) < BYTES_PER_FRAME:
                    continue
                text = self._transcribe_pcm_once(chunk, prompt_ids)
                if text:
                    texts.append(text)
            return clean_transcript(" ".join(texts))

        return self._transcribe_pcm_once(pcm, prompt_ids)

    def _transcribe_pcm_once(self, pcm: bytes, prompt_ids=None) -> str:
        samples = np.frombuffer(pcm, dtype=np.int16).astype(np.float32) / 32768.0
        text = self._run_pipe(samples, prompt_ids)
        text = clean_transcript(text)
        if is_suspicious_transcript(text):
            logging.warning("dropping suspicious Swift transcript: %r", text[:160])
            return ""
        return text

    def _run_pipe(self, samples, prompt_ids) -> str:
        gen = GENERATE_KWARGS
        if prompt_ids is not None:
            gen = {**GENERATE_KWARGS, "prompt_ids": prompt_ids}
        try:
            result = self.pipe(
                {"array": samples, "sampling_rate": SAMPLE_RATE},
                generate_kwargs=gen,
            )
        except Exception:
            # Vocab biasing can be rejected by some transformers versions —
            # never let it break dictation; fall back to an unbiased decode.
            if prompt_ids is not None:
                logging.exception("decode with prompt_ids failed; retrying unbiased")
                result = self.pipe(
                    {"array": samples, "sampling_rate": SAMPLE_RATE},
                    generate_kwargs=GENERATE_KWARGS,
                )
            else:
                raise
        return result.get("text", "") if isinstance(result, dict) else str(result)

    def _warm_up(self) -> None:
        samples = np.zeros(SAMPLE_RATE, dtype=np.float32)
        try:
            self.pipe(
                {"array": samples, "sampling_rate": SAMPLE_RATE},
                generate_kwargs=GENERATE_KWARGS,
            )
            logging.info("Swift model warm-up complete")
        except Exception as exc:
            logging.exception("Swift warm-up failed")
            raise RuntimeError("Swift warm-up failed") from exc

    def note_vad(self, frame: bytes) -> bool:
        if self.vad is None:
            return True
        if len(frame) != BYTES_PER_FRAME:
            return True
        return self.vad.is_speech(frame, SAMPLE_RATE)


def _has_mps() -> bool:
    try:
        import torch

        return bool(getattr(torch.backends, "mps", None) and torch.backends.mps.is_available())
    except Exception:
        return False


def clean_transcript(text: str) -> str:
    text = SPECIAL_TOKEN_RE.sub(" ", text or "")
    return re.sub(r"\s+", " ", text).strip()


def is_suspicious_transcript(text: str) -> bool:
    stripped = text.strip()
    if not stripped:
        return True
    compact = re.sub(r"\s+", "", stripped)
    if not any(ch.isalnum() for ch in compact):
        return True
    tokens = stripped.split()
    if len(tokens) >= 8:
        normalized = [
            re.sub(r"^[^\w\u0900-\u097F]+|[^\w\u0900-\u097F]+$", "", token).lower()
            for token in tokens
        ]
        normalized = [token for token in normalized if token]
        if not normalized:
            return True
        counts = Counter(normalized)
        top = counts.most_common(1)[0][1]
        unique_ratio = len(counts) / len(normalized)
        top_ratio = top / len(normalized)
        avg_len = sum(len(token) for token in normalized) / len(normalized)
        if top_ratio >= 0.55 and unique_ratio <= 0.30:
            return True
        if avg_len <= 1.25 and len(normalized) >= 12:
            return True
    punct = sum(1 for ch in compact if not ch.isalnum())
    if compact and punct / len(compact) > 0.65:
        return True
    return False


class LiveSession:
    """Per-connection rolling decode state."""

    def __init__(self, engine: SwiftEngine, websocket):
        self.engine = engine
        self.websocket = websocket
        self.session_pcm = bytearray()
        self.speech_pcm = bytearray()
        self.silence_frames = 0
        self.latest_text = ""
        self.last_decode_at = 0.0
        self.decode_task: asyncio.Task | None = None
        self.decode_lock = asyncio.Lock()
        self.closed = False
        self.prompt_ids = None

    def set_vocab(self, terms) -> None:
        self.prompt_ids = self.engine.build_prompt_ids(terms)
        if self.prompt_ids is not None:
            logging.info("Swift vocab biasing active (%d terms)", len(terms))

    def ingest_pcm(self, pcm: bytes) -> None:
        self.session_pcm.extend(pcm)
        offset = 0
        while offset + BYTES_PER_FRAME <= len(pcm):
            frame = pcm[offset : offset + BYTES_PER_FRAME]
            offset += BYTES_PER_FRAME
            if self.engine.note_vad(frame):
                self.speech_pcm.extend(frame)
                self.silence_frames = 0
            else:
                self.silence_frames += 1
        if offset < len(pcm):
            self.speech_pcm.extend(pcm[offset:])
        self._maybe_schedule_decode()

    def _maybe_schedule_decode(self) -> None:
        if self.closed:
            return
        buf = self._decode_buffer()
        if len(buf) < MIN_BUFFER_BYTES:
            return
        now = time.monotonic()
        if now - self.last_decode_at < DECODE_INTERVAL_SECS:
            return
        if self.decode_task and not self.decode_task.done():
            return
        self.decode_task = asyncio.create_task(self._run_decode(bytes(buf)))

    def _decode_buffer(self) -> bytearray:
        # Prefer speech-only VAD buffer; fall back to full session audio.
        if len(self.speech_pcm) >= MIN_BUFFER_BYTES:
            buf = self.speech_pcm
        else:
            buf = self.session_pcm
        if len(buf) > MAX_DECODE_BYTES:
            return bytearray(buf[-MAX_DECODE_BYTES:])
        return buf

    async def _run_decode(self, pcm: bytes) -> None:
        async with self.decode_lock:
            if self.closed:
                return
            self.last_decode_at = time.monotonic()
            try:
                text = await asyncio.to_thread(
                    self.engine.transcribe_pcm, pcm, self.prompt_ids
                )
            except Exception as exc:
                logging.exception("Swift decode failed")
                await self._send_error(f"decode failed: {exc}")
                return
            if text and text != self.latest_text:
                self.latest_text = text
                try:
                    await self.websocket.send(
                        json.dumps({"type": "partial", "text": self.latest_text})
                    )
                except Exception:
                    self.closed = True

    async def finalize(self) -> str:
        # Always decode the full captured session on release. Live partials are
        # useful while holding the hotkey, but the final text must include audio
        # that arrived after the last partial decode.
        buf = bytes(self.session_pcm)
        final_text = ""
        if len(buf) >= BYTES_PER_FRAME:
            # Skip near-silent sessions — decoding silence makes Whisper
            # hallucinate fluent text (the "see shit happened" phantom on a pause).
            if _pcm_rms(buf) < SILENCE_RMS_THRESHOLD:
                logging.info("Swift finalize skipped — near-silence")
                self.closed = True
                return ""
            # Pad trailing silence so the final word isn't clipped when the
            # stream closes right after the last syllable.
            padded = buf + (b"\x00" * FINAL_TAIL_PAD_BYTES)
            async with self.decode_lock:
                try:
                    final_text = await asyncio.to_thread(
                        self.engine.transcribe_pcm, padded, self.prompt_ids
                    )
                    if final_text:
                        self.latest_text = final_text
                except Exception as exc:
                    logging.exception("Swift finalize decode failed")
                    await self._send_error(f"finalize decode failed: {exc}")
                    return ""
        self.closed = True
        return final_text

    async def close(self) -> None:
        self.closed = True
        if self.decode_task and not self.decode_task.done():
            self.decode_task.cancel()

    async def _send_error(self, message: str) -> None:
        try:
            await self.websocket.send(json.dumps({"type": "error", "message": message}))
        except Exception:
            self.closed = True


async def handle_client(websocket, engine: SwiftEngine):
    await websocket.send(json.dumps({"type": "ready"}))
    session = LiveSession(engine, websocket)

    try:
        async for message in websocket:
            if isinstance(message, str):
                try:
                    payload = json.loads(message)
                except json.JSONDecodeError:
                    continue
                if payload.get("type") in ("config", "vocab"):
                    terms = (
                        payload.get("vocab")
                        or payload.get("hotwords")
                        or payload.get("terms")
                        or []
                    )
                    if isinstance(terms, list):
                        session.set_vocab([str(t) for t in terms])
                    continue
                if payload.get("type") == "finalize":
                    text = await session.finalize()
                    await websocket.send(json.dumps({"type": "final", "text": text}))
                    session.session_pcm.clear()
                    session.speech_pcm.clear()
                    session.latest_text = ""
                    session.closed = False
                    session.last_decode_at = 0.0
                    continue
                continue

            if isinstance(message, (bytes, bytearray)):
                session.ingest_pcm(bytes(message))
    finally:
        await session.close()


async def ws_main(engine: SwiftEngine, host: str, port: int):
    import websockets

    async def handler(websocket):
        await handle_client(websocket, engine)

    async with websockets.serve(handler, host, port, max_size=8 * 1024 * 1024):
        logging.info("Swift STT WS listening on ws://%s:%d/stream", host, port)
        await asyncio.Future()


def start_health_server(host: str, port: int):
    server = HTTPServer((host, port), HealthHandler)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server


def main():
    parser = argparse.ArgumentParser(description="AirNote Swift local STT sidecar")
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--port", type=int, default=8710)
    parser.add_argument("--health-port", type=int, default=0)
    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    health_port = args.health_port or (args.port + 1)
    start_health_server("127.0.0.1", health_port)
    engine = SwiftEngine(args.model_dir)
    asyncio.run(ws_main(engine, "127.0.0.1", args.port))


if __name__ == "__main__":
    main()
