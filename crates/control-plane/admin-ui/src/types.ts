export interface Meeting {
  id: string
  org_id?: string
  title: string
  agenda?: string | null
  status: 'scheduled' | 'live' | 'ended'
  created_by?: string
  started_at?: string | null
  ended_at?: string | null
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
