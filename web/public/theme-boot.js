// Runs before the app bundle so the first paint uses the stored theme, palettes and accent.
// Mirrors applyTheme/applyPalettes/applyAccent in src/theme.ts; keep the two in step.
(function () {
  var root = document.documentElement;
  var hex = /^#[0-9a-f]{6}$/i;
  var foreground = function (color) {
    var lin = function (offset) {
      var v = parseInt(color.slice(offset, offset + 2), 16) / 255;
      return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
    };
    return 0.2126 * lin(1) + 0.7152 * lin(3) + 0.0722 * lin(5) > 0.4 ? "#000000" : "#ffffff";
  };
  try {
    var theme = localStorage.getItem("openside.theme");
    root.dataset.theme = theme === "light" || theme === "dark" ? theme : "system";
    var palettes = JSON.parse(localStorage.getItem("openside.palette") || "null") || {};
    ["light", "dark"].forEach(function (scheme) {
      var p = palettes[scheme];
      if (!p || typeof p.id !== "string" || !hex.test(p.background) || !hex.test(p.foreground) || !hex.test(p.accent)) return;
      root.style.setProperty("--" + scheme + "-canvas", p.background);
      root.style.setProperty("--" + scheme + "-text", p.foreground);
      root.style.setProperty("--" + scheme + "-accent", p.accent);
      root.style.setProperty("--" + scheme + "-accent-fg", foreground(p.accent));
      root.dataset[scheme + "Palette"] = p.id;
    });
    var accent = localStorage.getItem("openside.accent");
    if (accent && hex.test(accent)) {
      root.style.setProperty("--primary", accent.toLowerCase());
      root.style.setProperty("--primary-foreground", foreground(accent));
    }
    var canvasOf = function (scheme) {
      var p = palettes[scheme];
      return p && hex.test(p.background) ? p.background : scheme === "dark" ? "#000000" : "#ffffff";
    };
    // Explicit theme: both metas show the chosen canvas. System: each one keeps its own scheme.
    var metas = document.querySelectorAll('meta[name="theme-color"]');
    for (var i = 0; i < metas.length; i++) {
      var scheme = theme === "light" || theme === "dark" ? theme : /dark/.test(metas[i].getAttribute("media") || "") ? "dark" : "light";
      metas[i].setAttribute("content", canvasOf(scheme));
    }
  } catch (error) {
    root.dataset.theme = "system";
  }
})();
