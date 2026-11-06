import { CSPProvider } from "@base-ui/react/csp-provider";
import { StrictMode, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { applyLocalePreference, loadLocalePreference } from "./i18n";
import { useLocale } from "./i18n/react";
import { consumeLocalAccessFragment } from "./localAccess";
import { applyLook, loadLook } from "./appearance";
import { applyTheme, forgetRetiredThemeKeys, loadTheme } from "./theme";
import { installScrollReveal } from "./ui/scrollReveal";
import "./assets/fonts/wanted-sans/WantedSansVariable.css";
import "./tokens.css";
import "./styles.css";
import "./shell.css";
import "./components/thread.css";
import "./components/composer.css";

forgetRetiredThemeKeys();
// theme-boot.js already painted this; reapplying covers a boot script that failed to load.
applyTheme(loadTheme());
applyLook(loadLook());
// The browser chrome colour follows the OS scheme while the preference is System.
window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener("change", () => applyTheme(loadTheme()));

consumeLocalAccessFragment();
installScrollReveal();
applyLocalePreference();
// A System preference follows the browser when its language list changes.
window.addEventListener("languagechange", () => {
  if (loadLocalePreference() === "system") applyLocalePreference();
});

/** Remounting on a language switch re-renders every string, including memoised and stateful ones. */
function LocalizedApp() {
  return <App key={useLocale()} />;
}

const root = document.getElementById("root");
if (root) {
  // The daemon serves `style-src 'self'`: Base UI's injected <style> (select scrollbar hiding) would be a CSP violation.
  const render = (page: ReactNode) =>
    createRoot(root).render(
      <StrictMode>
        <CSPProvider disableStyleElements>{page}</CSPProvider>
      </StrictMode>,
    );
  // Dynamic so the dev-only UI kit catalogue (`/__kit`) never reaches the production bundle.
  if (import.meta.env.DEV && window.location.pathname === "/__kit") void import("./ui/kit/KitPage").then(({ KitPage }) => render(<KitPage />));
  else render(<LocalizedApp />);
}

if (import.meta.env.PROD && "serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("/sw.js").catch((error: unknown) => console.warn("service worker", error));
  });
}
