// Applies the persisted (or OS-preferred) theme to <html data-theme> before
// first paint, so a visitor whose OS prefers dark mode doesn't see a light
// -> dark flash while the WASM app boots. Mirrors the same localStorage
// "theme" key and prefers-color-scheme fallback that wasm-theme's own
// prefers_color_scheme() (theme_toggle(), run post-boot in a use_effect)
// uses — this only sets the attribute for an immediate correct paint, it
// doesn't touch localStorage itself, so wasm-theme's later read+persist
// stays the single source of truth once the app is up.
(function () {
  try {
    var stored = localStorage.getItem("theme");
    var theme =
      stored || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
    document.documentElement.setAttribute("data-theme", theme);
  } catch (e) {
    // localStorage/matchMedia unavailable (e.g. disabled storage) — fall
    // through to the default light theme, same as wasm-theme's own catch-all.
  }
})();
