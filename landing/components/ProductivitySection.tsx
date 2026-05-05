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
    id: 'slack',
    title: 'Clarity at the speed of voice',
    description:
      'Speak your thoughts and instantly turn them into polished updates that land in Slack. Share the full context, and keep your team aligned.',
    visual: <SlideSlack />,
    designedFor: [
      { name: 'Microsoft Teams', mark: <TeamsMark /> },
      { name: 'Slack', mark: <SlackMark /> },
    ],
  },
  {
    id: 'email',
    title: 'Formatting that fits the medium',
    description:
      'From Slack threads to emails, your voice lands formatted, clear, and ready to send.',
    visual: <SlideEmail />,
    designedFor: [
      { name: 'Gmail', mark: <GmailMark /> },
      { name: 'Apple Mail', mark: <AppleMailMark /> },
      { name: 'Outlook', mark: <OutlookMark /> },
    ],
  },
  {
    id: 'imessage',
    title: 'Your voice, in the right style',
    description:
      'Skip the awkward. Said shifts your writing style to match the context — professional for work, casual for friends.',
    visual: <SlideIMessage />,
    designedFor: [
      { name: 'iMessage', mark: <IMessageMark /> },
      { name: 'WhatsApp', mark: <WhatsAppMark /> },
      { name: 'Messenger', mark: <MessengerMark /> },
    ],
  },
];

export function ProductivitySection() {
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
      className="relative w-full bg-background text-text"
      style={{ height: `${SLIDES.length * 100}vh` }}
    >
      <div className="sticky top-0 flex h-screen flex-col overflow-hidden">
        <div className="mx-auto flex h-full min-h-0 w-full max-w-[1280px] flex-col px-token-md pt-6 pb-5 md:pt-8 md:pb-6">
          <div className="max-w-[760px] shrink-0">
            <p className="text-[12.5px] tracking-[-0.005em] text-muted">
              Productivity
            </p>
            <h2 className="mt-2 text-[32px] font-normal leading-[1.04] tracking-[-0.02em] text-text sm:text-[40px] md:text-[46px] lg:text-[50px]">
              Clearer messages.
              <br />
              Faster updates. Less effort.
            </h2>
            <p className="mt-3 max-w-[520px] text-[14px] leading-[1.5] text-muted md:text-[15px]">
              Speak your updates, and Said turns them into polished messages,
              summaries, and replies across all your favorite tools.
            </p>
          </div>

          <div className="relative mt-5 min-h-0 flex-1 overflow-hidden rounded-[20px] bg-white ring-1 ring-black/[0.06] shadow-[0_24px_60px_-30px_rgba(15,20,40,0.18)]">
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

          <div className="mt-4 grid shrink-0 grid-cols-1 items-start gap-y-3 md:grid-cols-12 md:gap-x-8">
            <div className="md:col-span-7">
              <h3 className="text-[16px] font-medium tracking-[-0.005em] text-text">
                {SLIDES[active].title}
              </h3>
              <p className="mt-1 max-w-[520px] text-[13px] leading-[1.5] text-muted">
                {SLIDES[active].description}
              </p>
            </div>
            <div className="md:col-span-5 md:flex md:justify-end md:pt-1">
              <DesignedFor logos={SLIDES[active].designedFor} />
            </div>
          </div>

          <div className="mt-4 flex shrink-0 flex-wrap items-center justify-between gap-y-3">
            <div className="flex items-center gap-2.5 text-[13.5px] text-text/75">
              <span>Hold</span>
              <kbd className="inline-flex h-7 min-w-[60px] items-center justify-center rounded-[6px] bg-black/[0.04] px-2.5 font-sans text-[12px] font-medium text-text shadow-[inset_0_0_0_1px_rgba(0,0,0,0.08)]">
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
    <div className="flex items-center gap-3 text-[13px] text-muted">
      <span>Designed for</span>
      <ul className="flex items-center gap-1.5">
        {logos.map((logo) => (
          <li
            key={logo.name}
            aria-label={logo.name}
            className="flex h-7 w-7 items-center justify-center rounded-[7px] bg-white ring-1 ring-black/[0.06] shadow-[0_1px_2px_rgba(15,20,40,0.06)]"
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
              isActive ? 'w-10 bg-text' : 'w-5 bg-text/15'
            }`}
          />
        );
      })}
    </div>
  );
}

/* ───────────────────────── slides ───────────────────────── */

function SlideSlack() {
  return (
    <div className="grid h-full grid-cols-12 gap-0 bg-[#f8f9fb] p-4 md:p-5">
      <aside className="col-span-2 hidden flex-col gap-2 rounded-l-[12px] bg-white p-3 ring-1 ring-black/[0.05] md:flex">
        <Skeleton className="h-3 w-3/4" />
        <div className="mt-3 space-y-2">
          {Array.from({ length: 5 }).map((_, i) => (
            <div key={i} className="flex items-center gap-2">
              <span className="h-3 w-3 rounded-[3px] bg-black/[0.06]" />
              <Skeleton className="h-2.5 w-2/3" />
            </div>
          ))}
        </div>
      </aside>

      <div className="col-span-12 flex flex-col rounded-[12px] bg-white p-4 ring-1 ring-black/[0.05] md:col-span-7 md:rounded-none md:rounded-r-[12px] md:rounded-l-none md:border-l md:border-l-black/[0.05]">
        <div className="flex items-center justify-between border-b border-black/[0.05] pb-3">
          <Skeleton className="h-3 w-32" />
          <div className="flex items-center gap-2">
            <Skeleton className="h-3 w-10" />
            <Skeleton className="h-3 w-10" />
          </div>
        </div>

        <div className="flex-1 overflow-hidden pt-4">
          <div className="mb-4 flex justify-center">
            <span className="rounded-full bg-black/[0.04] px-3 py-1 text-[10.5px] text-text/55 ring-1 ring-black/[0.05]">
              Today
            </span>
          </div>

          <Message
            avatarColor="#7c3aed"
            initial="r"
            name="robert"
            time="10:19 AM"
          >
            Morning team! 👋 This is the final week for the Symphony project, so
            let&rsquo;s make sure everything is on track.
            <br />
            <span className="text-[#1264a3]">@toni</span> could you share a
            quick status update with us?
          </Message>

          <div className="mt-3">
            <Message
              avatarColor="#0ea5e9"
              initial="t"
              name="toni"
              time="10:21 AM"
            >
              Morning Robert 👋 Thanks for checking in. The Symphony project is
              progressing well — most tasks are on track.
              <br />
              We&rsquo;re wrapping up final testing and documentation this
              week.
              <br />
              I&rsquo;ll flag any blockers right away, but so far things look
              good.
            </Message>
          </div>
        </div>

        <div className="mt-3 rounded-[8px] bg-white px-3 py-2.5 text-[12px] text-text/35 ring-1 ring-[#1264a3]/60 shadow-[0_0_0_3px_rgba(18,100,163,0.08)]">
          Message to #Acme...
        </div>
      </div>

      <aside className="col-span-3 ml-3 hidden flex-col gap-3 rounded-[12px] bg-white p-3 ring-1 ring-black/[0.05] md:flex">
        {Array.from({ length: 5 }).map((_, i) => (
          <div key={i} className="flex items-center gap-2">
            <span className="h-6 w-6 shrink-0 rounded-full bg-black/[0.06]" />
            <div className="flex flex-1 flex-col gap-1">
              <Skeleton className="h-2 w-3/4" />
              <Skeleton className="h-2 w-1/2" />
            </div>
          </div>
        ))}
      </aside>
    </div>
  );
}

function Message({
  avatarColor,
  initial,
  name,
  time,
  children,
}: {
  avatarColor: string;
  initial: string;
  name: string;
  time: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex gap-3">
      <div
        className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[6px] text-[12px] font-medium text-white"
        style={{ background: avatarColor }}
      >
        {initial}
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="text-[12.5px] font-semibold text-text">{name}</span>
          <span className="text-[10.5px] text-text/45">{time}</span>
        </div>
        <p className="mt-0.5 whitespace-pre-line text-[12.5px] leading-[1.55] text-text/85">
          {children}
        </p>
      </div>
    </div>
  );
}

function SlideEmail() {
  return (
    <div className="grid h-full grid-cols-12 gap-0 bg-[#f8f9fb] p-4 md:p-5">
      <aside className="col-span-4 hidden flex-col gap-2 rounded-[12px] bg-white p-2 ring-1 ring-black/[0.05] md:flex">
        {Array.from({ length: 6 }).map((_, i) => {
          const active = i === 2;
          return (
            <div
              key={i}
              className={`flex items-center gap-3 rounded-[8px] p-2.5 ${
                active ? 'bg-[#1264a3]/[0.06] ring-1 ring-[#1264a3]/25' : ''
              }`}
            >
              <span className="h-7 w-7 shrink-0 rounded-full bg-black/[0.06]" />
              <div className="flex flex-1 flex-col gap-1.5">
                <Skeleton className="h-2 w-3/4" />
                <Skeleton className="h-2 w-1/2" />
              </div>
            </div>
          );
        })}
      </aside>

      <div className="col-span-12 flex flex-col rounded-[12px] bg-white p-5 ring-1 ring-black/[0.05] md:col-span-8 md:ml-3">
        <div className="flex items-center gap-3 border-b border-black/[0.05] pb-4">
          <span className="h-9 w-9 rounded-full bg-black/[0.06]" />
          <div className="flex flex-1 flex-col gap-1.5">
            <Skeleton className="h-2.5 w-1/3" />
            <Skeleton className="h-2 w-1/2" />
          </div>
        </div>

        <div className="flex-1 overflow-auto pt-5 text-[12.5px] leading-[1.65] text-text/85">
          <p>
            <span className="text-[#1264a3]">Draft</span>{' '}
            <span className="text-text/50">to Mikal Robbins</span>
          </p>
          <p className="mt-3">Hi Mikal,</p>
          <p className="mt-3">
            Could you please review the marketing proposal I sent yesterday? I
            think we need to focus on three key areas:
          </p>
          <ol className="mt-3 space-y-1 pl-1">
            <li>1. Social media strategy needs more specific targeting.</li>
            <li>2. Budget allocation for Q3 should be reconsidered.</li>
            <li>
              3. Product launch timeline may be too aggressive for our
              resources.
            </li>
          </ol>
          <p className="mt-3">Let me know your thoughts.</p>
          <p className="mt-3">Thanks,</p>
          <p>Tom</p>
        </div>
      </div>
    </div>
  );
}

function SlideIMessage() {
  return (
    <div className="grid h-full grid-cols-12 gap-0 bg-[#f8f9fb] p-4 md:p-5">
      <aside className="col-span-4 hidden flex-col gap-2 rounded-[12px] bg-white p-2 ring-1 ring-black/[0.05] md:flex">
        {Array.from({ length: 6 }).map((_, i) => {
          const active = i === 3;
          return (
            <div
              key={i}
              className={`flex items-center gap-3 rounded-[8px] p-2.5 ${
                active ? 'bg-black/[0.04]' : ''
              }`}
            >
              <span className="h-3 w-3 shrink-0 rounded-full bg-[#1264a3]" />
              <span className="h-7 w-7 shrink-0 rounded-full bg-black/[0.06]" />
              <div className="flex flex-1 flex-col gap-1.5">
                <Skeleton className="h-2 w-3/4" />
                <Skeleton className="h-2 w-1/2" />
              </div>
            </div>
          );
        })}
      </aside>

      <div className="col-span-12 flex flex-col rounded-[12px] bg-white p-5 ring-1 ring-black/[0.05] md:col-span-8 md:ml-3">
        <div className="flex flex-1 flex-col justify-end gap-2 overflow-hidden pb-4">
          <div className="mr-auto max-w-[60%] space-y-2">
            <div className="rounded-[18px] bg-black/[0.06] px-3.5 py-2 text-[12.5px] leading-[1.5] text-text/0">
              <Skeleton className="h-2 w-40" />
              <Skeleton className="mt-1.5 h-2 w-32" />
              <Skeleton className="mt-1.5 h-2 w-44" />
            </div>
            <div className="rounded-[18px] bg-black/[0.06] px-3.5 py-2 text-[12.5px] leading-[1.5]">
              <Skeleton className="h-2 w-44" />
              <Skeleton className="mt-1.5 h-2 w-36" />
              <Skeleton className="mt-1.5 h-2 w-40" />
              <Skeleton className="mt-1.5 h-2 w-28" />
            </div>
          </div>

          <Bubble>hey gonna be a lil late to the party… prob get there around 9:30 lol</Bubble>
          <Bubble>want me to bring anything? can stop by the store otw</Bubble>
          <Bubble>lmk thx</Bubble>
        </div>

        <div className="rounded-full bg-white px-4 py-2.5 text-[12px] text-text/35 ring-1 ring-[#1264a3]/55 shadow-[0_0_0_3px_rgba(18,100,163,0.08)]">
          &nbsp;
        </div>
      </div>
    </div>
  );
}

function Bubble({ children }: { children: React.ReactNode }) {
  return (
    <div className="ml-auto max-w-[70%] rounded-[18px] bg-[#1f86ff] px-3.5 py-2 text-[12.5px] leading-[1.4] text-white shadow-[0_1px_2px_rgba(31,134,255,0.25)]">
      {children}
    </div>
  );
}

function Skeleton({ className = '' }: { className?: string }) {
  return <span className={`block rounded-[3px] bg-black/[0.06] ${className}`} />;
}

/* ───────────────── tiny brand glyphs ───────────────── */

function SlackMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <rect x="3" y="10" width="6" height="3" rx="1.5" fill="#36c5f0" />
      <rect x="11" y="3" width="3" height="6" rx="1.5" fill="#2eb67d" />
      <rect x="15" y="11" width="6" height="3" rx="1.5" fill="#ecb22e" />
      <rect x="10" y="15" width="3" height="6" rx="1.5" fill="#e01e5a" />
    </svg>
  );
}
function TeamsMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <rect x="3" y="6" width="12" height="12" rx="2" fill="#5059c9" />
      <text
        x="9"
        y="15"
        textAnchor="middle"
        fontSize="9"
        fontWeight="700"
        fill="#fff"
        fontFamily="sans-serif"
      >
        T
      </text>
      <circle cx="18" cy="9" r="3" fill="#7b83eb" />
    </svg>
  );
}
function GmailMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path d="M3 7l9 6 9-6v11H3z" fill="#ea4335" />
      <path d="M3 7l9 6 9-6" stroke="#fff" strokeWidth="0.8" fill="none" />
    </svg>
  );
}
function AppleMailMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <rect x="3" y="6" width="18" height="12" rx="2" fill="#1f86ff" />
      <path d="M3.5 7l8.5 6 8.5-6" stroke="#fff" strokeWidth="1.2" fill="none" />
    </svg>
  );
}
function OutlookMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <rect x="3" y="6" width="11" height="12" rx="1.5" fill="#0078d4" />
      <text
        x="8.5"
        y="15"
        textAnchor="middle"
        fontSize="8"
        fontWeight="700"
        fill="#fff"
        fontFamily="sans-serif"
      >
        O
      </text>
      <rect x="15" y="8" width="6" height="8" rx="1" fill="#50a0e0" />
    </svg>
  );
}
function IMessageMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path
        d="M12 3c5 0 9 3.4 9 7.7 0 4.3-4 7.7-9 7.7-1 0-1.9-.1-2.7-.3L5 19.5l1.2-3.1C4.8 15 4 13 4 10.7 4 6.4 7.5 3 12 3z"
        fill="#2eb872"
      />
    </svg>
  );
}
function WhatsAppMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path
        d="M12 3c5 0 9 4 9 9 0 4.9-4 9-9 9-1.5 0-3-.4-4.3-1.1L3 21l1.2-4.5C3.4 15.2 3 13.6 3 12c0-5 4-9 9-9z"
        fill="#25d366"
      />
      <path
        d="M9 9c0 3 3 6 6 6l1.2-1.2-1.8-1-1 .8c-1-.4-1.8-1.2-2.2-2.2l.8-1-1-1.8L9 9z"
        fill="#fff"
      />
    </svg>
  );
}
function MessengerMark() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none">
      <path
        d="M12 3c5 0 9 3.7 9 8.3 0 2.6-1.3 4.9-3.4 6.5v3.2l-3.1-1.7c-.8.2-1.6.3-2.5.3-5 0-9-3.7-9-8.3S7 3 12 3z"
        fill="#0084ff"
      />
      <path
        d="M5.5 13.5l3.8-4 2 2 3.7-2.5-3.7 4.5-2-2-3.8 2z"
        fill="#fff"
      />
    </svg>
  );
}
