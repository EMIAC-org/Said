import type { TelemetryRun } from '../../types'
import { ms } from './format'

function Bool({ v }: { v: boolean }) {
  return (
    <span className={v ? 'text-ok' : 'text-fg-5'}>{v ? 'yes' : 'no'}</span>
  )
}

function FieldGrid({ children }: { children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-3 gap-2 gap-x-4 text-[12px]">{children}</div>
  )
}

function Field({ k, v, mono }: { k: string; v: React.ReactNode; mono?: boolean }) {
  return (
    <div>
      <div className="text-fg-4 text-[11px]">{k}</div>
      <div className={`text-fg mt-0.5 tabular-nums ${mono ? 'font-mono text-[11px]' : ''}`}>{v}</div>
    </div>
  )
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="mt-3 first:mt-0">
      <div className="text-[11px] font-semibold text-fg-4 uppercase tracking-wider mb-2">{title}</div>
      {children}
    </div>
  )
}

export function RunDetailPanel({
  run,
  onOpenDictation,
}: {
  run: TelemetryRun
  onOpenDictation?: (recordingId: string) => void
}) {
  const flags = run.content_flags
  return (
    <div className="bg-surface-2 border-t border-border-light px-5 py-4">
      <Section title="Identity">
        <FieldGrid>
          <Field k="run_id" v={run.run_id} mono />
          <Field k="recording_id" v={run.recording_id || '—'} mono />
          {run.recording_id && onOpenDictation ? (
            <div className="col-span-3">
              <button
                type="button"
                onClick={() => onOpenDictation(run.recording_id!)}
                className="text-[11px] text-accent hover:underline bg-transparent border-0 cursor-pointer p-0"
              >
                Open dictation inspector →
              </button>
            </div>
          ) : null}
          <Field k="device_id" v={run.device_id || '—'} mono />
        </FieldGrid>
      </Section>
      <Section title="Context">
        <FieldGrid>
          <Field k="mode" v={run.mode} mono />
          <Field k="target_app" v={run.target_app || '—'} />
          <Field k="platform" v={run.platform || '—'} mono />
          <Field k="app_version" v={run.app_version || '—'} mono />
          <Field k="machine_class" v={run.machine_class || '—'} mono />
          <Field k="client_version" v={run.client_version || '—'} mono />
        </FieldGrid>
      </Section>
      <Section title="Audio & text">
        <FieldGrid>
          <Field k="audio_seconds" v={run.audio_seconds ?? '—'} />
          <Field k="word_count" v={run.word_count ?? '—'} />
          <Field k="char_count" v={run.char_count ?? '—'} />
        </FieldGrid>
      </Section>
      <Section title="Latency (ms)">
        <FieldGrid>
          <Field k="transcribe_ms" v={ms(run.transcribe_ms)} />
          <Field k="embed_ms" v={ms(run.embed_ms)} />
          <Field k="polish_ms" v={ms(run.polish_ms)} />
          <Field k="total_ms" v={ms(run.total_ms)} />
          <Field k="paste_ms" v={ms(run.paste_ms)} />
        </FieldGrid>
      </Section>
      <Section title="Speech">
        <FieldGrid>
          <Field k="speech_model" v={run.speech_model || '—'} mono />
          <Field k="speech_path" v={run.speech_path || '—'} mono />
        </FieldGrid>
      </Section>
      <Section title="Outcome & fallbacks">
        <FieldGrid>
          <Field k="success" v={<Bool v={run.success} />} />
          <Field k="error_code" v={run.error_code || '—'} mono />
          <Field k="used_clipboard_fallback" v={<Bool v={run.used_clipboard_fallback} />} />
        </FieldGrid>
      </Section>
      <Section title="Edit watch">
        <FieldGrid>
          <Field k="edit_detected" v={<Bool v={run.edit_detected} />} />
          <Field k="edit_bucket" v={run.edit_bucket} mono />
          <Field k="edit_distance_chars" v={run.edit_distance_chars ?? '—'} />
          <Field k="edit_distance_words" v={run.edit_distance_words ?? '—'} />
          <Field k="accepted_as_is" v={<Bool v={run.accepted_as_is} />} />
          <Field k="deleted_entire_output" v={<Bool v={run.deleted_entire_output} />} />
          <Field k="re_recorded_quickly" v={<Bool v={run.re_recorded_quickly} />} />
        </FieldGrid>
      </Section>
      <Section title="Learning">
        <FieldGrid>
          <Field k="learning_candidate" v={<Bool v={run.learning_candidate} />} />
          <Field k="learning_modal_shown" v={<Bool v={run.learning_modal_shown} />} />
          <Field k="learning_confirmed" v={<Bool v={run.learning_confirmed} />} />
          <Field k="learning_dismissed" v={<Bool v={run.learning_dismissed} />} />
          <Field k="server_learning_saved" v={<Bool v={run.server_learning_saved} />} />
          <Field k="server_learning_blocked" v={<Bool v={run.server_learning_blocked} />} />
        </FieldGrid>
      </Section>
      <Section title="Content flags">
        <div className="flex flex-wrap gap-1.5">
          {flags &&
            Object.entries(flags).map(([k, on]) => (
              <span
                key={k}
                className={`text-[10px] px-2 py-0.5 rounded ${
                  on ? 'bg-info-bg text-info' : 'bg-surface-4 text-fg-4'
                }`}
              >
                {k}
              </span>
            ))}
        </div>
      </Section>
      <Section title="Timestamps">
        <FieldGrid>
          <Field k="event_at" v={run.event_at} mono />
          <Field k="received_at" v={run.received_at} mono />
        </FieldGrid>
      </Section>
    </div>
  )
}
