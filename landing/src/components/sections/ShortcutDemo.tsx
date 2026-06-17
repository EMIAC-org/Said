"use client";

import { motion, useReducedMotion } from "framer-motion";
import { Section, SectionTitle, SectionLede } from "@/components/ui/Section";
import { Keyboard, type HighlightKey } from "@/components/ui/Keyboard";
import { shortcutDemo, integrations } from "@/lib/content";

// ⌥ at SpaceRow index 2, Space at index 4. The Keyboard renders both with
// the accent fill + pulsing ring + label glyph on the cap.
const HOTKEY_HIGHLIGHT: HighlightKey[] = [
  { row: "space", index: 2, label: "⌥" },
  { row: "space", index: 4, label: "Space" },
];

export function ShortcutDemo() {
  const reduce = useReducedMotion();
  return (
    <Section id="shortcut">
      <div className="grid lg:grid-cols-2 gap-12 lg:gap-16 items-center">
        <div>
          <SectionTitle>{shortcutDemo.title}</SectionTitle>
          <SectionLede>{shortcutDemo.body}</SectionLede>
        </div>

        <div
          className="flex items-center justify-center py-6 md:py-10"
          style={{ perspective: 1600 }}
        >
          <motion.div
            animate={reduce ? undefined : { y: [0, -6, 0] }}
            transition={{ duration: 5, repeat: Infinity, ease: "easeInOut" }}
          >
            <motion.div
              style={
                reduce
                  ? { rotateX: 38, rotateY: -4, rotateZ: -1 }
                  : { rotateX: 38, rotateY: -4, rotateZ: -1 }
              }
            >
              <Keyboard
                variant="hotkey"
                featured={integrations.featured}
                highlightKeys={HOTKEY_HIGHLIGHT}
              />
            </motion.div>
          </motion.div>
        </div>
      </div>
    </Section>
  );
}
