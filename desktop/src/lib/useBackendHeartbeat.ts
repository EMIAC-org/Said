import { useEffect, useRef, useState } from "react";
import { getBackendEndpoint } from "./invoke";

export type HealthLevel = "green" | "yellow" | "orange" | "red" | "unreachable";

interface HeartbeatState {
  level: HealthLevel;
  /** True when the backend is unreachable or critically degraded. */
  showOverlay: boolean;
  /** True once the backend recovers after an overlay was shown. */
  justRecovered: boolean;
}

const PING_INTERVAL_MS = 4_000;
const PING_TIMEOUT_MS = 3_000;
const UNREACHABLE_THRESHOLD = 2;
const RECOVERY_TOAST_MS = 3_000;

export function useBackendHeartbeat(): HeartbeatState {
  const [level, setLevel] = useState<HealthLevel>("green");
  const [showOverlay, setShowOverlay] = useState(false);
  const [justRecovered, setJustRecovered] = useState(false);
  const failCount = useRef(0);
  const wasOverlay = useRef(false);

  useEffect(() => {
    let cancelled = false;
    let endpointUrl: string | null = null;

    async function resolveEndpoint() {
      const ep = await getBackendEndpoint();
      if (ep) endpointUrl = ep.url;
    }

    async function ping() {
      if (cancelled) return;

      if (!endpointUrl) {
        await resolveEndpoint();
        if (!endpointUrl) return;
      }

      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), PING_TIMEOUT_MS);

      try {
        const resp = await fetch(`${endpointUrl}/v1/health/ping`, {
          signal: controller.signal,
        });
        clearTimeout(timeout);

        if (!resp.ok) throw new Error(`status ${resp.status}`);

        const data = await resp.json();
        const backendLevel = (data.level ?? "green") as HealthLevel;

        setLevel(backendLevel);

        if (backendLevel === "red" || backendLevel === "orange") {
          failCount.current += 1;
          if (failCount.current >= UNREACHABLE_THRESHOLD) {
            setShowOverlay(true);
            wasOverlay.current = true;
          }
        } else {
          failCount.current = 0;
          if (wasOverlay.current) {
            wasOverlay.current = false;
            setShowOverlay(false);
            setJustRecovered(true);
            setTimeout(() => {
              if (!cancelled) setJustRecovered(false);
            }, RECOVERY_TOAST_MS);
          }
        }
      } catch {
        clearTimeout(timeout);
        failCount.current += 1;

        // The cached endpoint may be stale — the backend can respawn on a new
        // port after a dev rebuild or a watchdog restart. Drop it so the next
        // ping re-resolves the current port instead of pinging a dead one
        // forever (otherwise the overlay is stuck until a manual window reload).
        endpointUrl = null;

        if (failCount.current >= UNREACHABLE_THRESHOLD) {
          setLevel("unreachable");
          setShowOverlay(true);
          wasOverlay.current = true;
        }
      }
    }

    const id = setInterval(ping, PING_INTERVAL_MS);
    ping();

    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  return { level, showOverlay, justRecovered };
}
