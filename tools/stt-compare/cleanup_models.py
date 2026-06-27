#!/usr/bin/env python3
"""Remove local + global Hugging Face weights for STT benchmark models."""

from __future__ import annotations

import argparse

from hf_env import TRACKED_MODELS, purge_all_stt_weights, purge_global_model_caches


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--global-only",
        action="store_true",
        help="Only purge ~/.cache/huggingface hub entries for tracked models",
    )
    parser.add_argument(
        "--model",
        action="append",
        dest="models",
        help="Specific HF model id to purge (repeatable); default: all tracked",
    )
    args = parser.parse_args()

    if args.global_only:
        removed = purge_global_model_caches(tuple(args.models or TRACKED_MODELS))
        print("Removed from global HF cache:")
        for path in removed:
            print(f"  {path}")
        if not removed:
            print("  (nothing to remove)")
        return 0

    result = purge_all_stt_weights()
    print("Removed local HF cache:")
    for path in result["local"]:
        print(f"  {path}")
    if not result["local"]:
        print("  (nothing to remove)")
    print("Removed from global HF cache:")
    for path in result["global"]:
        print(f"  {path}")
    if not result["global"]:
        print("  (nothing to remove)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
