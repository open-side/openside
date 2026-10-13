import { CSPProvider } from "@base-ui/react/csp-provider";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { consumeLocalAccessFragment } from "./localAccess";
import { applyAccent, applyPalettes, applyTheme, loadAccent, loadPalettes, loadTheme } from "./theme";
import { installScrollReveal } from "./ui/scrollReveal";
import "./assets/fonts/wanted-sans/WantedSansVariable.css";
import "./styles.css";

// theme-boot.js already painted these; reapplying covers a boot script that failed to load.
applyTheme(loadTheme());
applyPalettes(loadPalettes());
applyAccent(loadAccent());
// The browser chrome colour follows the OS scheme while the preference is System.
window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener("change", () => applyTheme(loadTheme()));

consumeLocalAccessFragment();
installScrollReveal();

const root = document.getElementById("root");
if (root) {
  // The daemon serves `style-src 'self'`: Base UI's injected <style> (select scrollbar hiding) would be a CSP violation.
  createRoot(root).render(
    <StrictMode>
      <CSPProvider disableStyleElements>
        <App />
      </CSPProvider>
    </StrictMode>,
  );
}

if (import.meta.env.PROD && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch((error: unknown) => console.warn("service worker", error));
  });
}
