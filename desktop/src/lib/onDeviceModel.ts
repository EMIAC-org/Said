// Single source of truth for the on-device speech model AirNote downloads.
//
// The app bundles only the tiny Silero VAD support model. Users install this
// Oriserve speech model during onboarding; dictation and meetings share it.
//
// Backend commands that operate on it:
//   - `dictation_model_status`                    → install status
//   - `download_dictation_model`                  → download (auto-fetches VAD)
//   - `delete_dictation_model`                    → remove
//   - `reclaim_old_models`                        → delete the superseded model(s)
export const NEW_MODEL_FILE = "ggml-oriserve-hinglish-fp16.bin";
export const NEW_MODEL_NAME = "Oriserve Hinglish";
export const NEW_MODEL_SIZE_HINT = "~148 MB";
