"use client";

import { motion } from "framer-motion";
import { Quote } from "lucide-react";
import { Section, SectionTitle } from "@/components/ui/Section";
import { testimonials } from "@/lib/content";

function Avatar({ seed }: { seed: string }) {
  const hue = (seed.charCodeAt(0) * 23) % 360;
  return (
    <div
      aria-hidden
      className="h-10 w-10 rounded-full shrink-0"
      style={{
        background: `linear-gradient(135deg, hsl(${hue}, 55%, 55%), hsl(${
          (hue + 60) % 360
        }, 45%, 35%))`,
      }}
    />
  );
}

export function Testimonials() {
  return (
    <Section id="testimonials">
      <SectionTitle className="max-w-3xl">{testimonials.title}</SectionTitle>

      <div className="mt-12 grid md:grid-cols-3 gap-4">
        {testimonials.items.map((t, i) => (
          <motion.figure
            key={i}
            initial={{ opacity: 0, y: 16 }}
            whileInView={{ opacity: 1, y: 0 }}
            viewport={{ once: true, margin: "-10% 0px" }}
            transition={{
              duration: 0.5,
              delay: i * 0.08,
              ease: [0.22, 1, 0.36, 1],
            }}
            className="rounded-2xl bg-ink-800 hairline p-6 flex flex-col"
          >
            <Quote className="h-5 w-5 text-accent mb-4" strokeWidth={1.5} />
            <blockquote className="text-base text-ink-50 leading-relaxed flex-1">
              {t.quote}
            </blockquote>
            <figcaption className="mt-6 flex items-center gap-3">
              <Avatar seed={t.name} />
              <div>
                <div className="text-sm font-medium text-ink-50">{t.name}</div>
                <div className="text-xs text-ink-300">{t.role}</div>
              </div>
            </figcaption>
          </motion.figure>
        ))}
      </div>
    </Section>
  );
}
