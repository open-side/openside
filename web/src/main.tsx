import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { applyAccent, applyPalettes, applyTheme, loadAccent, loadPalettes, loadTheme } from "./theme";
import "./assets/fonts/wanted-sans/WantedSansVariable.css";
import "./styles.css";

// theme-boot.js already painted these; reapplying covers a boot script that failed to load.
applyTheme(loadTheme());
applyPalettes(loadPalettes());
applyAccent(loadAccent());
// The browser chrome colour follows the OS scheme while the preference is System.
window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener("change", () => applyTheme(loadTheme()));

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

if (import.meta.env.PROD && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch((error: unknown) => console.warn("service worker", error));
  });
}
