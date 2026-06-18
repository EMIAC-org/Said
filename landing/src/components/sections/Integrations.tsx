"use client";

import { useRef } from "react";
import {
  motion,
  useMotionValue,
  useReducedMotion,
  useSpring,
  useTransform,
} from "framer-motion";
import { Section, SectionTitle, SectionLede } from "@/components/ui/Section";
import { Keyboard } from "@/components/ui/Keyboard";
import { integrations } from "@/lib/content";

export function Integrations() {
  const reduce = useReducedMotion();
  const stageRef = useRef<HTMLDivElement>(null);

  const mx = useMotionValue(0);
  const my = useMotionValue(0);
  const spring = { stiffness: 80, damping: 20, mass: 0.5 };

  const rawRotateY = useTransform(mx, [-1, 1], [-3, 3]);
  const rawRotateXOffset = useTransform(my, [-1, 1], [2, -2]);
  const rotateY = useSpring(rawRotateY, spring);
  const rotateXOffset = useSpring(rawRotateXOffset, spring);
  const rotateX = useTransform(rotateXOffset, (v) => 45 + v);

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (reduce || !stageRef.current) return;
    if (e.pointerType === "touch") return;
    const rect = stageRef.current.getBoundingClientRect();
    const x = (e.clientX - rect.left) / rect.width;
    const y = (e.clientY - rect.top) / rect.height;
    mx.set(Math.max(-1, Math.min(1, x * 2 - 1)));
    my.set(Math.max(-1, Math.min(1, y * 2 - 1)));
  };
  const onPointerLeave = () => {
    mx.set(0);
    my.set(0);
  };

  return (
    <Section id="integrations">
      <div className="max-w-3xl">
        <SectionTitle>{integrations.title}</SectionTitle>
        <SectionLede>{integrations.body}</SectionLede>
      </div>

      <div
        ref={stageRef}
        onPointerMove={onPointerMove}
        onPointerLeave={onPointerLeave}
        className="mt-16 flex items-center justify-center py-10 md:py-16"
        style={{ perspective: 1800 }}
      >
        <motion.div
          animate={reduce ? undefined : { y: [0, -10, 0] }}
          transition={{ duration: 6, repeat: Infinity, ease: "easeInOut" }}
        >
          <motion.div
            style={
              reduce
                ? { rotateX: 45, rotateY: -8, rotateZ: -2 }
                : { rotateX, rotateY, rotateZ: -2 }
            }
          >
            <Keyboard variant="showcase" featured={integrations.featured} />
          </motion.div>
        </motion.div>
      </div>
    </Section>
  );
}
