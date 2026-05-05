export function ResultsSection() {
  return (
    <section className="relative w-full bg-black text-white">
      <div className="mx-auto w-full max-w-[1280px] px-token-md pb-token-xl pt-20">
        <div className="max-w-[560px]">
          <h2 className="text-[32px] font-normal leading-[1.05] tracking-[-0.02em] sm:text-[36px] md:text-[40px] lg:text-[44px]">
            Results you notice
            <br />
            immediately
          </h2>
          <p className="mt-4 max-w-[420px] text-[15px] leading-[1.55] text-white/55">
            Said helps developers ship faster, stay focused,
            <br />
            and spend less time on repetitive typing.
          </p>
        </div>

        <div className="mt-14 grid grid-cols-1 gap-x-16 gap-y-8 md:mt-16 md:grid-cols-2">
          <Stat value="6h 23m" label="Saved coding weekly" />
          <Stat value="230wpm" label="Write 5 times faster" />
        </div>
      </div>
    </section>
  );
}

function Stat({ value, label }: { value: string; label: string }) {
  return (
    <div className="flex flex-col">
      <div className="flex items-end justify-between gap-6 pb-4">
        <span className="text-[44px] font-normal leading-[1] tracking-[-0.03em] tabular-nums text-white sm:text-[56px] md:text-[64px] lg:text-[72px]">
          {value}
        </span>
        <span className="pb-2 text-[13px] text-white/55">{label}</span>
      </div>
      <div className="h-px w-full bg-white/15" />
    </div>
  );
}
