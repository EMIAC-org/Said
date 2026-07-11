import * as React from "react";
import { cn } from "@/lib/utils";

/**
 * Skeleton — a single shimmering placeholder block, shared by every view so the
 * loading treatment reads as one system. Compose several of these into a
 * content-shaped skeleton that mirrors the real layout (see the per-view
 * *Skeleton components), rather than showing a lone spinner or stale data.
 */
function Skeleton({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div aria-hidden className={cn("skeleton", className)} {...props} />;
}

export { Skeleton };
