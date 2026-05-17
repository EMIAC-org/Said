#!/usr/bin/env python3
"""Download text-only transcripts from HuggingFace parquet files.

Skips all audio datasets (FLEURS parquet embeds WAV blobs = OOM).
Uses pure text datasets only. Total: ~15 MB download, ~2 MB output.
"""

import json
import os
import sys
import tempfile
import urllib.request

OUT_DIR = os.path.dirname(os.path.abspath(__file__))
OUT_PATH = os.path.join(OUT_DIR, "transcripts.jsonl")


def download_parquet(url):
    tmp = tempfile.NamedTemporaryFile(suffix=".parquet", delete=False)
    req = urllib.request.Request(url, headers={"User-Agent": "said-eval/1.0"})
    with urllib.request.urlopen(req, timeout=120) as resp:
        while True:
            chunk = resp.read(65536)
            if not chunk:
                break
            tmp.write(chunk)
    tmp.close()
    return tmp.name


def main():
    import pandas as pd

    records = []

    # 1. English-to-Hinglish (text only, ~11 MB parquet, ~100K rows)
    print("[1/3] findnitai/english-to-hinglish...", end=" ", flush=True)
    try:
        url = "https://huggingface.co/api/datasets/findnitai/english-to-hinglish/parquet/default/train/0.parquet"
        path = download_parquet(url)
        df = pd.read_parquet(path)
        os.unlink(path)
        count = 0
        for _, row in df.head(5000).iterrows():
            h = str(row.get("hinglish", "")).strip()
            e = str(row.get("english", "")).strip()
            if h and len(h) > 10:
                records.append({"lang": "hinglish", "text": h})
                count += 1
            if e and len(e) > 10:
                records.append({"lang": "english", "text": e})
                count += 1
        print(f"{count} sentences")
    except Exception as e:
        print(f"SKIP: {e}")

    # 2. cfilt/iitb-english-hindi (parallel EN-HI corpus, text only)
    print("[2/3] cfilt/iitb-english-hindi...", end=" ", flush=True)
    try:
        api = "https://huggingface.co/api/datasets/cfilt/iitb-english-hindi/parquet"
        req = urllib.request.Request(api, headers={"User-Agent": "said-eval/1.0"})
        with urllib.request.urlopen(req, timeout=15) as resp:
            meta = json.loads(resp.read())
        # Get first parquet of default/train
        parquet_url = None
        for cfg in meta.values():
            for split_urls in cfg.values():
                if split_urls:
                    parquet_url = split_urls[0]
                    break
            if parquet_url:
                break
        if parquet_url:
            path = download_parquet(parquet_url)
            df = pd.read_parquet(path)
            os.unlink(path)
            count = 0
            for _, row in df.head(3000).iterrows():
                for col in df.columns:
                    val = row[col]
                    if isinstance(val, dict):
                        for lang_key, text in val.items():
                            if isinstance(text, str) and len(text.strip()) > 10:
                                lang = "hindi" if "hi" in lang_key else "english"
                                records.append({"lang": lang, "text": text.strip()})
                                count += 1
                    elif isinstance(val, str) and len(val.strip()) > 10:
                        lang = "hindi" if any(ord(c) > 0x0900 and ord(c) < 0x097F for c in val) else "english"
                        records.append({"lang": lang, "text": val.strip()})
                        count += 1
            print(f"{count} sentences")
        else:
            print("no parquet URL found")
    except Exception as e:
        print(f"SKIP: {e}")

    # 3. HinGE (human Hinglish, small ~1.5 MB)
    print("[3/3] LingoIITGN/HinGE...", end=" ", flush=True)
    try:
        api = "https://huggingface.co/api/datasets/LingoIITGN/HinGE/parquet"
        req = urllib.request.Request(api, headers={"User-Agent": "said-eval/1.0"})
        with urllib.request.urlopen(req, timeout=15) as resp:
            meta = json.loads(resp.read())
        parquet_url = None
        for cfg in meta.values():
            for split_urls in cfg.values():
                if split_urls:
                    parquet_url = split_urls[0]
                    break
            if parquet_url:
                break
        if parquet_url:
            path = download_parquet(parquet_url)
            df = pd.read_parquet(path)
            os.unlink(path)
            count = 0
            for _, row in df.iterrows():
                for col in df.columns:
                    val = row[col]
                    if isinstance(val, str) and len(val.strip()) > 10:
                        lang = "hinglish" if "hinglish" in col.lower() else (
                            "english" if "english" in col.lower() else "hindi")
                        records.append({"lang": lang, "text": val.strip()})
                        count += 1
                    elif isinstance(val, list):
                        for item in val:
                            if isinstance(item, str) and len(item.strip()) > 10:
                                lang = "hinglish" if "hinglish" in col.lower() else (
                                    "english" if "english" in col.lower() else "hindi")
                                records.append({"lang": lang, "text": item.strip()})
                                count += 1
            os.unlink(path) if os.path.exists(path) else None
            print(f"{count} sentences")
        else:
            print("no parquet URL found")
    except Exception as e:
        print(f"SKIP: {e}")

    # Deduplicate
    seen = set()
    unique = []
    for r in records:
        key = r["text"][:100]
        if key not in seen:
            seen.add(key)
            unique.append(r)

    with open(OUT_PATH, "w", encoding="utf-8") as f:
        for rec in unique:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")

    by_lang = {}
    for r in unique:
        by_lang[r["lang"]] = by_lang.get(r["lang"], 0) + 1

    size_kb = os.path.getsize(OUT_PATH) / 1024
    print(f"\nSaved {len(unique)} transcripts to {OUT_PATH} ({size_kb:.0f} KB)")
    for lang, count in sorted(by_lang.items()):
        print(f"  {lang}: {count}")


if __name__ == "__main__":
    main()
