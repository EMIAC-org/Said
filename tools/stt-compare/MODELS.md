# Local Hinglish STT benchmark models

**Leaderboard:** `tools/stt-compare/WINNERS.json` (auto-updated by `compare_local.py`)
**Full transcripts:** `tools/stt-compare/.local_stt_benchmark.json`

Weights download to `tools/stt-compare/.hf-cache` and are **purged after each compare run** (local + `~/.cache/huggingface/hub` for tracked models).

## Benchmark clip

`~/Downloads/6109386237469007631.ogg` (~53s Hinglish voice note)

## Models under test

| Model | HF id | Role |
|---|---|---|
| Zero STT | `shunyalabs/zero-stt-hinglish` | Fast baseline, Devanagari-heavy raw |
| Oriserve Apex | `Oriserve/Whisper-Hindi2Hinglish-Apex` | Best batch quality on benchmark clip (2026-06-11) |
| Oriserve Swift | `Oriserve/Whisper-Hindi2Hinglish-Swift` | Faster Oriserve variant; official streaming server default |

## Polish (fixed for compare runs)

| Field | Value |
|---|---|
| **Model** | `meta-llama/llama-4-scout-17b-16e-instruct` (`SELECTED_MODEL=smart`) |
| **Pipeline** | control-plane `polish-cli` |

## Commands

```bash
# Full compare (cached Zero+Apex raws, runs Swift, polishes all, cleans weights)
python tools/stt-compare/compare_local.py

# Swift only
python tools/stt-compare/compare_local.py --only swift

# Reuse all cached STT, polish only
python tools/stt-compare/compare_local.py --skip-stt

# Manual weight cleanup
python tools/stt-compare/cleanup_models.py
```

## Latest result (2026-06-11, incl. Swift)

| | Zero STT | Oriserve Apex | Oriserve Swift |
|---|---|---|---|
| STT time | 88.5s | 178.0s | **42.7s** |
| Raw script | Heavy Devanagari | Roman Hinglish | Roman Hinglish |
| Names (raw) | विक्पुल / निक्पुण | vipul / nipun | vipul / nipun |
| Scout polish | Vikpul, Time Sem | Vipul, agreement, TDS 2% | Vipul, agreement, TDS 2% |

**Overall winner (auto heuristic):** **Oriserve Swift** — tied Scout-polish score with Apex (4/5), **4× faster** than Apex.

**Human read:** Swift and Apex are very close on this clip. Swift wins on speed; Apex raw is slightly cleaner (`fanse hue` vs Swift `paise paise hue`, `media` vs `mahine`). Zero STT recall is strong but Scout polish breaks names.

**Recommendation:** Use **Swift** for local streaming/batch default; keep Apex as quality tie-break if Swift raw looks garbled on a clip.
