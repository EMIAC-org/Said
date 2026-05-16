import { useEffect, useRef, useCallback, useState } from 'react'

export type WsMessage =
  | { type: 'connected'; meeting_id: string; account_id: string }
  | { type: 'catchup'; summary: string | null; current_chunks: TranscriptChunk[]; tasks: WsTask[]; decisions: { text: string }[]; participants: WsParticipant[] }
  | { type: 'transcript_chunk'; speaker_id: string; speaker_name: string; text: string; timestamp_ms: number; chunk_index: number }
  | { type: 'task_detected'; task_id: string; title: string; assignee_id: string | null; assignee_name: string | null }
  | { type: 'summary_updated'; summary: string }
  | { type: 'participant_joined'; account_id: string; email: string }
  | { type: 'participant_left'; account_id: string }
  | { type: 'meeting_ended'; meeting_id: string }

export interface TranscriptChunk {
  speaker_id: string
  speaker_name: string
  text: string
  timestamp_ms: number
  chunk_index: number
}

export interface WsTask {
  task_id: string
  title: string
  assignee_name: string | null
  lark_task_id?: string | null
  status?: string
}

export interface WsParticipant {
  account_id: string
  email: string
  status: string
}

interface UseWebSocketOptions {
  meetingId: string
  token: string
  onMessage?: (msg: WsMessage) => void
}

export function useWebSocket({ meetingId, token, onMessage }: UseWebSocketOptions) {
  const wsRef = useRef<WebSocket | null>(null)
  const [connected, setConnected] = useState(false)
  const onMessageRef = useRef(onMessage)
  onMessageRef.current = onMessage

  useEffect(() => {
    if (!meetingId || !token) return

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const host = window.location.host
    const url = `${protocol}//${host}/v1/meetings/${meetingId}/ws?token=${encodeURIComponent(token)}`

    const ws = new WebSocket(url)
    wsRef.current = ws

    ws.onopen = () => setConnected(true)
    ws.onclose = () => setConnected(false)
    ws.onerror = () => setConnected(false)

    ws.onmessage = (e) => {
      try {
        const msg: WsMessage = JSON.parse(e.data)
        onMessageRef.current?.(msg)
      } catch {}
    }

    const pingInterval = setInterval(() => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'ping' }))
      }
    }, 25000)

    return () => {
      clearInterval(pingInterval)
      ws.close()
      wsRef.current = null
      setConnected(false)
    }
  }, [meetingId, token])

  const send = useCallback((data: unknown) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(data))
    }
  }, [])

  return { connected, send }
}
