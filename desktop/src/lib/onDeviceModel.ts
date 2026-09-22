// Single source of truth for the on-device speech model AirNote downloads.
//
// The app bundles only the tiny Silero VAD support model. Users install this
// speech model during onboarding (or, when upgrading, through the post-update
// gate). It comes from a private repository through AirNote's control plane.
//
// The filename is the historical one on purpose: the model installs over the
// retired Oriserve file at the same path, and progress events carry this name.
//
// Backend commands that operate on it:
//   - `dictation_model_status`                    → installed = current model
//   - `download_dictation_model`                  → download (auto-fetches VAD)
//   - `delete_dictation_model`                    → remove
//   - `reclaim_old_models`                        → delete the superseded model(s)
export const NEW_MODEL_FILE = "ggml-oriserve-hinglish-fp16.bin";
export const NEW_MODEL_NAME = "AirNote Hinglish (41h)";
export const NEW_MODEL_SIZE_HINT = "~141 MB";
