// Runs before the app bundle so the first paint uses the stored theme and look.
// Mirrors applyTheme in src/theme.ts and applyLook in src/appearance.ts; keep them in step.
(function () {
  var root = document.documentElement;
  var canvas = { light: "#f9f9f9", dark: "#1f1f1f" };
  try {
    var theme = localStorage.getItem("openside.theme");
    root.dataset.theme = theme === "light" || theme === "dark" ? theme : "system";
    // Explicit theme: both metas show the chosen canvas. System: each one keeps its own scheme.
    var metas = document.querySelectorAll('meta[name="theme-color"]');
    for (var i = 0; i < metas.length; i++) {
      var scheme = theme === "light" || theme === "dark" ? theme : /dark/.test(metas[i].getAttribute("media") || "") ? "dark" : "light";
      metas[i].setAttribute("content", canvas[scheme]);
    }
    var look = JSON.parse(localStorage.getItem("openside.look") || "null") || {};
    root.dataset.tabStyle = look.tabStyle === "serif" ? "serif" : "sans";
    root.dataset.transparency = look.transparency === "tinted" ? "tinted" : "transparent";
  } catch (error) {
    root.dataset.theme = "system";
  }
})();
