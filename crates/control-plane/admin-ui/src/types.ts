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
  lark_name?: string
  lark_department?: string
  role: string
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
