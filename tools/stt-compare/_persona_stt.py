#!/usr/bin/env python3
"""Transcribe the two latest recordings with the live Swift STT model.

Writes raw Roman-Hinglish transcripts to ../../.context/persona_transcripts.json
so the persona-lab Rust harness can polish them through the control-plane pipeline.
"""
from __future__ import annotations

import json
import time
from pathlib import Path

from hf_env import use_local_hf_cache

MODEL_ID = "Oriserve/Whisper-Hindi2Hinglish-Swift"
REPO = Path(__file__).resolve().parents[2]
OUT = REPO / ".context" / "persona_transcripts.json"
FILES = [
    Path.home() / "Downloads" / "said-2026-06-20-1251-41-words.wav",
    Path.home() / "Downloads" / "said-2026-06-20-0022-238-words.wav",
]


def load_audio_numpy(path: Path):
    import numpy as np
    import soundfile as sf

    data, sr = sf.read(str(path), dtype="float32")
    if data.ndim > 1:
        data = data.mean(axis=1)
    return {"array": np.asarray(data, dtype=np.float32), "sampling_rate": sr}


def main() -> int:
    cache = use_local_hf_cache()
    print(f"HF cache: {cache}", flush=True)
    print("Loading Swift pipeline (first run downloads ~1.5GB)...", flush=True)
    t0 = time.perf_counter()
    from transformers import pipeline

    asr = pipeline(
        "automatic-speech-recognition",
        model=MODEL_ID,
        device="cpu",
        model_kwargs={"torch_dtype": "auto"},
        generate_kwargs={"task": "transcribe", "language": "en"},
        chunk_length_s=30,
        stride_length_s=5,
    )
    print(f"Pipeline ready in {time.perf_counter()-t0:.1f}s", flush=True)

    out = {}
    for f in FILES:
        print(f"\n=== {f.name} ===", flush=True)
        t1 = time.perf_counter()
        try:
            res = asr(load_audio_numpy(f), return_timestamps=True)
        except Exception as e:  # noqa: BLE001
            print(f"numpy path failed ({e}); trying file path", flush=True)
            res = asr(str(f), return_timestamps=True)
        text = (res.get("text") if isinstance(res, dict) else res) or ""
        text = text.strip()
        out[f.name] = text
        print(f"[{time.perf_counter()-t1:.1f}s] {text}", flush=True)

    OUT.parent.mkdir(exist_ok=True)
    OUT.write_text(json.dumps(out, ensure_ascii=False, indent=2))
    print(f"\nSAVED -> {OUT}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
