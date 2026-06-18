import { cn } from "@/lib/cn";

export function Card({
  className,
  children,
  ...rest
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "relative rounded-xl bg-ink-800 hairline overflow-hidden",
        className,
      )}
      {...rest}
    >
      {children}
    </div>
  );
}
