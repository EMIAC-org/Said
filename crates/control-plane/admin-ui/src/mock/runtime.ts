// ──────────────────────────────────────────────────────────────────────────
// Mock data for the Runtime observability section.
//
// Shapes mirror the real control-plane Postgres schema (migration 013) so this
// can be swapped for live `/v1/runtime/*` admin endpoints in the next wave
// without touching the views:
//   runtime_sessions, runtime_stage_events, runtime_provider_usage,
//   personal_vocab_terms, personal_stt_replacements, runtime_learning_events,
//   org_vocab_terms, org_vocab_aliases, org_vocab_releases.
//
// All data is DETERMINISTIC (seeded PRNG) so the UI is stable across renders.
// ──────────────────────────────────────────────────────────────────────────

// ── Seeded PRNG (mulberry32) ────────────────────────────────────────────────
function mulberry32(seed: number) {
  let a = seed >>> 0
  return function () {
    a |= 0
    a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}
function strSeed(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 16777619)
  }
  return h >>> 0
}
function pick<T>(rng: () => number, arr: T[]): T {
  return arr[Math.floor(rng() * arr.length)]
}
function rangeInt(rng: () => number, lo: number, hi: number): number {
  return Math.floor(lo + rng() * (hi - lo + 1))
}

const DAY = 86_400_000
// Fixed "now" so timestamps are stable. Tuesday-ish anchor.
const NOW = 1_749_300_000_000 // ~2025-06-07

// ── Types (1:1 with schema-relevant columns) ────────────────────────────────
export interface RtUser {
  account_id: string
  email: string
  name: string
  role: 'admin' | 'manager' | 'member'
  department: string
  platform: 'macOS' | 'Windows'
  app_version: string
  last_active: string // ISO
  polish_count_7d: number
  words_7d: number
  acceptance_rate: number // 0..1 (kept unedited)
  avg_total_ms: number
  fail_rate: number // 0..1
  personal_vocab_count: number
  personal_alias_count: number
  daily: number[] // 7 polish counts, oldest→newest
}

export interface RtStage {
  stage: string
  status: 'ok' | 'error' | 'warning'
  latency_ms: number | null
  error_kind?: string | null
}

export interface RtPolish {
  id: string
  account_id: string
  created_at: string
  mode: 'normal_voice' | 'message_polish'
  source: 'desktop_voice' | 'runtime_wav_probe'
  status: 'completed' | 'failed'
  error_kind?: string | null
  model_used: string
  transcript: string
  output: string
  words: number
  accepted: boolean // user kept it without editing
  latency: { stt: number; polish: number; total: number }
  provider: { stt: string; llm: string }
  stages: RtStage[]
}

export interface RtVocabTerm {
  id: string
  term: string
  term_type: string
  source: string
  weight: number
  positive_count: number
  negative_count: number
  status: 'active' | 'archived'
  last_seen_at: string
}

export interface RtSttAlias {
  id: string
  transcript_form: string
  correct_form: string
  weight: number
  positive_count: number
  negative_count: number
  safety_status: 'safe' | 'unknown' | 'unsafe'
  status: 'active' | 'archived' | 'blocked'
  last_seen_at: string
}

export interface RtLearningEvent {
  id: string
  created_at: string
  event_type: 'accepted_edit' | 'rejected_edit' | 'learned' | 'corrected'
  classification: string
  detail: string
}

export interface RtCompanyTerm {
  id: string
  term: string
  term_type: string
  language: string
  priority: number
  status: 'approved' | 'draft' | 'archived'
  updated_at: string
}
export interface RtCompanyAlias {
  id: string
  transcript_form: string
  correct_form: string
  safety_status: 'safe' | 'unknown' | 'unsafe'
  status: 'approved' | 'draft' | 'archived'
  updated_at: string
}
export interface RtCompanyRelease {
  id: string
  version: number
  bucket_hash: string
  terms: number
  aliases: number
  notes: string
  created_at: string
}

export interface RtMetrics {
  total_polishes_7d: number
  total_polishes_prev_7d: number
  words_7d: number
  acceptance_rate: number
  acceptance_prev: number
  p50_ms: number
  p95_ms: number
  fail_rate: number
  active_users: number
  est_cost_usd_7d: number
  daily_volume: { day: string; count: number; accepted: number }[]
  acceptance_trend: number[] // 7 pts 0..1
  model_split: { label: string; value: number }[]
  source_split: { label: string; value: number }[]
  stage_latency: { stage: string; p50: number; p95: number }[]
}

// ── Source content (Hinglish dictation, realistic) ──────────────────────────
const PEOPLE: [string, string, RtUser['role'], string][] = [
  ['Aarav Sharma', 'aarav@emiactech.com', 'admin', 'Engineering'],
  ['Priya Verma', 'priya.verma@emiactech.com', 'manager', 'Product'],
  ['Rohan Gupta', 'rohan@emiactech.com', 'member', 'Engineering'],
  ['Ananya Iyer', 'ananya@emiactech.com', 'member', 'Design'],
  ['Kabir Khan', 'kabir.khan@emiactech.com', 'manager', 'Sales'],
  ['Diya Patel', 'diya@emiactech.com', 'member', 'Marketing'],
  ['Vivaan Reddy', 'vivaan@emiactech.com', 'member', 'Engineering'],
  ['Ishaan Nair', 'ishaan@emiactech.com', 'member', 'Support'],
  ['Saanvi Rao', 'saanvi@emiactech.com', 'manager', 'Operations'],
  ['Aditya Menon', 'aditya@emiactech.com', 'member', 'Engineering'],
  ['Meera Joshi', 'meera@emiactech.com', 'member', 'Product'],
  ['Karan Malhotra', 'karan@emiactech.com', 'member', 'Sales'],
]

const PAIRS: [string, string][] = [
  ['kal subah team standup hai 9 baje', 'Kal subah team standup hai 9 baje.'],
  ['please review the PR and merge it by EOD', 'Please review the PR and merge it by EOD.'],
  ['mujhe lagta hai hume deployment kal karna chahiye', 'Mujhe lagta hai humein deployment kal karna chahiye.'],
  ['send the invoice to accounts team aaj hi', 'Send the invoice to the accounts team today itself.'],
  ['Macobs ka integration almost ready hai', 'Macobs ka integration almost ready hai.'],
  ['lets sync at 4 pm regarding the roadmap', "Let's sync at 4 PM regarding the roadmap."],
  ['client ko bol do ki demo friday ko hoga', 'Client ko bol do ki demo Friday ko hoga.'],
  ['can you share the figma link in the channel', 'Can you share the Figma link in the channel?'],
  ['mai abhi airport ja raha hu call me later', "I'm heading to the airport right now, call me later."],
  ['update the deck before the board review please', 'Update the deck before the board review, please.'],
  ['ye bug production me aa raha hai urgent fix chahiye', 'Yeh bug production mein aa raha hai, urgent fix chahiye.'],
  ['schedule a one on one with the new hire next week', 'Schedule a one-on-one with the new hire next week.'],
  ['budget approve ho gaya hai aap proceed kar sakte ho', 'Budget approve ho gaya hai, aap proceed kar sakte ho.'],
  ['lets ship the beta to fifty users first', "Let's ship the beta to fifty users first."],
  ['mujhe analytics dashboard ka access chahiye', 'Mujhe analytics dashboard ka access chahiye.'],
  ['follow up with the vendor on the contract', 'Follow up with the vendor on the contract.'],
  ['standup me bata dena ki API migration done hai', 'Standup mein bata dena ki the API migration is done.'],
  ['can we move the release to monday morning', 'Can we move the release to Monday morning?'],
]

const FAIL_KINDS = ['deepgram_connect_failed', 'empty_transcript', 'polish_failed', 'model_failed']
const MODELS = [
  { name: 'llama-3.1-8b-instant', label: 'Fast', weight: 0.68 },
  { name: 'meta-llama/llama-4-scout-17b-16e-instruct', label: 'Smart', weight: 0.32 },
]

const VOCAB_TERMS = [
  ['Macobs', 'brand'], ['Airnote', 'brand'], ['Deepgram', 'brand'], ['Groq', 'brand'],
  ['EMIAC', 'acronym'], ['SKU', 'acronym'], ['Hinglish', 'other'], ['Postgres', 'code_identifier'],
  ['Tauri', 'code_identifier'], ['nova-3', 'code_identifier'], ['rustls', 'code_identifier'],
  ['Lark', 'brand'], ['Zoho Books', 'brand'], ['Divo', 'brand'], ['Cerebras', 'brand'],
]
const STT_ALIASES: [string, string][] = [
  ['mecobs', 'Macobs'], ['air note', 'Airnote'], ['deep gram', 'Deepgram'], ['grok', 'Groq'],
  ['e miac', 'EMIAC'], ['lurk', 'Lark'], ['devo', 'Divo'], ['nova three', 'nova-3'],
  ['russ tls', 'rustls'], ['hing lish', 'Hinglish'], ['tau ri', 'Tauri'], ['cerebra', 'Cerebras'],
]
const COMPANY_TERMS = [
  ['Macobs', 'brand', 9], ['Airnote', 'brand', 9], ['EMIAC', 'acronym', 8], ['Divo', 'brand', 7],
  ['Zoho Books', 'brand', 6], ['Lark Suite', 'brand', 6], ['Hinglish', 'other', 5],
  ['Control Plane', 'phrase', 4], ['BYOK', 'acronym', 5], ['Deepgram', 'brand', 7],
]
const COMPANY_ALIASES: [string, string][] = [
  ['mecobs', 'Macobs'], ['air note', 'Airnote'], ['e miac tech', 'EMIAC'],
  ['devo', 'Divo'], ['zoho box', 'Zoho Books'], ['control plain', 'Control Plane'],
]

// ── Generators ──────────────────────────────────────────────────────────────
function weightedModel(rng: () => number): { name: string; label: string } {
  return rng() < MODELS[0].weight
    ? { name: MODELS[0].name, label: MODELS[0].label }
    : { name: MODELS[1].name, label: MODELS[1].label }
}

function buildStages(rng: () => number, p: { source: string; status: string; latency: RtPolish['latency']; error_kind?: string | null }): RtStage[] {
  const out: RtStage[] = []
  const stt = p.latency.stt
  if (p.source === 'runtime_wav_probe') {
    out.push({ stage: 'stt_batch_complete', status: 'ok', latency_ms: stt })
  } else {
    out.push({ stage: 'stt_ws_connected', status: 'ok', latency_ms: rangeInt(rng, 180, 520) })
    out.push({ stage: 'first_audio_frame', status: 'ok', latency_ms: rangeInt(rng, 20, 90) })
    out.push({ stage: 'stt_first_transcript', status: 'ok', latency_ms: Math.round(stt * 0.4) })
    out.push({ stage: 'stt_final', status: 'ok', latency_ms: stt })
  }
  if (p.status === 'failed') {
    const kind = p.error_kind || 'polish_failed'
    if (kind === 'deepgram_connect_failed') {
      return [{ stage: 'stt_ws_connect', status: 'error', latency_ms: 6001, error_kind: kind }]
    }
    if (kind === 'empty_transcript') {
      out.push({ stage: 'stt_final', status: 'warning', latency_ms: stt, error_kind: 'empty_transcript' })
      return out
    }
    out.push({ stage: 'prompt_built', status: 'ok', latency_ms: rangeInt(rng, 1, 6) })
    out.push({ stage: 'llm_complete', status: 'error', latency_ms: p.latency.polish, error_kind: kind })
    return out
  }
  out.push({ stage: 'prompt_built', status: 'ok', latency_ms: rangeInt(rng, 1, 6) })
  out.push({ stage: 'llm_complete', status: 'ok', latency_ms: p.latency.polish })
  return out
}

export function getUsers(): RtUser[] {
  return PEOPLE.map(([name, email, role, dept], i) => {
    const rng = mulberry32(strSeed(email))
    const platform: RtUser['platform'] = rng() < 0.78 ? 'macOS' : 'Windows'
    const daily = Array.from({ length: 7 }, () => rangeInt(rng, i === 0 ? 8 : 0, i < 4 ? 34 : 18))
    const polish_count = daily.reduce((a, b) => a + b, 0)
    const acceptance = 0.62 + rng() * 0.32
    const fail = rng() * (i % 5 === 0 ? 0.12 : 0.04)
    const lastAgo = rangeInt(rng, 4, i < 4 ? 90 : 1400) // minutes
    return {
      account_id: `acc_${strSeed(email).toString(16)}`,
      email,
      name,
      role,
      department: dept,
      platform,
      app_version: pick(rng, ['2.3.4', '2.3.3', '2.3.2']),
      last_active: new Date(NOW - lastAgo * 60_000).toISOString(),
      polish_count_7d: polish_count,
      words_7d: polish_count * rangeInt(rng, 7, 16),
      acceptance_rate: acceptance,
      avg_total_ms: rangeInt(rng, 820, 1850),
      fail_rate: fail,
      personal_vocab_count: rangeInt(rng, 3, 14),
      personal_alias_count: rangeInt(rng, 1, 10),
      daily,
    }
  })
}

export function getUser(accountId: string): RtUser | undefined {
  return getUsers().find((u) => u.account_id === accountId)
}

export function getPolishes(accountId: string, limit = 60): RtPolish[] {
  const u = getUser(accountId)
  if (!u) return []
  const rng = mulberry32(strSeed(accountId + ':polishes'))
  const n = Math.min(limit, Math.max(8, Math.round(u.polish_count_7d * 0.5)))
  const out: RtPolish[] = []
  let t = NOW - rangeInt(rng, 5, 40) * 60_000
  for (let i = 0; i < n; i++) {
    t -= rangeInt(rng, 12, 240) * 60_000 // walk back in time
    const [transcript, output] = pick(rng, PAIRS)
    const failed = rng() < u.fail_rate * 1.6
    const error_kind = failed ? pick(rng, FAIL_KINDS) : null
    const model = weightedModel(rng)
    const source: RtPolish['source'] = rng() < 0.8 ? 'desktop_voice' : 'runtime_wav_probe'
    const mode: RtPolish['mode'] = rng() < 0.82 ? 'normal_voice' : 'message_polish'
    const stt = error_kind === 'deepgram_connect_failed' ? 6001 : rangeInt(rng, 320, 1100)
    const polish = failed ? rangeInt(rng, 120, 700) : rangeInt(rng, 260, 1150)
    const total = error_kind === 'deepgram_connect_failed' ? 6001 : stt + polish + rangeInt(rng, 20, 120)
    const accepted = !failed && rng() < u.acceptance_rate
    out.push({
      id: `run_${strSeed(accountId + i).toString(16)}`,
      account_id: accountId,
      created_at: new Date(t).toISOString(),
      mode,
      source,
      status: failed ? 'failed' : 'completed',
      error_kind,
      model_used: model.name,
      transcript,
      output: failed ? '' : output,
      words: output.split(/\s+/).filter(Boolean).length,
      accepted,
      latency: { stt, polish, total },
      provider: { stt: 'deepgram · nova-3', llm: `groq · ${model.label}` },
      stages: buildStages(rng, { source, status: failed ? 'failed' : 'completed', latency: { stt, polish, total }, error_kind }),
    })
  }
  return out
}

// Global server-log feed — every user's polishes merged, newest first.
export interface RtLogEntry extends RtPolish {
  user_name: string
  user_email: string
  user_role: RtUser['role']
  user_department: string
}
export function getAllPolishes(limit = 160): RtLogEntry[] {
  const all: RtLogEntry[] = []
  for (const u of getUsers()) {
    for (const p of getPolishes(u.account_id)) {
      all.push({
        ...p,
        user_name: u.name,
        user_email: u.email,
        user_role: u.role,
        user_department: u.department,
      })
    }
  }
  all.sort((a, b) => (a.created_at < b.created_at ? 1 : -1))
  return all.slice(0, limit)
}

export function getPersonalVocab(accountId: string): RtVocabTerm[] {
  const u = getUser(accountId)
  if (!u) return []
  const rng = mulberry32(strSeed(accountId + ':vocab'))
  const shuffled = [...VOCAB_TERMS].sort(() => rng() - 0.5).slice(0, u.personal_vocab_count)
  return shuffled.map(([term, type], i) => {
    const pos = rangeInt(rng, 1, 30)
    return {
      id: `pv_${strSeed(accountId + term).toString(16)}`,
      term,
      term_type: type,
      source: pick(rng, ['server_runtime', 'edit_learn', 'local_sync']),
      weight: +(0.6 + rng() * 2.2).toFixed(2),
      positive_count: pos,
      negative_count: rangeInt(rng, 0, 3),
      status: rng() < 0.92 ? 'active' : 'archived',
      last_seen_at: new Date(NOW - rangeInt(rng, 0, 6) * DAY - i * 3600_000).toISOString(),
    }
  })
}

export function getSttAliases(accountId: string): RtSttAlias[] {
  const u = getUser(accountId)
  if (!u) return []
  const rng = mulberry32(strSeed(accountId + ':alias'))
  const shuffled = [...STT_ALIASES].sort(() => rng() - 0.5).slice(0, u.personal_alias_count)
  return shuffled.map(([from, to], i) => {
    const pos = rangeInt(rng, 1, 22)
    const neg = rangeInt(rng, 0, 4)
    return {
      id: `sa_${strSeed(accountId + from).toString(16)}`,
      transcript_form: from,
      correct_form: to,
      weight: +(0.8 + rng() * 1.8).toFixed(2),
      positive_count: pos,
      negative_count: neg,
      safety_status: neg > 2 ? 'unknown' : 'safe',
      status: neg > 3 ? 'blocked' : rng() < 0.94 ? 'active' : 'archived',
      last_seen_at: new Date(NOW - rangeInt(rng, 0, 6) * DAY - i * 3600_000).toISOString(),
    }
  })
}

export function getLearningEvents(accountId: string, limit = 18): RtLearningEvent[] {
  const rng = mulberry32(strSeed(accountId + ':learn'))
  const types: RtLearningEvent['event_type'][] = ['accepted_edit', 'rejected_edit', 'learned', 'corrected']
  const classes = ['alias_replacement', 'vocab_promotion', 'formatting_only', 'common_word_blocked', 'edit_policy_rule']
  const details = [
    'mecobs → Macobs', 'air note → Airnote', 'formatting-only edit ignored', 'kaisa blocked (common word)',
    'learned alias devo → Divo', 'e miac → EMIAC', 'reverted nova three → nova-3', 'promoted to personal vocab',
  ]
  const n = rangeInt(rng, 8, limit)
  let t = NOW - rangeInt(rng, 10, 120) * 60_000
  return Array.from({ length: n }, (_, i) => {
    t -= rangeInt(rng, 30, 600) * 60_000
    return {
      id: `le_${strSeed(accountId + i).toString(16)}`,
      created_at: new Date(t).toISOString(),
      event_type: pick(rng, types),
      classification: pick(rng, classes),
      detail: pick(rng, details),
    }
  })
}

export function getMetrics(): RtMetrics {
  const users = getUsers()
  const total = users.reduce((a, u) => a + u.polish_count_7d, 0)
  const rng = mulberry32(strSeed('org-metrics'))
  const dayNames = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']
  const daily_volume = Array.from({ length: 7 }, (_, i) => {
    const count = users.reduce((a, u) => a + (u.daily[i] || 0), 0)
    return { day: dayNames[i], count, accepted: Math.round(count * (0.7 + rng() * 0.18)) }
  })
  const acceptance_rate = users.reduce((a, u) => a + u.acceptance_rate * u.polish_count_7d, 0) / Math.max(total, 1)
  const words = users.reduce((a, u) => a + u.words_7d, 0)
  const fastShare = MODELS[0].weight
  return {
    total_polishes_7d: total,
    total_polishes_prev_7d: Math.round(total * 0.86),
    words_7d: words,
    acceptance_rate,
    acceptance_prev: acceptance_rate - 0.04,
    p50_ms: 1140,
    p95_ms: 2380,
    fail_rate: users.reduce((a, u) => a + u.fail_rate, 0) / users.length,
    active_users: users.filter((u) => Date.now() - 0 && NOW - new Date(u.last_active).getTime() < 2 * DAY).length,
    est_cost_usd_7d: +(total * 0.00042 + words * 0.0000009).toFixed(2),
    daily_volume,
    acceptance_trend: Array.from({ length: 7 }, (_, i) => 0.66 + (i / 7) * 0.12 + (rng() - 0.5) * 0.05),
    model_split: [
      { label: 'Fast · 8b-instant', value: Math.round(total * fastShare) },
      { label: 'Smart · llama-4-scout', value: Math.round(total * (1 - fastShare)) },
    ],
    source_split: [
      { label: 'Desktop live', value: Math.round(total * 0.8) },
      { label: 'WAV probe', value: Math.round(total * 0.2) },
    ],
    stage_latency: [
      { stage: 'STT first transcript', p50: 410, p95: 980 },
      { stage: 'STT final', p50: 720, p95: 1450 },
      { stage: 'Prompt build', p50: 3, p95: 9 },
      { stage: 'LLM polish', p50: 560, p95: 1280 },
    ],
  }
}

export function getCompanyBucket(): {
  terms: RtCompanyTerm[]
  aliases: RtCompanyAlias[]
  releases: RtCompanyRelease[]
} {
  const rng = mulberry32(strSeed('company-bucket'))
  const terms: RtCompanyTerm[] = COMPANY_TERMS.map(([term, type, prio]) => ({
    id: `ct_${strSeed('t' + term).toString(16)}`,
    term: term as string,
    term_type: type as string,
    language: 'hinglish',
    priority: prio as number,
    status: rng() < 0.85 ? 'approved' : 'draft',
    updated_at: new Date(NOW - rangeInt(rng, 0, 20) * DAY).toISOString(),
  }))
  const aliases: RtCompanyAlias[] = COMPANY_ALIASES.map(([from, to]) => ({
    id: `ca_${strSeed('a' + from).toString(16)}`,
    transcript_form: from,
    correct_form: to,
    safety_status: rng() < 0.8 ? 'safe' : 'unknown',
    status: rng() < 0.85 ? 'approved' : 'draft',
    updated_at: new Date(NOW - rangeInt(rng, 0, 20) * DAY).toISOString(),
  }))
  const releases: RtCompanyRelease[] = Array.from({ length: 4 }, (_, i) => {
    const v = 4 - i
    return {
      id: `rel_${v}`,
      version: v,
      bucket_hash: 'sha256:' + strSeed('rel' + v).toString(16).padStart(8, '0') + 'a1b2c3d4',
      terms: 10 - i,
      aliases: 6 - i,
      notes: ['Added BYOK + Divo terms', 'Promoted 3 aliases from suggestions', 'Initial company bucket', 'Hotfix: blocked unsafe alias'][i],
      created_at: new Date(NOW - (i * 6 + 2) * DAY).toISOString(),
    }
  })
  return { terms, aliases, releases }
}
