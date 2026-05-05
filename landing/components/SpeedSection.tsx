export function SpeedSection() {
  return (
    <section className="relative w-full">
      <div className="mx-auto w-full max-w-[1280px] px-token-md pt-token-xl text-center">
        <h2 className="mx-auto max-w-[20ch] text-[44px] font-normal leading-[1.06] tracking-[-0.02em] text-text sm:text-[52px] md:text-[60px] lg:text-[64px]">
          5x faster than typing and
          <br />
          twice as accurate
        </h2>

        <p className="mx-auto mt-6 max-w-[560px] text-[18px] leading-[1.55] text-text/80 md:text-[20px]">
          Forget the keyboard. Write five times faster with your voice
          <br />
          and save hours every week with flawless accuracy.
        </p>

        <div className="mt-10 flex justify-center">
          <a
            href="/download"
            className="inline-flex items-center justify-center rounded-full bg-[#eef0f3] px-7 py-3 text-[15px] font-medium text-text transition hover:bg-[#e4e7ec] active:bg-[#dde0e6]"
          >
            Write faster with Said
          </a>
        </div>

        <Comparison />
      </div>
    </section>
  );
}

function Comparison() {
  return (
    <div className="mt-16 md:mt-20">
      <div className="relative grid grid-cols-1 gap-y-10 pb-16 text-left md:grid-cols-2 md:gap-x-12 md:pb-20">
        {/* Vertical divider — full height of the comparison block,
            spans down to where the horizontal rule sits, forming a clean cross. */}
        <div
          aria-hidden
          className="pointer-events-none absolute left-1/2 top-0 bottom-0 hidden w-px -translate-x-1/2 bg-black/10 md:block"
        />

        <ComparisonColumn
          icon={<WaveformIcon />}
          label="Using Said 3.1"
          wpm="230"
          side="left"
        >
          Make a new React component called TaskDashboard.
          Add a{' '}
          <span className="rounded-[4px] bg-accent/15 px-1 [color:#3a8ad9]">
            useState
          </span>{' '}
          hook for selectedTaskId initialized to null, and another for
          isSidebarOpen set to true.
          <span
            aria-hidden
            className="pulse-dot ml-2 inline-block h-3 w-3 translate-y-[1px] rounded-full bg-accent align-middle shadow-glow"
          />
        </ComparisonColumn>

        <ComparisonColumn
          icon={<KeyboardIcon />}
          label="Using Keyboard"
          wpm="40"
          side="right"
        >
          Make a new React component called TaskDashboard. Add a useState
          hook for selectedTaskId initialized to null, and another for
          isSidebarOpen set to true.
          <span
            aria-hidden
            className="ml-0.5 inline-block h-[1.1em] w-[2px] translate-y-[3px] animate-pulse bg-accent align-middle"
          />
        </ComparisonColumn>
      </div>

      {/* Horizontal divider — meets the vertical line at its midpoint. */}
      <div className="border-t border-black/10" />
    </div>
  );
}

type ColProps = {
  icon: React.ReactNode;
  label: string;
  wpm: string;
  side: 'left' | 'right';
  children: React.ReactNode;
};

function ComparisonColumn({ icon, label, wpm, side, children }: ColProps) {
  return (
    <div className={side === 'right' ? 'md:pl-12' : 'md:pr-12'}>
      <div className="flex items-center justify-between border-b border-black/10 pb-4 text-[15px]">
        <div className="flex items-center gap-2 text-muted">
          <span className="text-accent">{icon}</span>
          <span>{label}</span>
        </div>
        <div className="flex items-baseline gap-1.5 text-muted">
          <span className="font-mono text-[18px] tabular-nums text-text/85">{wpm}</span>
          <span className="text-[13px] tracking-wide">WPM</span>
        </div>
      </div>

      <p className="mt-6 text-[22px] leading-[1.55] text-text md:text-[24px]">
        {children}
      </p>
    </div>
  );
}

function WaveformIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden>
      <g stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
        <path d="M3 12v0" />
        <path d="M6 9v6" />
        <path d="M9 6v12" />
        <path d="M12 9v6" />
        <path d="M15 4v16" />
        <path d="M18 8v8" />
        <path d="M21 11v2" />
      </g>
    </svg>
  );
}

function KeyboardIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden>
      <rect x="2.5" y="6" width="19" height="12" rx="2.5" stroke="currentColor" strokeWidth="1.6" />
      <g fill="currentColor">
        <circle cx="6" cy="10.5" r="0.9" />
        <circle cx="9.5" cy="10.5" r="0.9" />
        <circle cx="13" cy="10.5" r="0.9" />
        <circle cx="16.5" cy="10.5" r="0.9" />
        <circle cx="6" cy="13.5" r="0.9" />
        <circle cx="18" cy="13.5" r="0.9" />
      </g>
      <path d="M9 16.5h6" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}
