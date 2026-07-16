/* Display-only rate-card mirrors of the backend costs.rs constants.
   Actual per-run cost always comes from the API; these only annotate the UI. */

// Dictation STT — Together Nemotron
export const STT_PER_HOUR = 0.09

// Dictation polish — Gemma
export const POLISH_IN_PER_M = 0.105
export const POLISH_OUT_PER_M = 0.51

// Meetings — approximated on the DeepSeek V4 Flash rate card (per product direction).
export const MEET_PROVIDER = 'DeepSeek'
export const MEET_MODEL = 'deepseek-v4-flash'
export const MEET_IN_PER_M = 0.14
export const MEET_CACHE_IN_PER_M = 0.0028
export const MEET_OUT_PER_M = 0.28
