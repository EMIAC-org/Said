"use client";

import { motion, useReducedMotion } from "framer-motion";
import { cn } from "@/lib/cn";

type SectionProps = {
  id?: string;
  className?: string;
  children: React.ReactNode;
  bleed?: boolean;
};

export function Section({ id, className, children, bleed }: SectionProps) {
  const reduce = useReducedMotion();
  return (
    <motion.section
      id={id}
      initial={reduce ? false : { opacity: 0, y: 16 }}
      whileInView={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.6, ease: [0.22, 1, 0.36, 1] }}
      viewport={{ once: true, margin: "-10% 0px" }}
      className={cn(
        "relative py-10 md:py-14",
        !bleed && "container-page",
        className,
      )}
    >
      {children}
    </motion.section>
  );
}

export function SectionEyebrow({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-xs uppercase tracking-[0.2em] text-ink-200 mb-4">
      {children}
    </p>
  );
}

export function SectionTitle({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <h2
      className={cn(
        "font-display text-4xl md:text-5xl lg:text-6xl tracking-tightest leading-[1.05] text-balance",
        className,
      )}
    >
      {children}
    </h2>
  );
}

export function SectionLede({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <p
      className={cn(
        "mt-6 text-lg md:text-xl text-ink-200 max-w-2xl leading-relaxed text-balance",
        className,
      )}
    >
      {children}
    </p>
  );
}
