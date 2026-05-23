# Tier 2 Local Correction Trainer

This is a dev/local tool only. It reads the user's local SQLite database,
trains a tiny correction scorer, and writes artifacts under a local Tier 2
directory. It is not bundled into the released app.

Runtime learning does not wait for this trainer. Confirmed edits update
`tier2_policy_weights` immediately, and the backend uses those SQLite policy
weights on the next dictation. This tool periodically distills accumulated
local policy, vocabulary, and alias evidence into `correction_model.onnx`.

## Dependencies

```bash
python3 -m pip install torch onnx numpy
```

## Usage

```bash
python3 tools/tier2/train_correction_model.py \
  --db "$HOME/Library/Application Support/VoicePolish/db.sqlite" \
  --user-id "$(sqlite3 "$HOME/Library/Application Support/VoicePolish/db.sqlite" 'SELECT id FROM local_user LIMIT 1')"
```

Outputs:

- `correction_model.onnx`
- `vocab_index.json`
- `model_metadata.json`

By default, the tool also upserts `tier2_model_metadata` in the same SQLite
database so the backend can lazy-load the ONNX artifact. User correction data
never leaves the local machine.
