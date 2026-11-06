import { cubicOut } from "svelte/easing";
import { scale, slide } from "svelte/transition";
import { prefs } from "./stores/prefs.svelte";

function reducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

// ---- Menus ----

const MENU_MS = 120;
const MENU_START_SCALE = 0.6;
const DROPDOWN_MS = 150;

/** Whether a menu opening or closing right now should animate. */
export function menusAnimate(): boolean {
  return prefs.transitions && !reducedMotion();
}

/** A menu expands out of its `transform-origin` and collapses back into it. */
export function menuTransition(node: Element) {
  return scale(node, {
    duration: menusAnimate() ? MENU_MS : 0,
    start: MENU_START_SCALE,
    easing: cubicOut,
  });
}

/** A dropdown list unrolls from the edge it is anchored to and rolls back up. */
export function dropdownTransition(node: Element) {
  return slide(node, { duration: menusAnimate() ? DROPDOWN_MS : 0, easing: cubicOut });
}

/** The same motion for a menu kept in the DOM, which a `transition:` cannot
    run on. Returns nothing when menus do not animate. */
export function animateMenu(el: Element, direction: "expand" | "collapse"): Animation | undefined {
  if (!menusAnimate()) return undefined;
  const collapsed = { transform: `scale(${MENU_START_SCALE})`, opacity: 0 };
  const expanded = { transform: "none", opacity: 1 };
  return el.animate(direction === "expand" ? [collapsed, expanded] : [expanded, collapsed], {
    duration: MENU_MS,
    easing: "cubic-bezier(0.33, 1, 0.68, 1)", // cubicOut
  });
}

// ---- Wheel scrolling ----

/** Stiffness of the spring that carries the scroll position to its target, per
    millisecond. A wheel notch of 86 px (WebKitGTK's step in an 800 px window)
    settles in about 250 ms and moves at most 15 px in a 60 Hz frame. WebKit's
    own animation takes 6 frames and jumps up to 34 px in one. */
const SPRING = 1 / 35;

/** A wheel notch reports at least this many pixels. Touchpads and high-resolution
    wheels send a stream of smaller deltas, already smooth. */
const NOTCH_PX = 40;

/** The nearest element from `from` upwards that scrolls vertically and has room
    left in direction `dir`. */
function scrollerFor(from: Element | null, dir: number): Element | null {
  for (let el = from; el; el = el.parentElement) {
    const max = el.scrollHeight - el.clientHeight;
    if (max <= 0) continue;
    const overflow = getComputedStyle(el).overflowY;
    if (overflow !== "auto" && overflow !== "scroll") continue;
    if (dir > 0 ? Math.ceil(el.scrollTop) < max : el.scrollTop > 0) return el;
  }
  return null;
}

/**
 * Take over wheel notches for the whole window and ease each one with a
 * critically damped spring. The distance is the event's own delta, so a notch
 * travels exactly as far as it does natively; only the movement is spread over
 * more frames. Returns the function that hands scrolling back to the browser.
 */
export function smoothWheel(): () => void {
  let run: {
    el: Element;
    target: number;
    /** Unrounded position, because `scrollTop` reads back rounded. */
    at: number;
    velocity: number;
    /** What `scrollTop` read after the last write. */
    written: number;
    lastFrame: number;
    frame: number;
  } | null = null;

  function step(now: number) {
    const r = run!;
    // Something else moved it: the scrollbar, a jump to the playing song, a
    // reloaded list. That wins over what was left of the notch.
    if (Math.abs(r.el.scrollTop - r.written) > 1) {
      run = null;
      return;
    }
    const dt = r.lastFrame ? now - r.lastFrame : 1000 / 60;
    r.lastFrame = now;
    // The exact solution of the spring, so a slow frame cannot destabilize it.
    const decay = Math.exp(-SPRING * dt);
    const offset = r.at - r.target;
    const c = r.velocity + SPRING * offset;
    r.at = r.target + (offset + c * dt) * decay;
    r.velocity = (r.velocity - SPRING * c * dt) * decay;

    const settled = Math.abs(r.at - r.target) < 0.5 && Math.abs(r.velocity) < 0.05;
    r.el.scrollTop = settled ? r.target : r.at;
    r.written = r.el.scrollTop;
    if (settled) run = null;
    else r.frame = requestAnimationFrame(step);
  }

  function onWheel(e: WheelEvent) {
    // Already handled (the volume button), zooming, or horizontal scrolling.
    if (e.defaultPrevented || e.ctrlKey || e.shiftKey) return;
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY) || reducedMotion()) return;
    const px = e.deltaMode === WheelEvent.DOM_DELTA_PIXEL ? e.deltaY : e.deltaY * NOTCH_PX;
    // Small deltas join a running scroll, so that the tail of a fast touchpad
    // flick is not cut short; otherwise the browser scrolls them as they come.
    if (Math.abs(px) < NOTCH_PX && !run) return;
    const el = scrollerFor(e.target as Element, Math.sign(px));
    if (!el) return;
    e.preventDefault();
    if (run?.el !== el) {
      if (run) cancelAnimationFrame(run.frame);
      const at = el.scrollTop;
      run = {
        el,
        target: at,
        at,
        velocity: 0,
        written: at,
        lastFrame: 0,
        frame: requestAnimationFrame(step),
      };
    }
    run.target = Math.max(0, Math.min(run.target + px, el.scrollHeight - el.clientHeight));
  }

  window.addEventListener("wheel", onWheel, { passive: false });
  return () => {
    window.removeEventListener("wheel", onWheel);
    if (run) cancelAnimationFrame(run.frame);
    run = null;
  };
}
