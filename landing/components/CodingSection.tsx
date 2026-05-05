'use client';

import { useEffect, useRef, useState } from 'react';

type Slide = {
  id: string;
  title: string;
  description: string;
  visual: React.ReactNode;
  designedFor: { name: string; mark: React.ReactNode }[];
};

const SLIDES: Slide[] = [
  {
    id: 'tech',
    title: 'Prompting with technical accuracy',
    description:
      'We benchmarked models on how accurately they capture developer language. Avalon consistently nails terms like GPT-4o, kubectl, and PyTorch.',
    visual: <SlideTechAccuracy />,
    designedFor: [
      { name: 'Cursor', mark: <CursorMark /> },
      { name: 'Windsurf', mark: <WindsurfMark /> },
      { name: 'VS Code', mark: <VSCodeMark /> },
    ],
  },
  {
    id: 'syntax',
    title: 'Syntax Highlighting',
    description:
      'Avalon highlights code the way developers expect — with precise syntax recognition across languages and frameworks.',
    visual: <SlideSyntax />,
    designedFor: [
      { name: 'Claude', mark: <ClaudeMark /> },
      { name: 'Lovable', mark: <LovableMark /> },
      { name: 'Bolt', mark: <BoltMark /> },
    ],
  },
  {
    id: 'speed',
    title: 'Prompt at the speed of thought',
    description:
      'Avalon turns natural, rambling speech into precise prompts — letting you build faster without stopping to edit your thoughts.',
    visual: <SlideSpeed />,
    designedFor: [
      { name: 'OpenAI', mark: <OpenAIMark /> },
      { name: 'Linear', mark: <LinearMark /> },
      { name: 'Replit', mark: <ReplitMark /> },
    ],
  },
];

export function CodingSection() {
  const sectionRef = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState(0);

  useEffect(() => {
    const onScroll = () => {
      const el = sectionRef.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      const total = el.offsetHeight - window.innerHeight;
      const scrolled = Math.max(0, -rect.top);
      const progress = total > 0 ? Math.min(1, scrolled / total) : 0;
      // Each slide gets an equal share of the scroll range; nudge by 0.0001
      // so the last slide locks in just before the section ends.
      const idx = Math.min(
        SLIDES.length - 1,
        Math.floor(progress * SLIDES.length - 0.0001 + 0.0001),
      );
      const clamped = Math.max(
        0,
        Math.min(SLIDES.length - 1, Math.floor(progress * SLIDES.length)),
      );
      setActive(clamped);
    };

    onScroll();
    window.addEventListener('scroll', onScroll, { passive: true });
    window.addEventListener('resize', onScroll);
    return () => {
      window.removeEventListener('scroll', onScroll);
      window.removeEventListener('resize', onScroll);
    };
  }, []);

  return (
    <section
      ref={sectionRef}
      className="relative w-full bg-black text-white"
      style={{ height: `${SLIDES.length * 100}vh` }}
    >
      {/* Sticky stage — pinned for the duration of the section */}
      <div className="sticky top-0 flex h-screen flex-col overflow-hidden">
        <div className="mx-auto flex h-full min-h-0 w-full max-w-[1280px] flex-col px-token-md pt-6 pb-5 md:pt-8 md:pb-6">
          {/* Header — compact so the stage gets the breathing room */}
          <div className="max-w-[760px] shrink-0">
            <p className="text-[12.5px] tracking-[-0.005em] text-white/40">
              Coding &amp; Prompting
            </p>
            <h2 className="mt-2 text-[32px] font-normal leading-[1.04] tracking-[-0.02em] sm:text-[40px] md:text-[46px] lg:text-[50px]">
              Prompt faster with your voice
            </h2>
            <p className="mt-3 max-w-[520px] text-[14px] leading-[1.5] text-white/55 md:text-[15px]">
              Speak your ideas into existence with ease. Said understands
              syntax, libraries, and frameworks as you speak.
            </p>
          </div>

          {/* Slide stage — flex-1 so it fills all remaining height */}
          <div className="relative mt-5 min-h-0 flex-1 overflow-hidden rounded-[20px] border border-white/[0.08] bg-[#0c0c0e]">
            {SLIDES.map((s, i) => (
              <div
                key={s.id}
                aria-hidden={i !== active}
                className={`absolute inset-0 transition-opacity duration-500 ease-out ${
                  i === active ? 'opacity-100' : 'pointer-events-none opacity-0'
                }`}
              >
                {s.visual}
              </div>
            ))}
          </div>

          {/* Caption */}
          <div className="mt-4 grid shrink-0 grid-cols-1 items-start gap-y-3 md:grid-cols-12 md:gap-x-8">
            <div className="md:col-span-7">
              <h3 className="text-[16px] font-medium tracking-[-0.005em]">
                {SLIDES[active].title}
              </h3>
              <p className="mt-1 max-w-[520px] text-[13px] leading-[1.5] text-white/50">
                {SLIDES[active].description}
              </p>
            </div>
            <div className="md:col-span-5 md:flex md:justify-end md:pt-1">
              <DesignedFor logos={SLIDES[active].designedFor} />
            </div>
          </div>

          {/* Pagination + Hold Space row */}
          <div className="mt-4 flex shrink-0 flex-wrap items-center justify-between gap-y-3">
            <div className="flex items-center gap-2.5 text-[13.5px] text-white/75">
              <span>Hold</span>
              <kbd className="inline-flex h-7 min-w-[60px] items-center justify-center rounded-[6px] bg-white/10 px-2.5 font-sans text-[12px] font-medium text-white shadow-[inset_0_0_0_1px_rgba(255,255,255,0.10)]">
                Space
              </kbd>
              <span>and try yourself</span>
            </div>

            <Pagination count={SLIDES.length} active={active} />
          </div>
        </div>
      </div>
    </section>
  );
}

function DesignedFor({ logos }: { logos: Slide['designedFor'] }) {
  return (
    <div className="flex items-center gap-3 text-[13px] text-white/40">
      <span>Designed for</span>
      <ul className="flex items-center gap-1.5">
        {logos.map((logo) => (
          <li
            key={logo.name}
            aria-label={logo.name}
            className="flex h-7 w-7 items-center justify-center rounded-[7px] bg-white/[0.06] ring-1 ring-white/[0.08]"
          >
            {logo.mark}
          </li>
        ))}
      </ul>
    </div>
  );
}

function Pagination({ count, active }: { count: number; active: number }) {
  return (
    <div className="flex items-center gap-1.5" aria-hidden>
      {Array.from({ length: count }).map((_, i) => {
        const isActive = i === active;
        return (
          <span
            key={i}
            className={`h-[3px] rounded-full transition-all duration-500 ${
              isActive ? 'w-10 bg-white' : 'w-5 bg-white/20'
            }`}
          />
        );
      })}
    </div>
  );
}

/* ───────────────────────── slides ───────────────────────── */

function SlideTechAccuracy() {
  return (
    <div className="grid h-full grid-cols-12 gap-3 p-4 md:p-5">
      <div className="relative col-span-7 overflow-hidden rounded-[10px] bg-[#0a0a0c] ring-1 ring-white/[0.06]">
        <CodeEditor />
      </div>
      <div className="col-span-5 flex flex-col justify-center gap-3">
        <div className="rounded-[12px] bg-[#16171b] p-4 ring-1 ring-white/[0.08] shadow-[0_24px_60px_rgba(0,0,0,0.4)]">
          <p className="text-[13px] leading-[1.55] text-white/85">
            Can you modify the ToDoListController to use Zustand instead of
            React
          </p>
          <div className="mt-4 flex items-center justify-between">
            <span className="inline-flex items-center gap-1.5 rounded-full bg-white/[0.08] px-2.5 py-1 text-[11px] text-white/60">
              <ChatGPTDot /> GPT-4o
            </span>
            <span className="inline-flex items-center gap-1.5 rounded-full bg-white/[0.08] px-2.5 py-1 text-[11px] text-white/70">
              Send <span className="text-white/50">↵</span>
            </span>
          </div>
        </div>
        <ClaudeCodeTerminal compact />
      </div>
    </div>
  );
}

function SlideSyntax() {
  return (
    <div className="flex h-full items-center justify-center p-5 md:p-8">
      <ClaudeCodeTerminal />
    </div>
  );
}

function SlideSpeed() {
  return (
    <div className="grid h-full grid-cols-12 gap-3 p-4 md:p-5">
      <div className="col-span-7 overflow-hidden rounded-[10px] bg-[#0a0a0c] p-5 ring-1 ring-white/[0.06]">
        <ChatTranscript />
      </div>
      <div className="col-span-5 flex flex-col gap-3">
        <div className="grid h-1/3 grid-cols-3 gap-2">
          {Array.from({ length: 3 }).map((_, i) => (
            <div
              key={i}
              className="rounded-[8px] bg-white/[0.04] ring-1 ring-white/[0.06]"
            />
          ))}
        </div>
        <div className="h-2.5 rounded-full bg-white/[0.06]" />
        <div className="h-2.5 w-3/4 rounded-full bg-white/[0.06]" />
        <div className="grow rounded-[10px] bg-white/[0.04] ring-1 ring-white/[0.06]" />
      </div>
    </div>
  );
}

/* ───────────────── shared visual primitives ───────────────── */

function CodeEditor() {
  return (
    <div className="h-full overflow-hidden p-4 font-mono text-[11px] leading-[1.7] text-white/70">
      <CodeLine n={1}>
        <Tok kw>import</Tok>{' '}
        <Tok punct>{'{'}</Tok> Body, Button, Column, Container, Head, Heading, Hr,
        Html, Img, Link, Preview, Row, <Tok punct>{'}'}</Tok>{' '}
        <Tok kw>from</Tok>{' '}
        <Tok str>&apos;@react-email/components&apos;</Tok>;
      </CodeLine>
      <CodeLine n={2}>
        <Tok kw>import</Tok> * <Tok kw>as</Tok>{' '}
        <Tok cls>React</Tok> <Tok kw>from</Tok>{' '}
        <Tok str>&apos;react&apos;</Tok>;
      </CodeLine>
      <CodeLine n={3}> </CodeLine>
      <CodeLine n={4}>
        <Tok kw>const</Tok> <Tok cls>WelcomeEmail</Tok> ={' '}
        <Tok punct>(</Tok>
        <Tok punct>{'{'}</Tok>
      </CodeLine>
      <CodeLine n={5}>
        {'  '}username = <Tok str>&apos;Steve&apos;</Tok>,
      </CodeLine>
      <CodeLine n={6}>
        {'  '}company = <Tok str>&apos;ACME&apos;</Tok>,
      </CodeLine>
      <CodeLine n={7}>
        <Tok punct>{'}'}</Tok>: <Tok cls>WelcomeEmailProps</Tok>
        <Tok punct>)</Tok> ={'>'} <Tok punct>{'{'}</Tok>
      </CodeLine>
      <CodeLine n={8}>
        {'  '}
        <Tok kw>const</Tok> previewText ={' '}
        <Tok str>{'`Welcome to ${company}, ${username}!`'}</Tok>;
      </CodeLine>
      <CodeLine n={9}> </CodeLine>
      <CodeLine n={10}>
        {'  '}
        <Tok kw>return</Tok> <Tok punct>(</Tok>
      </CodeLine>
      <CodeLine n={11}>
        {'    '}
        <Tok tag>{'<Html>'}</Tok>
      </CodeLine>
      <CodeLine n={12}>
        {'      '}
        <Tok tag>{'<Head />'}</Tok>
      </CodeLine>
      <CodeLine n={13}>
        {'      '}
        <Tok tag>{'<Preview>'}</Tok>
        {'{previewText}'}
        <Tok tag>{'</Preview>'}</Tok>
      </CodeLine>
      <CodeLine n={14}>
        {'      '}
        <Tok tag>{'<Tailwind>'}</Tok>
      </CodeLine>
      <CodeLine n={15}>
        {'        '}
        <Tok tag>{'<Body'}</Tok> className=
        <Tok str>&quot;bg-white my-auto mx-auto font-sans&quot;</Tok>
        <Tok tag>{'>'}</Tok>
      </CodeLine>
    </div>
  );
}

function CodeLine({ n, children }: { n: number; children: React.ReactNode }) {
  return (
    <div className="flex">
      <span className="mr-3 w-5 select-none text-right text-white/25">{n}</span>
      <span className="whitespace-pre-wrap">{children}</span>
    </div>
  );
}

function Tok({
  children,
  kw,
  str,
  cls,
  tag,
  punct,
}: {
  children: React.ReactNode;
  kw?: boolean;
  str?: boolean;
  cls?: boolean;
  tag?: boolean;
  punct?: boolean;
}) {
  let cls_ = 'text-white/80';
  if (kw) cls_ = 'text-[#c084fc]';
  if (str) cls_ = 'text-[#86efac]';
  if (cls) cls_ = 'text-[#fbbf24]';
  if (tag) cls_ = 'text-[#7dd3fc]';
  if (punct) cls_ = 'text-white/50';
  return <span className={cls_}>{children}</span>;
}

function ClaudeCodeTerminal({ compact }: { compact?: boolean }) {
  return (
    <div
      className={`rounded-[10px] bg-[#0a0a0c] p-4 ring-1 ring-white/[0.06] ${
        compact ? 'flex-1' : 'w-full max-w-[760px]'
      }`}
    >
      {!compact && (
        <pre
          aria-hidden
          className="mb-4 select-none whitespace-pre text-center font-mono text-[9px] leading-[1] text-white/35"
        >
          {`██████ ██╗      █████╗ ██╗   ██╗██████  ███████\n██╔═══╝██║     ██╔══██╗██║   ██║██╔══██ ██╔════╝\n██║    ██║     ███████║██║   ██║██║  ██ █████╗\n██║    ██║     ██╔══██║██║   ██║██║  ██ ██╔══╝\n╚█████╗████████╗██║  ██║╚██████╔╝██████╔ ███████\n ╚════╝╚═══════╝╚═╝  ╚═╝ ╚═════╝ ╚═════  ╚══════\n\n      ██████  ██████  ██████  ███████\n     ██╔════╝██╔═══██╗██╔══██╗██╔════╝\n     ██║     ██║   ██║██║  ██║█████╗\n     ██║     ██║   ██║██║  ██║██╔══╝\n     ╚██████╗╚██████╔╝██████╔╝███████\n      ╚═════╝ ╚═════╝ ╚═════╝ ╚══════`}
        </pre>
      )}
      <div className="rounded-[6px] bg-white/[0.03] p-2.5 font-mono text-[10.5px] leading-[1.6] text-[#fca5a5] ring-1 ring-[#fca5a5]/20">
        <span className="text-white/40">+</span>{' '}
        <span className="text-white/85">Welcome to</span>{' '}
        <span className="text-white">Claude Code</span>{' '}
        <span className="text-white/85">research preview!</span>
      </div>
      {!compact && (
        <>
          <div className="mt-2 rounded-[6px] bg-white/[0.03] p-2.5 font-mono text-[10.5px] leading-[1.6] text-white/80 ring-1 ring-white/[0.07]">
            <span className="text-white/40">{'>'}</span> I can&rsquo;t run npm_run dev on this project. Can you look into it and tell me what&rsquo;s wrong?
          </div>
          <div className="mt-2 rounded-[6px] bg-white/[0.03] p-2.5 font-mono text-[10.5px] text-white/80 ring-1 ring-white/[0.07]">
            <span className="text-white/40">{'>'}</span>
          </div>
        </>
      )}
      {compact && (
        <div className="mt-2 rounded-[6px] bg-white/[0.03] p-2.5 font-mono text-[10px] text-white/70 ring-1 ring-white/[0.07]">
          <span className="text-white/40">{'>'}</span> Ask our agent...
        </div>
      )}
    </div>
  );
}

function ChatTranscript() {
  return (
    <div className="space-y-3 text-[11.5px] leading-[1.6] text-white/80">
      <p>
        Let me design a beautiful, modern fitness application inspired by clean,
        health-focused design principles. I&rsquo;ll create an intuitive,
        user-centered interface that motivates users and makes tracking workouts
        and progress effortless.
      </p>
      <p className="text-white/60">Design inspiration:</p>
      <ul className="space-y-0.5 pl-4 text-white/70 [&>li]:relative [&>li]:before:absolute [&>li]:before:-left-3 [&>li]:before:text-white/40 [&>li]:before:content-['•']">
        <li>Bold, energetic typography to inspire action</li>
        <li>Dynamic animations for workout transitions</li>
        <li>Card-based layouts for workouts</li>
        <li>Vibrant gradients and glass-morphism effects</li>
      </ul>
      <div className="ml-auto w-fit max-w-[260px] rounded-[8px] bg-white/[0.06] px-3 py-2 text-[11px] text-white/80 ring-1 ring-white/[0.08]">
        I want the hero section to have a big headline in bold, then a
        subheadline that&rsquo;s a little lighter, and maybe a button that says
        &lsquo;Get Started.&rsquo;
      </div>
    </div>
  );
}

function ChatGPTDot() {
  return <span className="inline-block h-2.5 w-2.5 rounded-full bg-[#10a37f]" />;
}

/* ───────────────── tiny brand glyphs ───────────────── */

function CursorMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path d="M4 4l16 8-7 1.5L11 21 4 4z" fill="#fff" />
    </svg>
  );
}
function WindsurfMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path
        d="M3 14c4-3 7-3 11 0s7 3 7 3M3 8c4-3 7-3 11 0s7 3 7 3"
        stroke="#fff"
        strokeWidth="1.6"
        strokeLinecap="round"
        fill="none"
      />
    </svg>
  );
}
function VSCodeMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path d="M17 3 7 12l10 9V3z" fill="#3b82f6" />
    </svg>
  );
}
function ClaudeMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path
        d="M12 2c2 4 4 6 8 8-4 2-6 4-8 8-2-4-4-6-8-8 4-2 6-4 8-8z"
        fill="#f97316"
      />
    </svg>
  );
}
function LovableMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <circle cx="12" cy="12" r="9" fill="#f472b6" />
    </svg>
  );
}
function BoltMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path d="M13 2 4 14h6l-1 8 9-12h-6l1-8z" fill="#fbbf24" />
    </svg>
  );
}
function OpenAIMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <circle cx="12" cy="12" r="9" stroke="#fff" strokeWidth="1.6" fill="none" />
      <path d="M8 12h8M12 8v8" stroke="#fff" strokeWidth="1.6" />
    </svg>
  );
}
function LinearMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <rect x="4" y="4" width="16" height="16" rx="3" fill="#a78bfa" />
    </svg>
  );
}
function ReplitMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path d="M5 4h7v7H5zM12 11h7v7h-7zM5 11h7v9H5z" fill="#22d3ee" />
    </svg>
  );
}
