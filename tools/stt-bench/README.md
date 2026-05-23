# STT Bench

Local benchmark harness for testing "honest" STT candidates against real Said WAV files.

It does not modify Said runtime, SQLite, vocabulary, or learning state. It only reads:

- retained WAV files from app data
- optional local SQLite recording metadata
- optional `.env` API keys

## Quick Start

Run the current Said DB transcript surfaces over every retained WAV:

```bash
python3 tools/stt-bench/run.py \
  --providers db_raw,db_local,db_polished \
  --terms EMIAC,Macobs,Kubernetes,n8n,Perplexity,Claude,JavaScript
```

Run Deepgram raw mode over the latest 10 WAVs, using `DEEPGRAM_API_KEY` from `.env`:

```bash
python3 tools/stt-bench/run.py \
  --providers deepgram_raw \
  --latest 10 \
  --terms EMIAC,Macobs,Kubernetes,n8n,Perplexity,Claude,JavaScript
```

Run Vosk locally if you have a model:

```bash
python3 -m pip install vosk
python3 tools/stt-bench/run.py \
  --providers vosk \
  --vosk-model ~/models/vosk-model-small-hi-0.22 \
  --latest 10 \
  --terms EMIAC,Macobs,Kubernetes,n8n
```

Run faster-whisper if installed:

```bash
python3 -m pip install faster-whisper
python3 tools/stt-bench/run.py \
  --providers faster_whisper \
  --faster-whisper-model small \
  --latest 10 \
  --terms EMIAC,Macobs,Kubernetes,n8n
```

Run whisper.cpp if you have a binary and model:

```bash
python3 tools/stt-bench/run.py \
  --providers whisper_cpp \
  --whisper-cpp-bin /path/to/whisper-cli \
  --whisper-cpp-model /path/to/ggml-small.bin \
  --latest 10 \
  --terms EMIAC,Macobs,Kubernetes,n8n
```

## Outputs

Each run writes:

- `results/<run_id>/results.jsonl` - detailed provider output per WAV
- `results/<run_id>/results.csv` - spreadsheet-friendly detailed output
- `results/<run_id>/provider_summary.csv` - aggregate quality/latency summary
- `results/<run_id>/cases.jsonl` - discovered WAVs and DB metadata used for scoring

## Manifest

For stricter tests, pass a JSONL manifest:

```bash
python3 tools/stt-bench/run.py --manifest tools/stt-bench/manifest.example.jsonl --providers deepgram_raw
```

Each line can use a full path, filename, or audio id:

```json
{"wav":"b428fe62-4931-4fc8-8176-3e7cd981ffdb.wav","expected":["EMIAC"],"note":"dev term test"}
```

If no manifest is provided, the tool discovers WAV files under:

- `~/Library/Application Support/VoicePolish/audio`
- `~/Library/Application Support/Said/audio`

and infers expected terms from DB final/polished/local/raw text plus `--terms`.

