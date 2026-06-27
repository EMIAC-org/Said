import React from "react";
import ReactDOM from "react-dom/client";
import "./styles.css";
import App from "./App";
import StatusBar from "./StatusBar";
import { MeetingPill } from "./components/MeetingPill";

/**
 * Dev-only click/focus diagnostics. Logs every pointerdown/click with the
 * signals that distinguish the failure modes for "buttons don't work":
 *   • no log at all on click   → the click never reached the webview (native
 *                                 window not key / not receiving events)
 *   • logged but focus:false   → window isn't the key window (first-mouse eaten)
 *   • logged, sameElement:false→ an overlay is sitting on top of the target
 *   • logged, focus:true, on button, yet handler never runs → JS/handler issue
 * Watch these in the dev window's DevTools console. Stripped from release builds.
 */
function installClickDiagnostics() {
  if (!import.meta.env.DEV) return;
  const desc = (el: Element | null): string => {
    if (!el) return "null";
    const tag = el.tagName.toLowerCase();
    const cls =
      typeof el.className === "string" && el.className.trim()
        ? "." + el.className.trim().split(/\s+/).slice(0, 3).join(".")
        : "";
    const txt = (el.textContent || "").trim().slice(0, 24);
    return `${tag}${cls}${txt ? ` "${txt}"` : ""}`;
  };
  const log = (type: string) => (e: Event) => {
    const me = e as MouseEvent;
    const hit = document.elementFromPoint(me.clientX, me.clientY);
    const target = e.target as Element | null;
    console.info(`[click-diag] ${type}`, {
      focus: document.hasFocus(),
      visible: document.visibilityState,
      xy: `${me.clientX},${me.clientY}`,
      target: desc(target),
      elementFromPoint: desc(hit),
      sameElement: hit === target,
      targetIsButton: !!target?.closest?.("button, a, [role='button']"),
      topIsButton: !!hit?.closest?.("button, a, [role='button']"),
    });
  };
  window.addEventListener("pointerdown", log("pointerdown"), true);
  window.addEventListener("click", log("click"), true);
  window.addEventListener("focus", () => console.info("[click-diag] window FOCUS"));
  window.addEventListener("blur", () => console.info("[click-diag] window BLUR"));
  document.addEventListener("visibilitychange", () =>
    console.info("[click-diag] visibility →", document.visibilityState, "focus:", document.hasFocus()),
  );
  console.info(
    "[click-diag] installed. Click a 'dead' button and read the line: no line = native focus issue; focus:false = window not key; sameElement:false = overlay.",
  );
}

const root = ReactDOM.createRoot(document.getElementById("app")!);
const params = new URLSearchParams(window.location.search);
const isStatusBar =
  window.location.hash === "#statusbar" ||
  params.get("view") === "statusbar" ||
  params.has("statusbar");
const isMeetingPill =
  window.location.hash === "#meeting-pill" || params.get("view") === "meeting-pill";

console.info("[status-bar:entry]", {
  href: window.location.href,
  hash: window.location.hash,
  search: window.location.search,
  isStatusBar,
});

if (isMeetingPill) {
  // Floating live-meeting pill — minimal always-on-top capsule.
  document.documentElement.dataset.theme = "dark";
  document.documentElement.classList.add("statusbar-mode");
  document.body.classList.add("statusbar-mode");
  root.render(
    <React.StrictMode>
      <MeetingPill />
    </React.StrictMode>,
  );
} else if (isStatusBar) {
  // Floating status-bar window — always dark, independent of main-app theme
  document.documentElement.dataset.theme = "dark";
  document.documentElement.classList.add("statusbar-mode");
  document.body.classList.add("statusbar-mode");
  root.render(
    <React.StrictMode>
      <StatusBar />
    </React.StrictMode>,
  );
} else {
  // Main application window
  installClickDiagnostics();
  root.render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
