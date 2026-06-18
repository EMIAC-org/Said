import { cn } from "@/lib/cn";
import { forwardRef } from "react";

type ButtonProps = {
  variant?: "primary" | "secondary" | "ghost";
  size?: "md" | "lg";
} & React.ButtonHTMLAttributes<HTMLButtonElement>;

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "primary", size = "md", className, children, ...rest },
  ref,
) {
  const base =
    "inline-flex items-center justify-center font-medium rounded-xl transition-all duration-200 select-none disabled:opacity-50 disabled:pointer-events-none";
  const sizes = {
    md: "h-10 px-4 text-sm",
    lg: "h-12 px-6 text-base",
  };
  const variants = {
    primary:
      "bg-accent text-ink-900 shadow-[inset_0_1px_0_rgba(255,255,255,0.25),inset_0_-1px_0_rgba(0,0,0,0.15)] hover:bg-accent-soft active:translate-y-px",
    secondary:
      "bg-ink-700 text-ink-50 hairline hover:bg-ink-600 active:translate-y-px",
    ghost:
      "text-ink-100 hover:text-white hover:bg-ink-50/5",
  };
  return (
    <button
      ref={ref}
      className={cn(base, sizes[size], variants[variant], className)}
      {...rest}
    >
      {children}
    </button>
  );
});

type AnchorButtonProps = {
  variant?: "primary" | "secondary" | "ghost";
  size?: "md" | "lg";
} & React.AnchorHTMLAttributes<HTMLAnchorElement>;

export function ButtonLink({
  variant = "primary",
  size = "md",
  className,
  children,
  ...rest
}: AnchorButtonProps) {
  const base =
    "inline-flex items-center justify-center font-medium rounded-xl transition-all duration-200 select-none";
  const sizes = {
    md: "h-10 px-4 text-sm",
    lg: "h-12 px-6 text-base",
  };
  const variants = {
    primary:
      "bg-accent text-ink-900 shadow-[inset_0_1px_0_rgba(255,255,255,0.25),inset_0_-1px_0_rgba(0,0,0,0.15)] hover:bg-accent-soft active:translate-y-px",
    secondary:
      "bg-ink-700 text-ink-50 hairline hover:bg-ink-600 active:translate-y-px",
    ghost:
      "text-ink-100 hover:text-white hover:bg-ink-50/5",
  };
  return (
    <a className={cn(base, sizes[size], variants[variant], className)} {...rest}>
      {children}
    </a>
  );
}
