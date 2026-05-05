import { TrustLogos } from './TrustLogos';

export function SpeakSection() {
  return (
    <section className="relative w-full">
      <div className="mx-auto w-full max-w-[1280px] px-token-md py-token-xl text-center">
        <h2 className="mx-auto text-[44px] font-normal leading-[1.06] tracking-[-0.02em] text-text sm:text-[52px] md:text-[60px] lg:text-[64px]">
          Speak, and it&rsquo;s done
        </h2>

        <p className="mx-auto mt-6 max-w-[560px] text-[18px] leading-[1.55] text-text/80 md:text-[20px]">
          Speak naturally, and let Said&rsquo;s AI refine your words as you talk.
          <br />
          Fast, accurate, and works with every app.
        </p>

        <div className="mt-10 flex justify-center">
          <a
            href="/download"
            className="inline-flex items-center justify-center rounded-full bg-[#eef0f3] px-7 py-3 text-[15px] font-medium text-text transition hover:bg-[#e4e7ec] active:bg-[#dde0e6]"
          >
            Start transcribing
          </a>
        </div>

        <div className="mt-16 md:mt-20">
          <MonitorMockup />
        </div>

        <div className="mt-12 flex flex-wrap items-center justify-between gap-y-6 text-left">
          <div className="flex items-center gap-3 text-[15px] text-text/85">
            <span>Hold</span>
            <kbd className="inline-flex h-9 min-w-[72px] items-center justify-center rounded-[8px] bg-white px-3.5 font-sans text-[13px] font-medium text-text shadow-[0_1px_0_rgba(0,0,0,0.04),0_4px_10px_rgba(41,44,61,0.08)] ring-1 ring-black/[0.06]">
              Space
            </kbd>
            <span>and try yourself</span>
          </div>

          <TrustLogos />
        </div>
      </div>
    </section>
  );
}

function MonitorMockup() {
  return (
    <div className="mx-auto w-full max-w-[980px]">
      {/* Bezel */}
      <div className="relative overflow-hidden rounded-[24px] border border-black/10 bg-white p-3 shadow-[0_24px_60px_rgba(41,44,61,0.10)]">
        {/* Wallpaper — fixed 16:10 aspect for an iMac-like screen */}
        <div className="relative aspect-[16/10] overflow-hidden rounded-[14px]">
          <div
            aria-hidden
            className="absolute inset-0"
            style={{
              backgroundImage:
                'linear-gradient(120deg, #cfe0f3 0%, #d6e4f5 30%, #e7eef8 55%, #cfdef0 80%, #b9cde3 100%)',
            }}
          />
          <div
            aria-hidden
            className="absolute inset-0 opacity-70"
            style={{
              backgroundImage:
                "url(\"data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 800 500'><defs><filter id='w'><feTurbulence type='fractalNoise' baseFrequency='0.008' numOctaves='2' seed='4'/><feDisplacementMap in='SourceGraphic' scale='40'/></filter></defs><g filter='url(%23w)'><rect width='100%25' height='100%25' fill='url(%23g)'/></g><defs><linearGradient id='g' x1='0' y1='0' x2='1' y2='1'><stop offset='0' stop-color='%23bcd1ea'/><stop offset='1' stop-color='%23e6eef8'/></linearGradient></defs></svg>\")",
              backgroundSize: 'cover',
            }}
          />

          {/* App window — sits inset from the wallpaper edges */}
          <div className="absolute inset-x-[6%] top-[8%] bottom-[8%] overflow-hidden rounded-[14px] bg-white shadow-[0_18px_40px_rgba(41,44,61,0.10)]">
            <AppToolbar />
            <Transcript />
          </div>

          {/* Mac-style indicator */}
          <div className="absolute inset-x-0 bottom-3 flex justify-center">
            <span className="h-[5px] w-[100px] rounded-full bg-black/40" aria-hidden />
          </div>
        </div>
      </div>

      {/* Stand */}
      <div className="mx-auto h-5 w-[160px] rounded-b-[12px] bg-gradient-to-b from-[#d4d8de] to-[#bfc4cc]" />
      <div className="mx-auto h-3 w-[260px] rounded-b-[20px] bg-gradient-to-b from-[#bfc4cc] to-[#a9aeb6] shadow-[0_8px_18px_rgba(41,44,61,0.08)]" />
    </div>
  );
}

function AppToolbar() {
  return (
    <div className="flex items-center justify-between px-6 pt-5">
      <button
        aria-label="Menu"
        className="flex h-9 w-9 items-center justify-center rounded-full ring-1 ring-black/10"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none">
          <path d="M4 7h16M4 12h16M4 17h16" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
        </svg>
      </button>

      <div className="rounded-full ring-1 ring-black/10 px-5 py-1.5 font-mono text-[13px] text-text/80">
        8,032 characters &nbsp;&nbsp;&nbsp; 1,401 words
      </div>

      <button
        aria-label="Share"
        className="flex h-9 w-9 items-center justify-center rounded-full ring-1 ring-black/10"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none">
          <path
            d="M12 4v12M12 4l-4 4M12 4l4 4M5 14v4a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-4"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>
    </div>
  );
}

function Transcript() {
  return (
    <div className="px-10 pb-12 pt-8 text-left font-mono text-[13.5px] leading-[1.85] text-text">
      <h3 className="mb-4 text-[15px] font-semibold text-text">Chapter 2</h3>
      <p>
        The rain had finally eased, leaving the streets washed in silver light.
        Elena pulled her coat tighter, not against the cold, but against the
        strange feeling that someone had been following her since she left the
        café.
      </p>
      <p className="mt-5">
        She glanced over her shoulder—nothing. Just the quiet rhythm of the city,
        the drip of water from iron balconies, the hum of distant traffic. Still,
        the unease clung to her.
      </p>
      <p className="mt-5">La, la,</p>
    </div>
  );
}
