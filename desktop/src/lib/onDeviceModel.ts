// Single source of truth for the on-device speech model AirNote ships.
//
// This is the NEW model (Apex, q8_0 GGML, ~830 MB) that replaces the older
// 148 MB Oriserve model for BOTH dictation and meetings. When the final product
// name is chosen, rename it here once and every surface (onboarding, Settings,
// Meetings) follows.
//
// Backend commands that operate on it:
//   - `apex_model_status`                         → install status
//   - `meeting_download_whisper_model` {name}     → download (auto-fetches VAD)
//   - `delete_apex_model`                         → remove
//   - `reclaim_old_models`                        → delete the superseded model(s)
export const NEW_MODEL_FILE = "ggml-apex-hinglish-q8_0.bin";
// TODO(naming): final product name is TBD — swap this one constant when decided.
export const NEW_MODEL_NAME = "AirNote Native";
export const NEW_MODEL_SIZE_HINT = "~880 MB";
