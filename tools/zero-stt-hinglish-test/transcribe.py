#!/usr/bin/env python3
"""Local ASR test for shunyalabs/zero-stt-hinglish."""

from __future__ import annotations

import argparse
import os
import sys
import time
from pathlib import Path

DEFAULT_AUDIO = Path.home() / "Downloads" / "6109386237469007631.ogg"
MODEL_ID = "shunyalabs/zero-stt-hinglish"


def load_audio_numpy(path: Path):
    import numpy as np
    import soundfile as sf

    data, samplerate = sf.read(str(path), dtype="float32")
    if data.ndim > 1:
        data = data.mean(axis=1)
    return {"array": np.asarray(data, dtype=np.float32), "sampling_rate": samplerate}


def main() -> int:
    parser = argparse.ArgumentParser(description="Transcribe audio with zero-stt-hinglish")
    parser.add_argument(
        "audio",
        nargs="?",
        type=Path,
        default=DEFAULT_AUDIO,
        help=f"Audio file path (default: {DEFAULT_AUDIO})",
    )
    args = parser.parse_args()
    audio_path = args.audio.expanduser().resolve()

    if not audio_path.is_file():
        print(f"Error: audio file not found: {audio_path}", file=sys.stderr)
        return 1

    if "HF_HOME" not in os.environ:
        os.environ["HF_HOME"] = str(Path.home() / ".cache" / "huggingface")

    print(f"Audio: {audio_path}")
    print(f"Model: {MODEL_ID}")
    print("Loading pipeline (first run may download weights)...", flush=True)

    t0 = time.perf_counter()
    try:
        from transformers import pipeline

        asr = pipeline(
            "automatic-speech-recognition",
            model=MODEL_ID,
            device="cpu",
        )
    except Exception as e:
        print(f"Pipeline init failed: {e}", file=sys.stderr)
        return 2

    t_load = time.perf_counter() - t0
    print(f"Pipeline ready in {t_load:.2f}s", flush=True)

    t1 = time.perf_counter()
    try:
        result = asr(str(audio_path), return_timestamps=True)
    except Exception as e:
        print(f"Direct path transcribe failed ({e}); trying soundfile load...", flush=True)
        try:
            inputs = load_audio_numpy(audio_path)
            result = asr(inputs, return_timestamps=True)
        except Exception as e2:
            print(f"Transcription failed: {e2}", file=sys.stderr)
            return 3

    t_infer = time.perf_counter() - t1
    t_total = time.perf_counter() - t0

    if isinstance(result, dict):
        text = result.get("text", result)
    else:
        text = result

    print("\n--- Transcript ---")
    print(text.strip() if isinstance(text, str) else text)
    print("--- End ---")
    print(f"Inference: {t_infer:.2f}s | Total (incl. load): {t_total:.2f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
