"use client";

import * as Accordion from "@radix-ui/react-accordion";
import { Plus } from "lucide-react";
import { Section, SectionTitle } from "@/components/ui/Section";
import { faq } from "@/lib/content";

export function FAQ() {
  return (
    <Section id="faq">
      <SectionTitle className="max-w-3xl">{faq.title}</SectionTitle>

      <Accordion.Root
        type="single"
        collapsible
        className="mt-12 max-w-3xl rounded-2xl bg-ink-800 hairline divide-y divide-ink-50/5 overflow-hidden"
      >
        {faq.items.map((item, i) => (
          <Accordion.Item key={i} value={`item-${i}`}>
            <Accordion.Header>
              <Accordion.Trigger className="group w-full flex items-center justify-between px-6 py-5 text-left hover:bg-ink-50/[0.03] transition-colors">
                <span className="text-base text-ink-50 font-medium pr-4">
                  {item.q}
                </span>
                <Plus
                  className="h-4 w-4 text-ink-200 shrink-0 transition-transform duration-300 group-data-[state=open]:rotate-45"
                  strokeWidth={2}
                />
              </Accordion.Trigger>
            </Accordion.Header>
            <Accordion.Content className="overflow-hidden data-[state=open]:animate-faqDown data-[state=closed]:animate-faqUp">
              <div className="px-6 pb-6 text-ink-200 leading-relaxed">{item.a}</div>
            </Accordion.Content>
          </Accordion.Item>
        ))}
      </Accordion.Root>
    </Section>
  );
}
