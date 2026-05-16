export interface SttReplacement {
  from: string;
  to: string;
  method: string; // "exact" | "phonetic"
}

export interface SttNearMiss {
  rule_from: string;
  rule_to: string;
  best_token: string;
  similarity: number;
}

export interface VocabSelected {
  term: string;
  term_type: string | null;
  weight: number;
  meaning: string | null;
  resolution: string; // "resolved" | "candidate"
}

export interface VocabRejected {
  term: string;
  reason: string;
  best_phonetic_sim: number;
}

export interface PipelineTrace {
  // Stage 1: STT
  deepgram_raw: string;
  stt_confidence: number;
  stt_ms: number;

  // Stage 2: STT Replacements
  stt_replacements_applied: SttReplacement[];
  stt_near_misses: SttNearMiss[];
  transcript_after_replacements: string;

  // Stage 3: Vocabulary Selection
  vocab_selected: VocabSelected[];
  vocab_rejected_sample: VocabRejected[];

  // Stage 4: Prompt
  system_prompt_preview: string;
  system_prompt_length: number;
  rag_examples_used: number;

  // Stage 5: LLM Polish
  polished: string;
  llm_model: string;
  llm_ms: number;

  // Stage 6: Post-processing
  devanagari_detected: boolean;
  format_recovered: boolean;
  content_guard_triggered: boolean;

  // Final
  final_output: string;
  total_ms: number;
}

export interface HistoryEntry {
  id: number;
  timestamp: Date;
  trace: PipelineTrace;
  audioUrl: string;
}
