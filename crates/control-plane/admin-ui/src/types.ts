export interface Meeting {
  id: string
  org_id?: string
  title: string
  agenda?: string | null
  status: 'scheduled' | 'live' | 'ended'
  created_by?: string
  started_at?: string | null
  ended_at?: string | null
  scheduled_at?: string | null
  duration_minutes?: number
  lark_calendar_id?: string | null
  lark_event_id?: string | null
  lark_event_status?: 'pending' | 'created' | 'failed'
  created_at: string
}

export interface Participant {
  id: string
  account_id: string
  status: string
  joined_at?: string | null
  left_at?: string | null
  disconnect_count: number
  lark_name?: string
  name?: string
}

export interface Task {
  id: string
  title: string
  assignee?: string | null
  status: string
  lark_task_id?: string | null
}

export interface Decision {
  id: string
  text: string
}

export interface TranscriptChunk {
  speaker_id: string
  speaker_name?: string | null
  text: string
  timestamp_ms: number
  chunk_index: number
}

export interface MeetingDetail {
  meeting: Meeting
  participants: Participant[]
  summary?: string | null
  tasks: Task[]
  decisions: Decision[]
  transcript: TranscriptChunk[]
}

export interface OrgMember {
  account_id: string
  email?: string
  lark_name?: string
  lark_department?: string
  role: string
  auth_source?: string
  lark_connected?: boolean
  joined_at?: string
  desktop_active?: boolean
}

export interface DesktopClient {
  id: string
  account_id: string
  device_id: string
  platform: string
  app_version: string
  hostname?: string | null
  first_seen_at: string
  last_seen_at: string
  email?: string | null
  lark_name?: string | null
  lark_avatar_url?: string | null
  auth_source?: string
  lark_connected?: boolean
  company_bucket_version?: number
  company_vocab_synced_at?: string | null
  personal_vocab_count?: number
  personal_alias_count?: number
}

export interface OrgVocabTerm {
  id: string
  term: string
  term_norm: string
  term_type: string
  language: string
  weight: number
  priority: number
  status: string
  updated_at: string
}

export interface OrgVocabAlias {
  id: string
  transcript_form: string
  transcript_norm: string
  correct_form: string
  correct_norm: string
  language: string
  weight: number
  status: string
  safety_status: string
  updated_at: string
}

export interface OrgVocabSuggestion {
  id: string
  kind: 'term' | 'alias' | string
  term?: string | null
  transcript_form?: string | null
  correct_form?: string | null
  term_type?: string | null
  users_count: number
  total_positive_count: number
  total_negative_count: number
  confidence: number
  safety_status: string
  status: string
  updated_at: string
}

export interface OrgVocabRelease {
  id: string
  version: number
  bucket_hash: string
  notes?: string | null
  created_at: string
}

export interface DiagnosticsEvent {
  id: string
  device_id: string
  account_id?: string | null
  org_id?: string | null
  event_type: string
  severity: string
  app_version?: string | null
  os?: string | null
  arch?: string | null
  channel?: string | null
  phase?: string | null
  context: Record<string, unknown>
  created_at: string
}

export interface BugReport {
  id: string
  org_id?: string | null
  account_id?: string | null
  reporter_email?: string | null
  reporter_name?: string | null
  title: string
  description: string
  severity: 'low' | 'normal' | 'high' | 'blocking' | string
  status: 'open' | 'triaged' | 'fixed' | 'closed' | string
  app_version?: string | null
  platform?: string | null
  device_id?: string | null
  screenshot_url?: string | null
  screenshot_data_url?: string | null
  screenshot_name?: string | null
  screenshot_mime?: string | null
  created_at: string
  updated_at: string
}

export interface User {
  account: { id: string; email: string }
  license?: { tier: string }
}

export interface Org {
  id: string
  name: string
  slug: string
  role: string
  meeting_creator_roles?: string[]
}

export interface TelemetryUserRow {
  account_id: string
  email: string
  lark_name?: string | null
  role: string
  auth_source: string
  runs: number
  audio_minutes: number
  acceptance_rate: number
  edit_rate: number
  heavy_edit_rate: number
  fallback_rate: number
  learning_success_rate: number
  last_active_at?: string | null
  desktop_active: boolean
  primary_speech?: string | null
  costs: TelemetryCostSummary
}

export interface TelemetryCostSummary {
  stt_usd: number
  polish_usd: number
  total_usd: number
  cloud_stt_minutes: number
  runs: number
  stt_costed_runs: number
  polish_costed_runs: number
  coverage_rate: number
}

export interface TelemetryDailyCost {
  event_date: string
  runs: number
  stt_usd: number
  polish_usd: number
  total_usd: number
  coverage_rate: number
}

export interface TelemetryCostModelBreakdown {
  stt: {
    provider: string
    model: string
    runs: number
    audio_minutes: number
    cost_usd: number
  }[]
  polish: {
    provider: string
    model?: string | null
    attempts: number
    input_tokens: number
    output_tokens: number
    cost_usd: number
  }[]
}

export interface TelemetrySpeechBreakdown {
  by_model_path: { speech_model: string; speech_path: string; count: number }[]
  by_model: { speech_model: string; count: number; share: number }[]
  total_tagged: number
  latency_by_model?: {
    speech_model: string
    transcribe_p50: number | null
    transcribe_p95: number | null
    runs: number
  }[]
}

export interface TelemetryQuality {
  acceptance_rate: number
  edit_rate: number
  heavy_edit_rate: number
  fallback_rate: number
  learning_candidate_rate: number
  learning_success_rate: number
}

export interface TelemetryLatency {
  total_p50: number | null
  total_p95: number | null
  transcribe_p50: number | null
  transcribe_p95: number | null
  embed_p50?: number | null
  embed_p95?: number | null
  polish_p50?: number | null
  polish_p95?: number | null
  paste_p50?: number | null
  paste_p95?: number | null
}

export interface TelemetryDailyRollup {
  event_date: string
  mode: string
  run_count: number
  audio_seconds: number
  accepted_count: number
  edit_count: number
  heavy_edit_count: number
  learning_modal_shown: number
  learning_confirmed: number
  failure_count: number
  fallback_count: number
}

export interface TelemetryUpload {
  received_at: string
  device_id?: string | null
  client_version?: string | null
  run_count: number
  rollup_count: number
  accepted_count: number
  rejected_count: number
}

export interface TelemetryRunContentFlags {
  has_numbers: boolean
  has_currency: boolean
  has_percent: boolean
  has_email: boolean
  has_url: boolean
  has_code_like_terms: boolean
  mixed_language: boolean
  protected_term_hit: boolean
}

export interface TelemetryRun {
  run_id: string
  recording_id?: string | null
  device_id?: string | null
  mode: string
  target_app?: string | null
  platform?: string | null
  app_version?: string | null
  machine_class?: string | null
  audio_seconds?: number | null
  word_count?: number | null
  char_count?: number | null
  transcribe_ms?: number | null
  embed_ms?: number | null
  polish_ms?: number | null
  total_ms?: number | null
  paste_ms?: number | null
  success: boolean
  error_code?: string | null
  used_clipboard_fallback: boolean
  speech_provider?: string | null
  speech_model?: string | null
  speech_path?: string | null
  speech_cost_usd?: number | null
  speech_cost_source?: string | null
  polish_attempts: {
    provider: string
    model?: string | null
    input_tokens?: number | null
    output_tokens?: number | null
    cost_usd?: number | null
    cost_source?: string | null
    generation_id?: string | null
    status: string
    error_kind?: string | null
    created_at: string
  }[]
  polish_cost_usd?: number | null
  total_cost_usd?: number | null
  cost_coverage: 'complete' | 'partial' | 'unknown'
  edit_detected: boolean
  edit_bucket: string
  edit_distance_chars?: number | null
  edit_distance_words?: number | null
  accepted_as_is: boolean
  deleted_entire_output: boolean
  re_recorded_quickly: boolean
  learning_candidate: boolean
  learning_modal_shown: boolean
  learning_confirmed: boolean
  learning_dismissed: boolean
  server_learning_saved: boolean
  server_learning_blocked: boolean
  content_flags?: TelemetryRunContentFlags
  client_version?: string | null
  event_at: string
  received_at: string
}

export interface TelemetryUserProfile {
  window_days: number
  member: {
    account_id: string
    email: string
    lark_name?: string | null
    lark_department?: string | null
    role: string
    auth_source: string
    lark_connected: boolean
    desktop_active: boolean
  }
  summary: {
    runs: number
    audio_minutes: number
    word_count: number
    char_count: number
  }
  quality: TelemetryQuality
  quality_counts: {
    accepted_as_is: number
    edit_detected: number
    heavy_edit: number
    deleted_entire_output: number
    re_recorded_quickly: number
    failures: number
  }
  learning: {
    learning_candidate: number
    learning_modal_shown: number
    learning_confirmed: number
    learning_dismissed: number
    server_learning_saved: number
    server_learning_blocked: number
  }
  latency_ms: TelemetryLatency
  speech?: TelemetrySpeechBreakdown
  costs: {
    summary: TelemetryCostSummary
    daily: TelemetryDailyCost[]
    by_model: TelemetryCostModelBreakdown
    rate_card: {
      currency: string
      together_nemotron_per_hour: number
      gemma_input_per_million_tokens: number
      gemma_output_per_million_tokens: number
      effective_from: string
      provider_reported_polish_cost_preferred: boolean
    }
  }
  by_mode: { mode: string; count: number }[]
  by_target_app: { target_app: string | null; count: number }[]
  content_flags: Record<string, number>
  daily_rollups: TelemetryDailyRollup[]
  uploads: TelemetryUpload[]
}

export interface TelemetryUserMemory {
  hygiene: {
    memory_dirty_at?: string | null
    last_hygiene_at?: string | null
    hygiene_version: number
    pending_review: boolean
  }
  vocab_terms: {
    term: string
    term_type: string
    weight: number
    positive_count: number
    status: string
    source: string
  }[]
  aliases: {
    transcript_form: string
    correct_form: string
    weight: number
    positive_count: number
    status: string
    safety_status: string
    learned_speech_model?: string | null
  }[]
  edit_policies: {
    variant_form: string
    correct_form: string
    edit_type: string
    positive_count: number
    negative_count: number
    status: string
  }[]
  audit_log: {
    created_at: string
    action: string
    heard?: string | null
    correct?: string | null
    verdict: string
    reason: string
    model?: string | null
  }[]
  prompt_profile_latest?: {
    profile_source: string
    profile_markdown: string
    profile_chars: number
    profile_hash: string
    client_profile_version?: number | null
    last_run_id?: string | null
    updated_at: string
  } | null
  server_learned_profile?: {
    profile_markdown: string
    version: number
    status: string
    updated_at: string
  } | null
}

export interface ObservabilitySummary {
  window_days: number
  dictation_count: number
  aliases_learned: number
  edits_detected: number
  stt_error_edits: number
  classify_stt_error_rate: number
}

export interface DictationListItem {
  id: string
  account_id: string
  recording_id?: string | null
  client_run_id?: string | null
  target_app?: string | null
  word_count?: number | null
  recording_seconds?: number | null
  model_used?: string | null
  source: string
  created_at: string
  edit_bucket?: string | null
  edit_detected?: boolean | null
  total_ms?: number | null
  has_edit_feedback: boolean
}

export interface DictationTraceText {
  hash: string
  chars: number
  words: number
  redacted: boolean
  text: string
}

export interface DictationTraceStage {
  index: number
  stage: string
  component: string
  function: string
  input_ref?: string | null
  output_ref?: string | null
  changed: boolean
  duration_ms?: number | null
  reason?: string | null
  risk?: string | null
  metadata?: Record<string, unknown>
}

export interface DictationTrace {
  version: number
  texts: Record<string, DictationTraceText>
  stages: DictationTraceStage[]
  summary?: Record<string, unknown>
}

export interface DictationDetailItem {
  id: string
  account_id: string
  recording_id?: string | null
  raw_transcript?: string | null
  transcript?: string | null
  local_corrected_transcript?: string | null
  polished_output?: string | null
  final_text?: string | null
  target_app?: string | null
  model_used?: string | null
  word_count?: number | null
  edit_feedback_json?: Record<string, unknown>
  dictation_trace_json?: DictationTrace | Record<string, never>
  created_at: string
  edit_bucket?: string | null
  total_ms?: number | null
}

export interface AliasLearnEvent {
  id: string
  heard: string
  correct: string
  source: string
  safety?: string | null
  created_at: string
  recording_id?: string | null
}
