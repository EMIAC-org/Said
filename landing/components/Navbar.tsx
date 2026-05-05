import Link from 'next/link';

const NAV_LINKS = [
  { label: 'Pricing', href: '/pricing' },
  { label: 'User Guide', href: '/guide' },
  { label: 'Blog', href: '/blog' },
  { label: 'Changelog', href: '/changelog' },
  { label: 'API', href: '/api' },
  { label: 'Download iOS', href: '/ios' },
] as const;

export function Navbar() {
  return (
    <header className="relative z-10 mx-auto flex w-full max-w-[1280px] items-center justify-between px-token-md pt-8">
      <Link
        href="/"
        aria-label="Said — home"
        className="text-[22px] font-bold tracking-[-0.04em] text-ink"
      >
        AQUA
      </Link>

      <nav
        aria-label="Primary"
        className="flex items-center gap-1 rounded-full bg-[#eef0f3] p-1.5"
      >
        {NAV_LINKS.map((link) => (
          <Link
            key={link.href}
            href={link.href}
            className="rounded-full px-4 py-2 text-[15px] font-medium text-muted transition hover:text-text"
          >
            {link.label}
          </Link>
        ))}
        <Link
          href="/download"
          className="ml-1 rounded-[10px] bg-accent px-5 py-2 text-[15px] font-semibold text-white shadow-[inset_0_1px_0_rgba(255,255,255,0.55),0_1px_2px_rgba(13,71,161,0.18),0_6px_16px_rgba(103,190,255,0.32)] transition hover:brightness-105 active:brightness-95"
        >
          Download
        </Link>
      </nav>
    </header>
  );
}
