import { cn } from "@/lib/cn";

type Props = {
  className?: string;
  label?: string;
  aspect?: "video" | "square" | "phone";
  children?: React.ReactNode;
};

export function PlaceholderMedia({
  className,
  label = "Product screenshot",
  aspect = "video",
  children,
}: Props) {
  const aspectClass = {
    video: "aspect-video",
    square: "aspect-square",
    phone: "aspect-[9/19]",
  }[aspect];

  // If there are children (often interactive), don't claim role="img" —
  // it would hide the children from assistive tech. Fall back to a
  // semantically-neutral wrapper and let the children describe themselves.
  const isDecorativeOnly = !children;

  return (
    <div
      className={cn(
        "relative w-full overflow-hidden rounded-2xl bg-ink-800",
        "border border-dashed border-ink-50/10",
        aspectClass,
        className,
      )}
      {...(isDecorativeOnly ? { role: "img", "aria-label": label } : {})}
    >
      <div
        aria-hidden
        className="absolute inset-0 opacity-50"
        style={{
          backgroundImage:
            "radial-gradient(circle at 20% 30%, rgba(165,180,252,0.12), transparent 40%), radial-gradient(circle at 80% 70%, rgba(80,120,255,0.08), transparent 50%)",
        }}
      />
      <div className="absolute inset-0 flex items-center justify-center">
        {children ?? (
          <span className="text-xs uppercase tracking-[0.2em] text-ink-300">
            {label}
          </span>
        )}
      </div>
    </div>
  );
}
