#!/usr/bin/env python3
"""Local ASR for Oriserve Swift using AirNote-installed weights (oriserve-swift/).

Matches the desktop sidecar decode settings: language=hi, no timestamps.
"""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

import numpy as np

DEFAULT_AUDIO = Path.home() / "Downloads" / "said-2026-06-20-1251-words.wav"
MODEL_DIR_NAME = "oriserve-swift"
SAMPLE_RATE = 16_000
CHUNK_SECS = 28.0


def swift_model_dir() -> Path:
    return (
        Path.home()
        / "Library"
        / "Application Support"
        / "VoicePolish"
        / "models"
        / MODEL_DIR_NAME
    )


def load_audio(path: Path) -> np.ndarray:
    import soundfile as sf

    data, sr = sf.read(str(path), dtype="float32")
    if data.ndim > 1:
        data = data.mean(axis=1)
    if sr != SAMPLE_RATE:
        try:
            import torch
            import torchaudio

            wav = torch.from_numpy(np.asarray(data, dtype=np.float32)).unsqueeze(0)
            wav = torchaudio.functional.resample(wav, sr, SAMPLE_RATE)
            data = wav.squeeze(0).numpy()
        except Exception:
            ratio = SAMPLE_RATE / sr
            idx = (np.arange(int(len(data) * ratio)) / ratio).astype(np.int64)
            idx = np.clip(idx, 0, len(data) - 1)
            data = data[idx]
    return np.asarray(data, dtype=np.float32)


def chunk_audio(audio: np.ndarray, chunk_samples: int, hop_samples: int) -> list[np.ndarray]:
    if len(audio) <= chunk_samples:
        return [audio]
    chunks: list[np.ndarray] = []
    start = 0
    while start < len(audio):
        end = min(start + chunk_samples, len(audio))
        chunks.append(audio[start:end])
        if end >= len(audio):
            break
        start += hop_samples
    return chunks


def pick_device() -> str:
    import torch

    if torch.cuda.is_available():
        return "cuda"
    if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio", nargs="?", type=Path, default=DEFAULT_AUDIO)
    parser.add_argument("--model-dir", type=Path, default=None)
    args = parser.parse_args()

    audio_path = args.audio.expanduser().resolve()
    if not audio_path.is_file():
        print(f"Error: audio not found: {audio_path}", file=sys.stderr)
        return 1

    model_dir = (args.model_dir or swift_model_dir()).expanduser().resolve()
    if not (model_dir / "config.json").is_file():
        print(f"Error: Swift model not installed at {model_dir}", file=sys.stderr)
        return 1

    print(f"Audio: {audio_path}")
    print(f"Model: {model_dir}")

    from transformers import pipeline

    device = pick_device()
    print(f"Loading Swift on {device}...", flush=True)
    t0 = time.perf_counter()
    asr = pipeline(
        "automatic-speech-recognition",
        model=str(model_dir),
        device=device,
        generate_kwargs={
            "task": "transcribe",
            "language": "hi",
            "no_repeat_ngram_size": 3,
        },
    )
    t_load = time.perf_counter() - t0
    print(f"Pipeline ready in {t_load:.1f}s", flush=True)

    audio = load_audio(audio_path)
    audio_secs = len(audio) / SAMPLE_RATE
    chunk_samples = int(CHUNK_SECS * SAMPLE_RATE)
    hop_samples = int(24.0 * SAMPLE_RATE)

    t1 = time.perf_counter()
    parts: list[str] = []
    for chunk in chunk_audio(audio, chunk_samples, hop_samples):
        result = asr({"array": chunk, "sampling_rate": SAMPLE_RATE})
        text = result.get("text", result) if isinstance(result, dict) else str(result)
        text = text.strip()
        if text:
            parts.append(text)
    text = " ".join(parts).strip()
    t_infer = time.perf_counter() - t1

    print("\n--- Transcript ---")
    print(text)
    print("--- End ---")
    rtf = t_infer / audio_secs if audio_secs > 0 else 0.0
    print(
        f"Inference: {t_infer:.2f}s | RTF: {rtf:.2f}x | "
        f"Load: {t_load:.2f}s | Total: {time.perf_counter() - t0:.2f}s"
    )
    return 0 if text else 1


if __name__ == "__main__":
    raise SystemExit(main())
