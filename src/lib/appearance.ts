/**
 * Settings > General > Appearance: user overrides of the Eole tokens in
 * styles/tokens.css, applied as inline custom properties on <html>. A blank
 * pref keeps the token's Eole value.
 */
import { hexToRgba } from "./color";
import type { Prefs } from "./stores/prefs.svelte";

/** First family of the --font-default stack, shown as the font box's placeholder. */
export const DEFAULT_FONT = "Selawik";

/** The colors the Appearance settings can change. Each `default` must match
    the token's value in tokens.css; `alpha` marks a token stored translucent,
    which the picker edits as its opaque base color. */
export const THEME_COLORS = [
  { pref: "color_bg_main", token: "--bg-main", label: "Background", default: "#1e1e1e" },
  { pref: "color_bg_panel", token: "--bg-panel", label: "Panel background", default: "#1b1b1b" },
  { pref: "color_txt", token: "--txt", label: "Text", default: "#f0f0f0" },
  { pref: "color_txt_full", token: "--txt-full", label: "Highlighted text", default: "#ffffff" },
  { pref: "color_txt_semifaded", token: "--txt-semifaded", label: "Secondary text", default: "#888888" },
  { pref: "color_txt_faded", token: "--txt-faded", label: "Faded text", default: "#6e6e6e" },
  { pref: "color_accent", token: "--accent", label: "Now playing", default: "#ffaf32" },
  { pref: "color_selection", token: "--selection", label: "Selection", default: "#0fb1ff", alpha: 0.63 },
] as const;

export type ThemeColor = (typeof THEME_COLORS)[number];
export type ColorPref = ThemeColor["pref"];

/** Every token applyAppearance may set, so a cleared pref removes its override. */
const TOKENS = [...THEME_COLORS.map((c) => c.token), "--bg-headerbar", "--font-family"];

const GENERIC_FAMILIES = new Set([
  "serif",
  "sans-serif",
  "monospace",
  "cursive",
  "fantasy",
  "system-ui",
  "ui-serif",
  "ui-sans-serif",
  "ui-monospace",
  "ui-rounded",
  "emoji",
  "math",
  "fangsong",
]);

/** `Noto Sans, serif` → `"Noto Sans", serif`. Named families are quoted so a
    name with digits or punctuation stays valid CSS; generic keywords stay bare. */
function fontStack(input: string): string {
  return input
    .split(",")
    .map((f) => f.trim().replace(/^["']|["']$/g, "").trim())
    .filter(Boolean)
    .map((f) =>
      GENERIC_FAMILIES.has(f.toLowerCase())
        ? f.toLowerCase()
        : `"${f.replace(/["\\]/g, "\\$&")}"`,
    )
    .join(", ");
}

function overrides(values: Prefs): Record<string, string> {
  const out: Record<string, string> = {};
  for (const c of THEME_COLORS) {
    const hex = values[c.pref];
    if (hex) out[c.token] = "alpha" in c ? hexToRgba(hex, c.alpha) : hex;
  }
  // The transport bar's translucent backdrop is the main background at 86%,
  // so it follows a custom background instead of staying Eole grey.
  if (values.color_bg_main) out["--bg-headerbar"] = hexToRgba(values.color_bg_main, 0.86);
  // The default stack stays behind the custom font, so characters it lacks
  // (Japanese text, for one) still render.
  const stack = fontStack(values.font_family);
  if (stack) out["--font-family"] = `${stack}, var(--font-default)`;
  return out;
}

function setOverrides(map: Record<string, unknown>) {
  const style = document.documentElement.style;
  for (const token of TOKENS) {
    const value = map[token];
    if (typeof value === "string" && value) style.setProperty(token, value);
    else style.removeProperty(token);
  }
}

/** localStorage copy of the last applied overrides. The prefs table stays the
    source of truth; this copy exists for code that runs before prefs load
    (main-window startup) or cannot read them (the About and cover windows). */
export const APPEARANCE_CACHE_KEY = "crescendo.appearance";

/** Apply the appearance prefs and refresh the cached copy. */
export function applyAppearance(values: Prefs) {
  const map = overrides(values);
  setOverrides(map);
  try {
    localStorage.setItem(APPEARANCE_CACHE_KEY, JSON.stringify(map));
  } catch {
    // Without storage the next startup shows Eole colors until prefs load.
  }
}

/** Apply the cached overrides from the last applyAppearance, if any. */
export function restoreAppearance() {
  let map: unknown = null;
  try {
    map = JSON.parse(localStorage.getItem(APPEARANCE_CACHE_KEY) ?? "null");
  } catch {
    map = null;
  }
  setOverrides(map && typeof map === "object" ? (map as Record<string, unknown>) : {});
}
