import { useState } from 'react'
import { Bluetooth, CalendarDays, ChevronDown, Headphones, Mic2, Play, Radio, ShieldCheck } from 'lucide-react'

interface ReleaseSection {
  eyebrow: string
  title: string
  body: string[]
  videoSrc?: string
  posterSrc?: string
  bullets?: string[]
}

interface ReleaseGroup {
  title: string
  count: number
  items: string[]
}

const release = {
  version: '2.3.7',
  date: 'Jun 15, 2026',
  title: 'Bluetooth-safe dictation',
  intro: 'AirNote now avoids the macOS Bluetooth headset mode that could make music, calls, and system audio sound like a low-quality phone stream after starting dictation.',
  sections: [
    {
      eyebrow: '#Bluetooth Output',
      title: 'Bluetooth playback is no longer muted directly',
      body: [
        'When the active output device is Bluetooth or Bluetooth LE, AirNote skips the CoreAudio mute/unmute path entirely.',
        'Built-in speakers, wired output, and USB output still use speaker suppression. Bluetooth output is left alone to avoid corrupting the headset audio profile.',
      ],
      videoSrc: '/changelog/2.3.7-bluetooth-output.mp4',
      posterSrc: '/changelog/2.3.7-bluetooth-output.jpg',
      bullets: ['Detects Bluetooth transport via CoreAudio', 'Keeps existing suppression for non-Bluetooth devices', 'Prevents reconnect-only recovery loops'],
    },
    {
      eyebrow: '#Microphone Selection',
      title: 'AirNote avoids headset microphones when it can',
      body: [
        'Opening a Bluetooth headset microphone can force macOS into hands-free call mode. AirNote now prefers a built-in, display, or USB microphone when one is available.',
        'If the Bluetooth mic is the only usable input, recording still works. Admins and testers can also override this behavior with AIRNOTE_ALLOW_BLUETOOTH_MIC.',
      ],
      videoSrc: '/changelog/2.3.7-safe-mic-selection.mp4',
      posterSrc: '/changelog/2.3.7-safe-mic-selection.jpg',
      bullets: ['Shared by dictation and meeting capture', 'Falls back safely when no alternate mic exists', 'Logs selected input device for debugging'],
    },
    {
      eyebrow: '#Meeting Capture',
      title: 'Meeting mic capture uses the same guard',
      body: [
        'The meeting recorder now uses the same input-device selection as normal dictation, so long-running captures do not accidentally degrade Bluetooth playback.',
      ],
      videoSrc: '/changelog/2.3.7-meeting-capture.mp4',
      posterSrc: '/changelog/2.3.7-meeting-capture.jpg',
    },
  ] satisfies ReleaseSection[],
}

const groupedNotes: ReleaseGroup[] = [
  {
    title: 'Audio',
    count: 3,
    items: [
      'Skipped speaker suppression when the default output transport is Bluetooth or Bluetooth LE.',
      'Added a safer recorder input picker that avoids common Bluetooth headset microphones when another microphone is available.',
      'Documented AIRNOTE_ALLOW_BLUETOOTH_MIC and AIRNOTE_DISABLE_SPEAKER_SUPPRESSION for local testing.',
    ],
  },
  {
    title: 'Meetings',
    count: 1,
    items: [
      'Meeting microphone capture now uses the shared safe input picker before opening a cpal stream.',
    ],
  },
  {
    title: 'Verification',
    count: 3,
    items: [
      'Added tests for common Bluetooth headset input names.',
      'Added tests for Bluetooth transport detection in speaker suppression.',
      'Verified said-recorder tests, focused desktop speaker suppression tests, and the desktop cargo check.',
    ],
  },
]

function VideoPanel({ section }: { section: ReleaseSection }) {
  const [canPlayVideo, setCanPlayVideo] = useState(Boolean(section.videoSrc))

  return (
    <div className="group relative my-7 overflow-hidden rounded-[22px] border border-border bg-surface-2 shadow-[0_24px_70px_hsla(0,0%,0%,0.32)]">
      <div className="absolute inset-0 bg-[radial-gradient(circle_at_20%_20%,hsla(226,80%,78%,0.16),transparent_28%),radial-gradient(circle_at_84%_70%,hsla(190,70%,65%,0.12),transparent_30%)]" />
      {section.videoSrc && canPlayVideo ? (
        <video
          className="relative z-10 aspect-video w-full object-cover"
          poster={section.posterSrc}
          autoPlay
          muted
          loop
          playsInline
          preload="metadata"
          onError={() => setCanPlayVideo(false)}
          aria-label={`${section.title} release animation`}
        >
          <source src={section.videoSrc} type="video/mp4" />
        </video>
      ) : (
        <div className="relative z-10 flex aspect-video items-center justify-center">
          <div className="absolute left-[14%] top-[18%] h-[58%] w-[30%] rounded-3xl border border-white/10 bg-surface-3/70 shadow-2xl backdrop-blur-xl changelog-float" />
          <div className="absolute right-[13%] top-[14%] h-[46%] w-[34%] rounded-3xl border border-white/10 bg-surface-3/75 shadow-2xl backdrop-blur-xl changelog-float-delayed" />
          <div className="relative flex h-20 w-20 items-center justify-center rounded-full border border-accent/30 bg-accent-light shadow-[0_0_44px_var(--color-accent-glow)]">
            <Play size={22} className="ml-1 text-accent" fill="currentColor" />
          </div>
          <div className="absolute left-[26%] top-[28%] h-1.5 w-28 rounded-full bg-accent/70 changelog-scan" />
          <div className="absolute bottom-5 left-5 rounded-full border border-white/10 bg-surface-2/80 px-3 py-1.5 text-[10px] font-semibold text-fg-3 backdrop-blur-xl">
            Drop video at {section.videoSrc}
          </div>
        </div>
      )}
    </div>
  )
}

function SectionBlock({ section, icon: Icon }: { section: ReleaseSection; icon: typeof Bluetooth }) {
  return (
    <section className="border-t border-border-light py-10 first:border-t-0 first:pt-0">
      <div className="mb-3 flex items-center gap-2 text-[13px] font-semibold text-accent">
        <Icon size={16} />
        {section.eyebrow}
      </div>
      <h2 className="max-w-[760px] text-[30px] font-semibold leading-tight tracking-tight">{section.title}</h2>
      <div className="mt-4 max-w-[760px] space-y-4 text-[15px] leading-7 text-fg-3">
        {section.body.map(paragraph => <p key={paragraph}>{paragraph}</p>)}
      </div>

      <VideoPanel section={section} />

      {section.bullets?.length ? (
        <ul className="grid grid-cols-3 gap-3">
          {section.bullets.map(bullet => (
            <li key={bullet} className="rounded-2xl border border-border-light bg-surface-4/20 p-4 text-[12px] leading-relaxed text-fg-3">
              <ShieldCheck size={14} className="mb-2 text-ok" />
              {bullet}
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  )
}

function NotesGroup({ group, defaultOpen = false }: { group: ReleaseGroup; defaultOpen?: boolean }) {
  return (
    <details className="group border-t border-border-light py-5 first:border-t-0" open={defaultOpen}>
      <summary className="flex cursor-pointer list-none items-center justify-between gap-4">
        <div className="flex items-center gap-2.5">
          <h3 className="text-[15px] font-semibold">{group.title}</h3>
          <span className="rounded-full bg-surface-4 px-2 py-0.5 text-[10px] font-semibold text-fg-4">{group.count}</span>
        </div>
        <ChevronDown size={16} className="text-fg-4 transition-transform group-open:rotate-180" />
      </summary>
      <ul className="mt-4 space-y-2.5">
        {group.items.map(item => (
          <li key={item} className="flex gap-3 text-[13px] leading-relaxed text-fg-3">
            <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-accent" />
            <span>{item}</span>
          </li>
        ))}
      </ul>
    </details>
  )
}

export function ChangelogPage() {
  const icons = [Headphones, Mic2, Radio]

  return (
    <div className="min-h-screen bg-[hsl(60_8%_5%)] text-fg">
      <header className="mx-auto flex h-24 max-w-[1320px] items-center justify-between px-8">
        <a href="/changelog" className="flex items-center gap-2.5 text-[17px] font-semibold tracking-tight">
          <svg viewBox="0 0 24 24" width={25} height={25} className="text-fg">
            <rect x="3" y="8.5" width="3" height="7" rx="1.5" fill="currentColor" />
            <rect x="8" y="4.5" width="3" height="15" rx="1.5" fill="currentColor" />
            <rect x="13" y="2.5" width="3" height="19" rx="1.5" fill="currentColor" />
            <rect x="18" y="6.5" width="3" height="11" rx="1.5" fill="currentColor" />
          </svg>
          AirNote
        </a>

        <nav className="hidden items-center gap-9 text-[15px] font-medium text-fg-3 md:flex">
          <a className="transition-colors hover:text-fg" href="/changelog">Product</a>
          <a className="transition-colors hover:text-fg" href="/changelog">Enterprise</a>
          <a className="transition-colors hover:text-fg" href="/changelog">Pricing</a>
          <a className="transition-colors hover:text-fg" href="/changelog">Resources</a>
        </nav>

        <div className="hidden items-center gap-2 md:flex">
          <a className="px-3 py-2 text-[14px] font-medium text-fg-3 transition-colors hover:text-fg" href="/admin/login">Sign in</a>
          <a className="rounded-full border border-border px-4 py-2 text-[14px] font-medium text-fg-2 transition-colors hover:border-fg-4 hover:text-fg" href="/admin/login">Contact sales</a>
          <a className="rounded-full bg-fg px-4 py-2 text-[14px] font-semibold text-[hsl(60_8%_5%)] transition-opacity hover:opacity-90" href="/admin/login">Download</a>
        </div>
      </header>

      <main className="mx-auto grid max-w-[1320px] grid-cols-[240px_minmax(0,760px)_1fr] gap-10 px-8 pb-24 pt-28">
        <aside className="hidden pt-8 text-[16px] text-fg-4 lg:block">
          <div className="flex items-center gap-3">
            <span className="rounded-full border border-fg-4/60 px-3 py-0.5 text-[15px] leading-none text-fg-3">{release.version}</span>
            <span>{release.date}</span>
          </div>
        </aside>

        <article className="min-w-0">
          <header className="mb-16">
            <div className="mb-3 text-[18px] font-medium text-fg-4">Changelog</div>
            <h1 className="max-w-[780px] text-[56px] font-medium leading-[1.04] tracking-[-0.052em] text-fg">
              {release.title}
            </h1>
            <p className="mt-9 max-w-[690px] text-[19px] leading-8 text-fg-2">{release.intro}</p>
          </header>

          {release.sections.map((section, index) => (
            <SectionBlock key={section.title} section={section} icon={icons[index] ?? Bluetooth} />
          ))}

          <section className="border-t border-border-light pt-10">
            <div className="mb-4 flex items-center gap-2 text-[13px] font-semibold text-accent">
              <CalendarDays size={16} />
              #Release Notes
            </div>
            <h2 className="text-[30px] font-semibold tracking-tight">Detailed changes</h2>
            <p className="mt-3 max-w-[700px] text-[14px] leading-7 text-fg-4">
              Operational notes for the release. Keep this section factual and grouped so admins can quickly scan what changed.
            </p>

            <div className="mt-7 rounded-2xl border border-border bg-surface-3 px-5">
              {groupedNotes.map((group, index) => (
                <NotesGroup key={group.title} group={group} defaultOpen={index === 0} />
              ))}
            </div>
          </section>
        </article>
      </main>
    </div>
  )
}
