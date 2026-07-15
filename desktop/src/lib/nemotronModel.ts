// Must stay in sync with desktop/src-tauri/src/nemotron.rs. These are separate
// FastConformer/RNNT GGUFs, not Whisper models.
export type NemotronVariant = "q4" | "q8";

export const NEMOTRON_MODELS: Record<
  NemotronVariant,
  { file: string; name: string; sizeHint: string; qualityHint: string }
> = {
  q4: {
    file: "nemotron-3.5-asr-streaming-0.6b-Q4_K_M.gguf",
    name: "Nemotron Streaming 3.5 (Q4)",
    sizeHint: "~496 MB",
    qualityHint: "Smaller download; best first option for lower-memory devices",
  },
  q8: {
    file: "nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf",
    name: "Nemotron Streaming 3.5 (Q8)",
    sizeHint: "~750 MB",
    qualityHint: "Higher-precision experimental option; needs more memory",
  },
};
