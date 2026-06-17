import type { Metadata } from "next";
import {
  ArrowRight,
  Bug,
  CheckCircle2,
  Download,
  Keyboard,
  Languages,
  MapPin,
  MessageSquareText,
  Mic2,
  Move,
  ShieldCheck,
  Sparkles,
} from "lucide-react";
import { downloads } from "@/lib/content";

export const metadata: Metadata = {
  title: "Airnote User Guide",
  description:
    "Airnote shortcuts, Polish Mode, selected-text cleanup, status pill placement, bug reporting, and permissions.",
};

const shortcuts = [
  {
    keys: ["Fn", "hold"],
    title: "Fast dictation",
    body: "Record while held. Release to process and type the final output into the active app.",
  },
  {
    keys: ["Fn", "Space"],
    title: "Long dictation lock",
    body: "Start recording, lock it, then press Fn again to finish.",
  },
  {
    keys: ["Shift", "Cmd", "Space"],
    title: "Toggle Polish Mode",
    body: "Airnote waits for the final polished message and pastes it once.",
  },
  {
    keys: ["Option", "1"],
    title: "Polish selected message",
    body: "Select text in any app and replace only that selection with polished English.",
  },
  {
    keys: ["Option", "2"],
    title: "English cleanup",
    body: "Selected-text English conversion and cleanup shortcut.",
  },
  {
    keys: ["Option", "5"],
    title: "Hinglish cleanup",
    body: "Selected-text Hinglish cleanup shortcut.",
  },
  {
    keys: ["Ctrl", "Cmd", "V"],
    title: "Paste again",
    body: "Paste the last Airnote output again if the target app rejected the first paste.",
  },
];

const workflows = [
  {
    icon: Sparkles,
    title: "Polish Mode Workflow",
    tag: "Voice mode",
    demo: "bhai client ko bol do ki documentation done hai and we can discuss scope today evening",
    output:
      "Hi, the documentation is complete. We can review the approach and finalize the scope during the call this evening.",
    steps: [
      "Press Shift + Cmd + Space. The pill shows Polish mode on.",
      "Speak normally with Fn. Airnote transcribes first, then runs a message-polish pass.",
      "The target app receives only the final polished message. No draft text is streamed into the field.",
    ],
  },
  {
    icon: Move,
    title: "Move The Status Pill",
    tag: "Placement",
    demo: "The pill can sit near the bottom, near the notch, or wherever it does not block your work.",
    output: "Drag me",
    steps: [
      "Press Shift + Cmd + / to make the status pill movable.",
      "Drag the pill anywhere that feels comfortable for your screen.",
      "Press Shift + Cmd + . to reset it to the centered default position.",
    ],
  },
  {
    icon: MessageSquareText,
    title: "Polish Selected Text With Option + 1",
    tag: "Selected text",
    demo:
      "Let me know if you want to jump on call right away i will set it up in few minutes and then you can query anything in Zoho Books using your Claude account.",
    output:
      "Please let me know if you would like to get on a call right away. I can set it up in a few minutes, and then you can ask anything about Zoho Books using your Claude account.",
    steps: [
      "Select the rough message in Slack, WhatsApp, Gmail, Chrome, Notes, or any editable app.",
      "Press Option + 1. Use the left Option key if your keyboard has two.",
      "Airnote replaces only the selected text. It should not hit Enter or send the message.",
    ],
  },
];

const vocabulary = [
  {
    label: "Personal",
    title: "Your Mac learns locally",
    body: "Edit a proper noun, brand, acronym, or code word after dictation. Airnote learns that correction on your machine.",
  },
  {
    label: "Company",
    title: "Company bucket",
    body: "Enterprise users can receive approved company-wide words on day one, without rebuilding personal vocabulary from scratch.",
  },
  {
    label: "Safety",
    title: "Common words are blocked",
    body: "Common Hindi, Hinglish, and English words should not become aliases for company terms.",
  },
];

const permissions = [
  ["Accessibility", "Types or replaces text in the focused app."],
  ["Input Monitoring", "Detects global shortcuts like Fn, Option + 1, and Shift + Command + Space."],
  ["Microphone", "Records your voice for dictation."],
];

function AirnoteMark({ className = "h-6 w-6" }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden className={className}>
      <rect x="3" y="8.5" width="3" height="7" rx="1.5" />
      <rect x="8" y="4.5" width="3" height="15" rx="1.5" />
      <rect x="13" y="2.5" width="3" height="19" rx="1.5" />
      <rect x="18" y="6.5" width="3" height="11" rx="1.5" />
    </svg>
  );
}

function Key({ children }: { children: string }) {
  return (
    <span className="inline-flex min-h-7 items-center justify-center rounded-md border border-white/12 bg-white/[0.055] px-2.5 font-mono text-[11px] font-black text-white shadow-[inset_0_-1px_0_rgba(255,255,255,0.08)]">
      {children}
    </span>
  );
}

function GuideDemo({
  mode,
  input,
  output,
}: {
  mode: string;
  input: string;
  output: string;
}) {
  return (
    <div className="relative overflow-hidden rounded-2xl border border-white/10 bg-[#0d1016] p-4 shadow-[0_30px_90px_-60px_rgba(0,0,0,0.9)]">
      <div className="relative h-[230px] overflow-hidden rounded-xl border border-white/10 bg-[#171b23] md:h-[260px]">
        <div className="h-9 border-b border-white/8 bg-white/[0.055]" />
        <div className="absolute left-1/2 top-0 h-5 w-28 -translate-x-1/2 rounded-b-2xl bg-black" />
        <div className="absolute inset-x-5 top-14 rounded-xl border border-white/10 bg-[#0f131b] p-4 text-sm leading-6 text-white/58">
          <p>{input}</p>
          <div className="mt-4 rounded-lg border border-accent/20 bg-accent/8 p-3 text-white">
            {output}
          </div>
        </div>
        <div className="absolute bottom-5 left-1/2 inline-flex h-10 -translate-x-1/2 items-center gap-2 rounded-full border border-white/10 bg-black px-4 text-xs font-bold text-white shadow-[0_18px_45px_rgba(0,0,0,0.5)]">
          <span className="inline-flex h-4 items-center gap-1">
            <span className="h-2 w-1 animate-[guideBar_1s_ease-in-out_infinite] rounded-full bg-accent" />
            <span className="h-3 w-1 animate-[guideBar_1s_ease-in-out_infinite_.12s] rounded-full bg-accent" />
            <span className="h-4 w-1 animate-[guideBar_1s_ease-in-out_infinite_.24s] rounded-full bg-accent" />
            <span className="h-3 w-1 animate-[guideBar_1s_ease-in-out_infinite_.36s] rounded-full bg-accent" />
          </span>
          {mode}
        </div>
      </div>
    </div>
  );
}

export default function GuidePage() {
  return (
    <main className="min-h-screen overflow-hidden bg-[#090a0d] text-white">
      <style>{`
        @keyframes guideBar {
          0%, 100% { transform: scaleY(.55); opacity: .6; }
          50% { transform: scaleY(1.15); opacity: 1; }
        }
      `}</style>

      <div className="pointer-events-none absolute inset-x-0 top-0 h-[680px] bg-[radial-gradient(circle_at_18%_0%,rgba(120,168,255,.14),transparent_30%),radial-gradient(circle_at_86%_10%,rgba(98,230,184,.12),transparent_28%)]" />
      <div className="pointer-events-none absolute left-1/2 top-[320px] h-[680px] w-[1120px] -translate-x-1/2 rounded-full bg-accent/8 blur-[150px]" />

      <header className="container-page relative z-10 flex items-center justify-between py-7">
        <a href="/" className="flex items-center gap-2.5">
          <AirnoteMark className="h-6 w-6 text-accent" />
          <span className="font-display text-lg tracking-tight">Airnote</span>
        </a>
        <nav className="flex items-center gap-1 text-sm text-white/70">
          <a href="/" className="rounded-full px-4 py-2 transition-colors hover:bg-white/8 hover:text-white">
            Product
          </a>
          <a href="/changelog" className="rounded-full px-4 py-2 transition-colors hover:bg-white/8 hover:text-white">
            Changelog
          </a>
          <a
            href={downloads.mac.latestDmg}
            className="rounded-full bg-white px-4 py-2 font-medium text-ink-900 transition-colors hover:bg-ink-50"
          >
            Download
          </a>
        </nav>
      </header>

      <section className="container-page relative z-10 grid gap-10 pb-16 pt-16 lg:grid-cols-[minmax(0,1.04fr)_minmax(360px,.96fr)] lg:items-end lg:pt-24">
        <div>
          <div className="mb-6 inline-flex items-center gap-2 rounded-full border border-white/10 bg-white/[0.045] px-3 py-1.5 text-xs font-semibold uppercase tracking-[0.18em] text-accent">
            <BookOpenIcon />
            User Guide
          </div>
          <h1 className="max-w-3xl font-display text-5xl leading-[0.98] tracking-tightest text-white md:text-7xl">
            Speak anywhere. Polish only when you choose.
          </h1>
          <p className="mt-7 max-w-2xl text-lg leading-8 text-white/68">
            Airnote types into the active app, keeps the small status pill visible, and gives you focused shortcuts for dictation, message polish, HUD placement, and selected-text cleanup.
          </p>
          <div className="mt-9 flex flex-wrap gap-3">
            <a
              href="#shortcuts"
              className="inline-flex h-11 items-center gap-2 rounded-full bg-white px-5 text-sm font-medium text-ink-900 transition-colors hover:bg-ink-50"
            >
              See shortcuts
              <ArrowRight className="h-4 w-4" />
            </a>
            <a
              href={downloads.mac.latestDmg}
              className="inline-flex h-11 items-center gap-2 rounded-full border border-white/10 bg-black/20 px-5 text-sm font-medium text-white/75 backdrop-blur-md transition-colors hover:bg-white/8 hover:text-white"
            >
              <Download className="h-4 w-4" />
              Download Airnote
            </a>
          </div>
        </div>
        <GuideDemo
          mode="Polishing"
          input="hello aron i am done with detailed documentation can walk you through approach and finalise scope before execution"
          output="Hello Aaron, I have completed the detailed documentation. I can walk you through the approach, and then we can finalize the scope before proceeding with execution."
        />
      </section>

      <section id="shortcuts" className="container-page relative z-10 py-12">
        <div className="mb-7 flex items-center gap-3">
          <Keyboard className="h-5 w-5 text-accent" />
          <h2 className="font-display text-3xl tracking-tight text-white">Core shortcuts</h2>
        </div>
        <div className="grid gap-3 md:grid-cols-2">
          {shortcuts.map((shortcut) => (
            <article key={shortcut.title} className="rounded-2xl border border-white/10 bg-white/[0.035] p-5">
              <div className="mb-4 flex flex-wrap gap-1.5">
                {shortcut.keys.map((key) => (
                  <Key key={key}>{key}</Key>
                ))}
              </div>
              <h3 className="text-base font-semibold text-white">{shortcut.title}</h3>
              <p className="mt-2 text-sm leading-6 text-white/58">{shortcut.body}</p>
            </article>
          ))}
        </div>
        <div className="mt-5 rounded-2xl border border-accent/20 bg-accent/8 p-5 text-sm leading-6 text-white/68">
          <strong className="text-white">Rule of thumb:</strong> normal dictation is fast and direct. Polish Mode is for messages that should sound professional, so it intentionally waits for the final polished version and pastes once.
        </div>
      </section>

      <section className="container-page relative z-10 grid gap-8 py-10">
        {workflows.map((workflow) => {
          const Icon = workflow.icon;
          return (
            <article key={workflow.title} className="grid gap-5 border-t border-white/10 py-10 lg:grid-cols-[minmax(0,.95fr)_minmax(360px,1.05fr)] lg:items-center">
              <div className="rounded-3xl border border-white/10 bg-white/[0.035] p-6">
                <div className="mb-5 inline-flex items-center gap-2 rounded-full bg-accent px-3 py-1.5 text-xs font-black uppercase tracking-[0.14em] text-[#07110d]">
                  <Icon className="h-3.5 w-3.5" />
                  {workflow.tag}
                </div>
                <h2 className="font-display text-3xl tracking-tight text-white">{workflow.title}</h2>
                <div className="mt-5 space-y-3">
                  {workflow.steps.map((step, index) => (
                    <div key={step} className="flex gap-3 border-b border-white/[0.055] pb-3 last:border-b-0 last:pb-0">
                      <span className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-blue-400/14 text-xs font-black text-blue-300">
                        {index + 1}
                      </span>
                      <p className="text-sm leading-6 text-white/62">{step}</p>
                    </div>
                  ))}
                </div>
              </div>
              <GuideDemo mode={workflow.tag} input={workflow.demo} output={workflow.output} />
            </article>
          );
        })}
      </section>

      <section className="container-page relative z-10 grid gap-6 border-t border-white/10 py-12 lg:grid-cols-[.85fr_1.15fr] lg:items-center">
        <div>
          <div className="mb-4 flex items-center gap-2 text-sm font-semibold text-accent">
            <Languages className="h-4 w-4" />
            What polish means
          </div>
          <h2 className="font-display text-3xl tracking-tight text-white">Cleaner language, same facts.</h2>
          <p className="mt-4 text-sm leading-7 text-white/60">
            Polish Mode improves language and structure, but it should preserve names, numbers, rates, dates, legal/commercial facts, questions, and intent.
          </p>
        </div>
        <div className="rounded-2xl border border-white/10 bg-[#0f1218] p-5 text-sm leading-7">
          <p className="text-white/45">
            Hello Aaron, I'm done with detailed documentation. I can walk you through approach and then finalise scope before execution. Let me know time for evening call as per your time zone.
          </p>
          <p className="my-3 font-mono text-xs text-white/30">Option + 1 / Polish Mode output</p>
          <p className="text-accent">
            Hello Aaron, I have completed the detailed documentation. I can walk you through the approach, and then we can finalize the scope before proceeding with execution. Please let me know a suitable time for the evening call based on your time zone.
          </p>
        </div>
      </section>

      <section className="container-page relative z-10 grid gap-6 border-t border-white/10 py-12 lg:grid-cols-2">
        <div className="rounded-3xl border border-white/10 bg-white/[0.035] p-6">
          <div className="mb-4 flex items-center gap-2 text-sm font-semibold text-accent">
            <Bug className="h-4 w-4" />
            Report a bug
          </div>
          <h2 className="font-display text-3xl tracking-tight text-white">Send issues directly to the team.</h2>
          <div className="mt-5 space-y-3">
            {[
              "Open Airnote and click Report bug in the sidebar.",
              "Airnote opens the control-plane bug form with a secure report token plus app version, platform, and device details.",
              "Describe what happened. Attach a screenshot if it helps. The report appears in the admin Bugs page for the team.",
            ].map((step, index) => (
              <div key={step} className="flex gap-3">
                <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-accent" />
                <p className="text-sm leading-6 text-white/62">{step}</p>
              </div>
            ))}
          </div>
        </div>
        <div className="rounded-3xl border border-white/10 bg-[#f7f8fc] p-5 text-[#0c0e1a] shadow-[0_30px_100px_-70px_rgba(255,255,255,0.35)]">
          <div className="rounded-2xl border border-[#dfe3ef] bg-white p-5">
            <h3 className="text-3xl font-semibold tracking-tight">Report a bug</h3>
            <p className="mt-2 text-sm text-[#697292]">Tell us what happened in Airnote. Add a screenshot if it helps.</p>
            <div className="mt-5 grid gap-3">
              <div className="rounded-xl border border-[#dfe3ef] bg-[#f7f8fc] px-3 py-2 text-sm text-[#7d8291]">e.g. Dictation pasted into the wrong field</div>
              <div className="grid gap-3 md:grid-cols-[1fr_130px]">
                <div className="h-24 rounded-xl border border-[#dfe3ef] bg-[#f7f8fc] p-3 text-sm text-[#7d8291]">Steps, expected behavior, actual behavior...</div>
                <div className="rounded-xl border border-[#dfe3ef] bg-[#f7f8fc] p-3 text-sm">Normal</div>
              </div>
              <div className="rounded-xl border border-dashed border-accent/60 bg-accent/10 p-4 text-sm text-[#697292]">PNG/JPG/WebP screenshot optional</div>
              <div className="rounded-xl bg-[#0c0e1a] py-3 text-center text-sm font-black text-white">Submit bug report</div>
            </div>
          </div>
        </div>
      </section>

      <section className="container-page relative z-10 border-t border-white/10 py-12">
        <div className="mb-7 flex items-center gap-3">
          <ShieldCheck className="h-5 w-5 text-accent" />
          <h2 className="font-display text-3xl tracking-tight text-white">Learning and permissions</h2>
        </div>
        <div className="grid gap-4 lg:grid-cols-3">
          {vocabulary.map((item) => (
            <article key={item.title} className="rounded-2xl border border-white/10 bg-white/[0.035] p-5">
              <div className="mb-4 inline-flex rounded-full bg-accent px-3 py-1 text-[10px] font-black uppercase tracking-[0.14em] text-[#07110d]">
                {item.label}
              </div>
              <h3 className="text-base font-semibold text-white">{item.title}</h3>
              <p className="mt-2 text-sm leading-6 text-white/58">{item.body}</p>
            </article>
          ))}
        </div>
        <div className="mt-8 overflow-hidden rounded-2xl border border-white/10">
          {permissions.map(([name, body]) => (
            <div key={name} className="grid gap-2 border-b border-white/8 bg-white/[0.025] p-4 text-sm last:border-b-0 md:grid-cols-[220px_1fr]">
              <div className="font-semibold text-white">{name}</div>
              <div className="text-white/58">{body}</div>
            </div>
          ))}
        </div>
      </section>

      <section className="container-page relative z-10 pb-20 pt-8">
        <div className="rounded-[2rem] border border-white/10 bg-white/[0.035] p-7 text-center md:p-10">
          <MapPin className="mx-auto h-6 w-6 text-accent" />
          <h2 className="mt-4 font-display text-3xl tracking-tight text-white">Need the app while reading?</h2>
          <p className="mx-auto mt-3 max-w-xl text-sm leading-6 text-white/58">
            Keep this guide open, install the latest Mac build, and test the shortcuts in Notes, Slack, Gmail, Chrome, or any editable app.
          </p>
          <a
            href={downloads.mac.latestDmg}
            className="mt-6 inline-flex h-11 items-center gap-2 rounded-full bg-white px-5 text-sm font-medium text-ink-900 transition-colors hover:bg-ink-50"
          >
            <Mic2 className="h-4 w-4" />
            Download Airnote {downloads.mac.latestVersion}
          </a>
        </div>
      </section>
    </main>
  );
}

function BookOpenIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="2">
      <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20" />
      <path d="M4 4.5A2.5 2.5 0 0 1 6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5z" />
    </svg>
  );
}
