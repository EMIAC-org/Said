import React from "react";
import ReactDOM from "react-dom/client";
import "./styles.css";
import App from "./App";
import StatusBar from "./StatusBar";

const root = ReactDOM.createRoot(document.getElementById("app")!);
const params = new URLSearchParams(window.location.search);
const isStatusBar =
  window.location.hash === "#statusbar" ||
  params.get("view") === "statusbar" ||
  params.has("statusbar");

console.info("[status-bar:entry]", {
  href: window.location.href,
  hash: window.location.hash,
  search: window.location.search,
  isStatusBar,
});

if (isStatusBar) {
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
  root.render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
