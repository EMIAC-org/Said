// Tiny React hook for branching UI on the host OS.
//
// Returns `null` until the underlying Tauri command resolves, then a stable
// platform string ("macos" / "windows" / "linux") matching
// `std::env::consts::OS`. Components that need an immediate non-null value
// should default-branch (e.g. assume macOS) and re-render when it arrives.

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

export type Platform = "macos" | "windows" | "linux" | "unknown";

let cached: Platform | null = null;
let inflight: Promise<Platform> | null = null;

function resolve(): Promise<Platform> {
  if (cached) return Promise.resolve(cached);
  if (inflight) return inflight;
  inflight = invoke<string>("get_platform")
    .then((v) => {
      cached =
        v === "macos" || v === "windows" || v === "linux"
          ? (v as Platform)
          : ("unknown" as const);
      return cached;
    })
    .catch(() => {
      cached = "unknown";
      return cached;
    })
    .finally(() => {
      inflight = null;
    }) as Promise<Platform>;
  return inflight;
}

export function usePlatform(): Platform | null {
  const [p, setP] = useState<Platform | null>(cached);
  useEffect(() => {
    if (cached) return;
    let mounted = true;
    resolve().then((v) => {
      if (mounted) setP(v);
    });
    return () => {
      mounted = false;
    };
  }, []);
  return p;
}
