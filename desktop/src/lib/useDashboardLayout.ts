import { useEffect, useState, useCallback } from "react";

/**
 * Dashboard layout preference — UI-only, persisted in localStorage.
 *
 * - `split`     : two-column "Insights ⟷ Timeline" view (default)
 * - `editorial` : single-column magazine-style daily summary
 *
 * Every call site stays in sync via two channels:
 *   1. `storage` event — fires when another window/tab writes the key.
 *   2. A same-window `CustomEvent` on the `window` — `storage` does not
 *      fire in the originating window, so without this the Settings
 *      modal would update localStorage and its own state but the
 *      DashboardView's hook instance would stay stale.
 */
export type DashboardLayout = "split" | "editorial";

const KEY  = "said:dashboard-layout";
const EVT  = "said:dashboard-layout-change";
const DEFAULT: DashboardLayout = "split";

function read(): DashboardLayout {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "split" || v === "editorial") return v;
  } catch {
    /* ignore */
  }
  return DEFAULT;
}

export function useDashboardLayout(): {
  layout: DashboardLayout;
  setLayout: (next: DashboardLayout) => void;
} {
  const [layout, setLayoutState] = useState<DashboardLayout>(read);

  useEffect(() => {
    function onStorage(e: StorageEvent) {
      if (e.key !== KEY) return;
      const v = e.newValue;
      if (v === "split" || v === "editorial") setLayoutState(v);
    }
    function onLocal(e: Event) {
      const v = (e as CustomEvent<DashboardLayout>).detail;
      if (v === "split" || v === "editorial") setLayoutState(v);
    }
    window.addEventListener("storage", onStorage);
    window.addEventListener(EVT, onLocal as EventListener);
    return () => {
      window.removeEventListener("storage", onStorage);
      window.removeEventListener(EVT, onLocal as EventListener);
    };
  }, []);

  const setLayout = useCallback((next: DashboardLayout) => {
    setLayoutState(next);
    try {
      localStorage.setItem(KEY, next);
    } catch {
      /* ignore storage failures */
    }
    // Notify other hook instances in this same window.
    window.dispatchEvent(new CustomEvent<DashboardLayout>(EVT, { detail: next }));
  }, []);

  return { layout, setLayout };
}
