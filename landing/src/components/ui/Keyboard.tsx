"use client";

import { getBrandIcon, isHexDark, type Brand } from "@/lib/brandIcons";
import { cn } from "@/lib/cn";

type RowName = "function" | "icon" | "qwerty" | "asdf" | "zxcv" | "space";

export type HighlightKey = {
  row: RowName;
  index: number;
  /** Optional label rendered on the highlighted key (e.g. "⌥", "Space"). */
  label?: string;
};

export type FeaturedApp = { name: string; slug: string };

type Variant = "showcase" | "hotkey";

type Props = {
  featured: ReadonlyArray<FeaturedApp>;
  highlightKeys?: ReadonlyArray<HighlightKey>;
  variant?: Variant;
  className?: string;
};

/* ---------------------------------- Sticker --------------------------------- */

function IconSticker({ brand }: { brand: Brand }) {
  const dark = isHexDark(brand.hex);
  const bg = dark ? `#${brand.hex}` : "#ffffff";
  const fg = dark ? "#ffffff" : `#${brand.hex}`;
  return (
    <div
      className="grid place-items-center rounded-[22%]"
      style={{
        width: "calc(var(--key) * 0.78)",
        height: "calc(var(--key) * 0.78)",
        background: bg,
        boxShadow:
          "0 2px 4px rgba(0,0,0,0.35), inset 0 1px 0 rgba(255,255,255,0.45)",
      }}
    >
      <svg
        role="img"
        viewBox="0 0 24 24"
        width="55%"
        height="55%"
        aria-label={brand.title}
      >
        <title>{brand.title}</title>
        <path d={brand.path} fill={fg} />
      </svg>
    </div>
  );
}

/* ------------------------------------ Key ----------------------------------- */

type KeyProps = {
  width?: number;
  height?: number;
  children?: React.ReactNode;
  highlighted?: boolean;
  label?: string;
};

function Key({
  width = 1,
  height = 1,
  children,
  highlighted = false,
  label,
}: KeyProps) {
  return (
    <div
      className={cn(
        "grid place-items-center shrink-0 rounded-md select-none relative",
        highlighted && "animate-keyHighlight",
      )}
      style={{
        width: `calc(var(--key) * ${width} + var(--gap) * ${Math.max(0, width - 1)})`,
        height: `calc(var(--key) * ${height} + var(--gap) * ${Math.max(0, height - 1)})`,
        background: highlighted
          ? "linear-gradient(180deg, #c7d2fe 0%, #818cf8 100%)"
          : "linear-gradient(180deg, #2a2a2d 0%, #1c1c1f 100%)",
        boxShadow: highlighted
          ? "inset 0 1px 0 rgba(255,255,255,0.4), inset 0 -2px 0 rgba(0,0,0,0.25), 0 2px 4px rgba(0,0,0,0.5)"
          : "inset 0 1px 0 rgba(255,255,255,0.08), inset 0 -2px 0 rgba(0,0,0,0.45), 0 1px 1px rgba(0,0,0,0.6)",
        border: highlighted
          ? "1px solid rgba(49, 46, 129, 0.7)"
          : "1px solid rgba(0,0,0,0.45)",
      }}
    >
      {children}
      {label && (
        <span
          className="absolute font-medium text-[#1e1b4b] pointer-events-none"
          style={{ fontSize: "calc(var(--key) * 0.28)" }}
        >
          {label}
        </span>
      )}
    </div>
  );
}

function Row({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex" style={{ gap: "var(--gap)" }}>
      {children}
    </div>
  );
}

/* --------------------------- Highlight resolver utility -------------------- */

function makeHighlightSet(keys: ReadonlyArray<HighlightKey> = []) {
  const set = new Map<string, string | undefined>();
  for (const k of keys) set.set(`${k.row}:${k.index}`, k.label);
  return set;
}

/* ----------------------------------- Rows ----------------------------------- */

function FunctionRow({ hl }: { hl: Map<string, string | undefined> }) {
  return (
    <Row>
      {Array.from({ length: 14 }).map((_, i) => {
        const key = `function:${i}`;
        const isHl = hl.has(key);
        return (
          <Key
            key={i}
            height={0.55}
            highlighted={isHl}
            label={hl.get(key)}
          />
        );
      })}
    </Row>
  );
}

function IconRow({
  featured,
  hl,
}: {
  featured: ReadonlyArray<FeaturedApp>;
  hl: Map<string, string | undefined>;
}) {
  const apps = featured.slice(0, 14);
  return (
    <Row>
      {apps.map((app, i) => {
        const brand = getBrandIcon(app.slug);
        const key = `icon:${i}`;
        const isHl = hl.has(key);
        return (
          <Key key={app.slug} highlighted={isHl} label={hl.get(key)}>
            {brand ? <IconSticker brand={brand} /> : null}
          </Key>
        );
      })}
    </Row>
  );
}

function LetterRow({
  rowName,
  count,
  leading,
  trailing,
  hl,
}: {
  rowName: "qwerty" | "asdf" | "zxcv";
  count: number;
  leading?: number;
  trailing?: number;
  hl: Map<string, string | undefined>;
}) {
  let index = 0;
  const nextKey = () => {
    const key = `${rowName}:${index}`;
    const props = { highlighted: hl.has(key), label: hl.get(key) };
    index++;
    return props;
  };
  return (
    <Row>
      {leading ? <Key width={leading} {...nextKey()} /> : null}
      {Array.from({ length: count }).map((_, i) => (
        <Key key={i} {...nextKey()} />
      ))}
      {trailing ? <Key width={trailing} {...nextKey()} /> : null}
    </Row>
  );
}

function SpaceRow({ hl }: { hl: Map<string, string | undefined> }) {
  // Index map (kept Mac-ish):
  // 0:fn  1:ctrl  2:⌥  3:⌘  4:Space  5:⌘  6:⌥  7:arrows
  const cfg: Array<{ width: number }> = [
    { width: 1.25 }, // fn
    { width: 1.25 }, // ctrl
    { width: 1.25 }, // ⌥
    { width: 1.25 }, // ⌘
    { width: 6.0 },  // Space
    { width: 1.25 }, // ⌘
    { width: 1.25 }, // ⌥
    { width: 1.5 },  // arrows
  ];
  return (
    <Row>
      {cfg.map((c, i) => {
        const key = `space:${i}`;
        return (
          <Key
            key={i}
            width={c.width}
            highlighted={hl.has(key)}
            label={hl.get(key)}
          />
        );
      })}
    </Row>
  );
}

/* --------------------------------- Keyboard --------------------------------- */

export function Keyboard({
  featured,
  highlightKeys,
  variant = "showcase",
  className,
}: Props) {
  const hl = makeHighlightSet(highlightKeys);

  // Single source of truth for key sizing. The `hotkey` variant runs ~22%
  // smaller to read as a focused close-up next to copy, rather than a hero
  // shot.
  const keyClamp =
    variant === "hotkey"
      ? "clamp(18px, 3.6vw, 46px)"
      : "clamp(22px, 4.5vw, 58px)";

  return (
    <div
      style={
        {
          "--key": keyClamp,
          "--gap": "clamp(3px, 0.5vw, 6px)",
        } as React.CSSProperties
      }
      className={cn("relative inline-flex flex-col rounded-2xl", className)}
    >
      <div
        className="relative rounded-2xl"
        style={{
          padding: "calc(var(--gap) * 2.2)",
          background: "linear-gradient(180deg, #1a1a1d 0%, #0e0e10 100%)",
          border: "1px solid rgba(255,255,255,0.06)",
          boxShadow:
            "0 60px 80px -30px rgba(0,0,0,0.7), inset 0 1px 0 rgba(255,255,255,0.04)",
        }}
      >
        <div className="flex flex-col" style={{ gap: "var(--gap)" }}>
          <FunctionRow hl={hl} />
          <IconRow featured={featured} hl={hl} />
          <LetterRow rowName="qwerty" count={14} hl={hl} />
          <LetterRow rowName="asdf" count={13} trailing={1.5} hl={hl} />
          <LetterRow rowName="zxcv" count={12} leading={1.5} trailing={1.5} hl={hl} />
          <SpaceRow hl={hl} />
        </div>
      </div>
    </div>
  );
}
