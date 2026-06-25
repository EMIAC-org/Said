#!/usr/bin/env python3
"""Local batch ASR for a Hugging Face Whisper fine-tune."""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path

from hf_env import use_local_hf_cache

DEFAULT_AUDIO = Path.home() / "Downloads" / "6109386237469007631.ogg"


def load_audio_numpy(path: Path):
    import numpy as np
    import soundfile as sf

    data, samplerate = sf.read(str(path), dtype="float32")
    if data.ndim > 1:
        data = data.mean(axis=1)
    return {"array": np.asarray(data, dtype=np.float32), "sampling_rate": samplerate}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True, help="Hugging Face model id")
    parser.add_argument("audio", nargs="?", type=Path, default=DEFAULT_AUDIO)
    args = parser.parse_args()

    audio_path = args.audio.expanduser().resolve()
    if not audio_path.is_file():
        print(f"Error: audio file not found: {audio_path}", file=sys.stderr)
        return 1

    cache = use_local_hf_cache()
    print(f"Audio: {audio_path}")
    print(f"Model: {args.model}")
    print(f"HF cache: {cache}")
    print("Loading pipeline (first run may download weights)...", flush=True)

    t0 = time.perf_counter()
    from transformers import pipeline

    asr = pipeline(
        "automatic-speech-recognition",
        model=args.model,
        device="cpu",
        model_kwargs={"torch_dtype": "auto"},
        generate_kwargs={"task": "transcribe", "language": "en"},
    )
    t_load = time.perf_counter() - t0
    print(f"Pipeline ready in {t_load:.2f}s", flush=True)

    t1 = time.perf_counter()
    try:
        result = asr(str(audio_path), return_timestamps=True)
    except Exception:
        result = asr(load_audio_numpy(audio_path), return_timestamps=True)
    t_infer = time.perf_counter() - t1

    text = result.get("text", result) if isinstance(result, dict) else result
    print("\n--- Transcript ---")
    print(text.strip() if isinstance(text, str) else text)
    print("--- End ---")
    print(f"Inference: {t_infer:.2f}s | Total (incl. load): {time.perf_counter() - t0:.2f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
