"use client";

import { footer } from "@/lib/content";

function Wordmark() {
  return (
    <div className="flex items-center gap-2">
      <span
        aria-hidden
        className="inline-block h-6 w-6 rounded-md bg-accent shadow-[inset_0_-2px_0_rgba(0,0,0,0.2)]"
      />
      <span className="font-display text-lg tracking-tight text-ink-50">Airnote</span>
    </div>
  );
}

export function Footer() {
  return (
    <footer id="download" className="border-t border-ink-50/5 mt-20">
      <div className="container-page py-20">
        <div className="grid lg:grid-cols-12 gap-12">
          <div className="lg:col-span-4">
            <Wordmark />
            <p className="mt-4 text-sm text-ink-200 max-w-xs leading-relaxed">
              {footer.tagline}
            </p>
            <form
              onSubmit={(e) => e.preventDefault()}
              className="mt-6 flex items-center gap-2 max-w-sm"
            >
              <label htmlFor="email-signup" className="sr-only">
                Email address
              </label>
              <input
                id="email-signup"
                type="email"
                autoComplete="email"
                placeholder="you@email.com"
                className="flex-1 h-10 rounded-xl bg-ink-800 hairline px-3 text-sm text-ink-50 placeholder:text-ink-300 focus:outline-none focus:border-ink-50/20"
              />
              <button
                type="submit"
                className="h-10 px-4 rounded-xl bg-ink-700 hairline text-sm text-ink-50 hover:bg-ink-600 transition-colors"
              >
                Subscribe
              </button>
            </form>
          </div>

          <div className="lg:col-span-8 grid grid-cols-2 md:grid-cols-4 gap-8">
            {footer.columns.map((col) => (
              <div key={col.heading}>
                <h3 className="text-xs uppercase tracking-[0.15em] text-ink-300 mb-4">
                  {col.heading}
                </h3>
                <ul className="space-y-2.5">
                  {col.links.map((l) => (
                    <li key={l.label}>
                      <a
                        href={l.href}
                        className="text-sm text-ink-100 hover:text-white transition-colors"
                      >
                        {l.label}
                      </a>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        </div>

        <div className="mt-16 pt-8 border-t border-ink-50/5 flex flex-col-reverse md:flex-row md:items-center md:justify-between gap-4">
          <p className="text-xs text-ink-300">{footer.copyright}</p>
          <ul className="flex items-center gap-5">
            {footer.social.map((s) => (
              <li key={s.label}>
                <a
                  href={s.href}
                  className="text-xs text-ink-200 hover:text-white transition-colors"
                >
                  {s.label}
                </a>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </footer>
  );
}
