<script lang="ts">
  import { prefs } from "$lib/stores/prefs.svelte";

  interface Props {
    /** which edge of the *preceding* panel this splitter resizes */
    direction?: "horizontal" | "vertical";
    /** current size of the controlled panel in px */
    size: number;
    min?: number;
    max?: number;
    /** true when the panel sits on the right/bottom of the splitter (drag inverts) */
    invert?: boolean;
    /** layout pref key for persistence */
    persistKey?: string;
    onresize: (px: number) => void;
  }
  let {
    direction = "horizontal",
    size,
    min = 120,
    max = 600,
    invert = false,
    persistKey,
    onresize,
  }: Props = $props();

  let dragging = false;
  let startPos = 0;
  let startSize = 0;

  function onDown(e: PointerEvent) {
    dragging = true;
    startPos = direction === "horizontal" ? e.clientX : e.clientY;
    startSize = size;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function onMove(e: PointerEvent) {
    if (!dragging) return;
    const pos = direction === "horizontal" ? e.clientX : e.clientY;
    let delta = pos - startPos;
    if (invert) delta = -delta;
    onresize(Math.min(max, Math.max(min, startSize + delta)));
  }
  function onUp() {
    if (!dragging) return;
    dragging = false;
    if (persistKey) prefs.setLayout(persistKey, size);
  }
</script>

<div
  class="splitter {direction}"
  role="separator"
  aria-orientation={direction === "horizontal" ? "vertical" : "horizontal"}
  onpointerdown={onDown}
  onpointermove={onMove}
  onpointerup={onUp}
></div>

<style>
  .splitter {
    flex: none;
    position: relative;
    z-index: 10;
  }
  .splitter.horizontal {
    width: 5px;
    margin: 0 -2px;
    cursor: ew-resize;
  }
  .splitter.vertical {
    height: 5px;
    margin: -2px 0;
    cursor: ns-resize;
  }
  .splitter:hover::after,
  .splitter:active::after {
    content: "";
    position: absolute;
    inset: 0;
    background: var(--selection);
    opacity: 0.5;
  }
</style>
