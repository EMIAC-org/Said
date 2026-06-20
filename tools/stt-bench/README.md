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

Run Deepgram vs Apple SpeechAnalyzer over the latest WAV:

```bash
python3 tools/stt-bench/run.py \
  --providers deepgram_raw,apple_speech \
  --latest 1 \
  --apple-speech-locale en-US \
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

Run FluidAudio if you have built `fluidaudiocli`:

```bash
python3 tools/stt-bench/run.py \
  --providers fluid_audio \
  --fluid-audio-bin /tmp/FluidAudio/.build/release/fluidaudiocli \
  --fluid-audio-model-version v2 \
  --fluid-audio-language en \
  --latest 10 \
  --terms EMIAC,Macobs,Kubernetes,n8n
```

Run Gladia or AssemblyAI if their keys are present in `.env`:

```bash
GLADIA_API_KEY=...
ASSEMBLYAI_API_KEY=...

python3 tools/stt-bench/run.py \
  --providers gladia,assemblyai \
  --gladia-model solaria-1 \
  --gladia-code-switching \
  --assemblyai-keyterms-from-terms \
  --latest 10 \
  --terms EMIAC,Macobs,Kubernetes,n8n
```

Run the final Sortformer diarization reconciler over a whisper.cpp JSON:

```bash
/tmp/airnote-nemo-venv/bin/python tools/stt-bench/sortformer_diarize.py \
  --audio /path/to/meeting.asr.wav \
  --transcript-json /path/to/meeting.whisper.json \
  --model-path /path/to/diar_streaming_sortformer_4spk-v2.1.nemo \
  --diarization-out /path/to/meeting.diarization.final.json \
  --transcript-out /path/to/meeting.transcript.final.json
```

For benchmark fixtures with reference speaker segments, add:

```bash
  --reference-segments /path/to/reference.segments.json \
  --metrics-out /path/to/final-diarization.metrics.json
```

The reconciler preserves ASR text segments and assigns speakers by diarization
overlap. If Sortformer misses a region, the text remains in the final transcript
with an unassigned/provisional speaker label. In track-wise meeting mode, obvious
mic echo duplicates can be merged into the matching remote segment.

When the input transcript is AirNote's combined meeting artifact and includes
both `mic.wav` and `system.wav` in `source_wavs`, the same command automatically
switches to track-wise finalization:

- `mic.wav` is diarized into `Local Speaker 1`, `Local Speaker 2`, etc.
- `system.wav` is diarized into `Remote Speaker 1`, `Remote Speaker 2`, etc.
- the two labeled timelines are merged by timestamp into one final transcript.
- if laptop-speaker audio leaks into the mic, source-activity and text-overlap
  checks can merge the mic echo back into the matching remote speaker instead of
  creating a fake local speaker. The final JSON records the decision under
  `echo_suppression`.
- unassigned ASR hallucinations that extend past the real audio duration are
  dropped from the final transcript.
- unassigned tail hallucinations are dropped only when the source-activity file
  says that track was silent during the segment.

For a no-model smoke test, pass existing per-track diarization JSON:

```bash
python3 tools/stt-bench/sortformer_diarize.py \
  --audio /path/to/meeting.merged.wav \
  --transcript-json /path/to/meeting.transcript.json \
  --diarization-out /path/to/meeting.diarization.final.json \
  --transcript-out /path/to/meeting.transcript.final.json \
  --existing-track-diarization-json mic=/path/to/mic.diarization.json \
  --existing-track-diarization-json system=/path/to/system.diarization.json \
  --force-trackwise
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
