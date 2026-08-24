(() => {
  "use strict";

  const storageKey = "racetoturin.theme";
  const root = document.documentElement;
  const explicitThemes = new Set(["light", "dark"]);
  let preference = "system";

  try {
    const saved = localStorage.getItem(storageKey);
    if (explicitThemes.has(saved)) preference = saved;
  } catch {
    // Storage can be unavailable in privacy modes; the system theme still works.
  }

  function applyTheme(next, persist) {
    preference = explicitThemes.has(next) ? next : "system";

    if (preference === "system") {
      root.removeAttribute("data-theme");
    } else {
      root.dataset.theme = preference;
    }

    if (persist) {
      try {
        if (preference === "system") {
          localStorage.removeItem(storageKey);
        } else {
          localStorage.setItem(storageKey, preference);
        }
      } catch {
        // The in-memory choice still applies for this page.
      }
    }

    document.querySelectorAll("[data-theme-option]").forEach((button) => {
      button.setAttribute(
        "aria-pressed",
        String(button.dataset.themeOption === preference),
      );
    });
  }

  // This executes in <head>, before CSS is requested, to avoid a theme flash.
  applyTheme(preference, false);

  function connectPicker() {
    const picker = document.querySelector("[data-theme-picker]");
    if (!picker) return;

    picker.querySelectorAll("[data-theme-option]").forEach((button) => {
      button.addEventListener("click", () => {
        applyTheme(button.dataset.themeOption, true);
      });
    });
    applyTheme(preference, false);
    picker.hidden = false;
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", connectPicker, { once: true });
  } else {
    connectPicker();
  }
})();
