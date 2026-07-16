import type { TelemetryRun, TelemetryUserRow, TelemetryUserProfile } from '../types'

/* ── Overview ───────────────────────────────────────────────────── */
export interface AdminOverview {
  window_days: number | null
  spend: { stt_usd: number; polish_usd: number; meeting_usd: number; total_usd: number }
  totals: { runs: number; words: number; audio_minutes: number; active_people: number; total_people: number }
  volume: { event_date: string; runs: number }[]
  top_people: {
    account_id: string
    name: string
    email: string
    platform?: string | null
    app_version?: string | null
    runs: number
    dictation_usd: number
    meeting_usd: number
    total_usd: number
  }[]
  recent_runs: {
    run_id: string
    account_id: string
    name: string
    target_app?: string | null
    word_count?: number | null
    total_cost_usd?: number | null
    cost_coverage: string
    event_at: string
  }[]
}

/* ── Org-wide runs feed (per-run shape + who ran it) ────────────── */
export type OrgRun = TelemetryRun & {
  account_id: string
  name?: string | null
  email?: string | null
  lark_name?: string | null
}

export interface OrgRunsResponse {
  window_days: number | null
  total: number
  limit: number
  offset: number
  runs: OrgRun[]
}

/* ── People (list rows + windowed cost) ─────────────────────────── */
export type PersonRow = TelemetryUserRow & {
  word_count?: number
  platform?: string | null
  app_version?: string | null
  meeting_cost_usd?: number
  meetings_hosted?: number
}

export interface PeopleResponse {
  window_days: number | null
  total: number
  limit: number
  offset: number
  users: PersonRow[]
}

/* ── Person detail (existing profile + meeting rollup) ──────────── */
export type PersonDetail = TelemetryUserProfile & {
  meeting_cost_usd?: number
  meetings_hosted?: number
  member: TelemetryUserProfile['member'] & { platform?: string | null; app_version?: string | null }
}

/* ── Meetings cost ──────────────────────────────────────────────── */
export interface MeetingCostRow {
  id: string
  title: string
  status: string
  created_at: string
  host_account_id: string
  host_name: string
  host_email: string
  participant_count: number
  input_tokens: number
  cached_input_tokens: number
  output_tokens: number
  cost_usd: number
}

export interface OrgMeetingCosts {
  window_days: number | null
  total_cost_usd: number
  total_tokens: number
  meetings: MeetingCostRow[]
}

export interface MeetingCostDetail {
  meeting_id: string
  model: string
  provider: string
  input_tokens: number
  cached_input_tokens: number
  output_tokens: number
  cost_usd: number
  by_slot: { slot_index: number | null; input_tokens: number; output_tokens: number; cost_usd: number }[]
}
