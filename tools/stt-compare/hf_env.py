"""Project-local Hugging Face cache — keeps weights off ~/.cache."""

from __future__ import annotations

import os
import shutil
from pathlib import Path

STT_COMPARE_DIR = Path(__file__).resolve().parent
LOCAL_HF_HOME = STT_COMPARE_DIR / ".hf-cache"

# Models we may download during benchmarks (for targeted cleanup).
TRACKED_MODELS = (
    "shunyalabs/zero-stt-hinglish",
    "Oriserve/Whisper-Hindi2Hinglish-Apex",
    "Oriserve/Whisper-Hindi2Hinglish-Swift",
)


def use_local_hf_cache() -> Path:
    LOCAL_HF_HOME.mkdir(parents=True, exist_ok=True)
    os.environ["HF_HOME"] = str(LOCAL_HF_HOME)
    os.environ.setdefault("HF_HUB_CACHE", str(LOCAL_HF_HOME / "hub"))
    return LOCAL_HF_HOME


def hub_dir_name(model_id: str) -> str:
    return "models--" + model_id.replace("/", "--")


def purge_local_hf_cache() -> list[str]:
    removed: list[str] = []
    hub = LOCAL_HF_HOME / "hub"
    if hub.is_dir():
        shutil.rmtree(hub)
        removed.append(str(hub))
    if LOCAL_HF_HOME.is_dir() and not any(LOCAL_HF_HOME.iterdir()):
        LOCAL_HF_HOME.rmdir()
        removed.append(str(LOCAL_HF_HOME))
    return removed


def purge_global_model_caches(model_ids: tuple[str, ...] = TRACKED_MODELS) -> list[str]:
    """Remove tracked STT weights from the user's global HF hub cache."""
    global_hub = Path.home() / ".cache" / "huggingface" / "hub"
    removed: list[str] = []
    if not global_hub.is_dir():
        return removed
    for model_id in model_ids:
        path = global_hub / hub_dir_name(model_id)
        if path.is_dir():
            shutil.rmtree(path)
            removed.append(str(path))
    return removed


def purge_all_stt_weights() -> dict[str, list[str]]:
    return {
        "local": purge_local_hf_cache(),
        "global": purge_global_model_caches(),
    }
