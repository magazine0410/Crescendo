<script lang="ts">
  import { prefs } from "$lib/stores/prefs.svelte";
  import Icon from "./Icon.svelte";

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
    /** A collapsed panel keeps this divider only for its restore button. */
    collapsed?: boolean;
    /** Accessible identity for the panel controlled by the optional button. */
    panelId?: string;
    panelLabel?: string;
    animate?: boolean;
    onresize: (px: number) => void;
    ontoggle?: () => void;
    ondragchange?: (dragging: boolean) => void;
  }
  let {
    direction = "horizontal",
    size,
    min = 120,
    max = 600,
    invert = false,
    persistKey,
    collapsed = false,
    panelId,
    panelLabel = "panel",
    animate = true,
    onresize,
    ontoggle,
    ondragchange,
  }: Props = $props();

  let dragging = $state(false);
  let startPos = 0;
  let startSize = 0;

  const toggleIcon = $derived(
    invert
      ? collapsed
        ? "arrow-left-to-line"
        : "arrow-right-to-line"
      : collapsed
        ? "arrow-right-to-line"
        : "arrow-left-to-line",
  );

  function onDown(e: PointerEvent) {
    if (collapsed || e.button !== 0) return;
    dragging = true;
    ondragchange?.(true);
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
    ondragchange?.(false);
    if (persistKey) void prefs.setLayout(persistKey, size);
  }
</script>

<div
  class="splitter {direction}"
  class:collapsed
  class:dragging
  class:animate
  role="separator"
  aria-orientation={direction === "horizontal" ? "vertical" : "horizontal"}
  onpointerdown={onDown}
  onpointermove={onMove}
  onpointerup={onUp}
  onpointercancel={onUp}
>
  <span class="drag-highlight" aria-hidden="true"></span>
  {#if ontoggle}
    <button
      class="toggle"
      class:invert
      title={`${collapsed ? "Show" : "Hide"} ${panelLabel}`}
      aria-label={`${collapsed ? "Show" : "Hide"} ${panelLabel}`}
      aria-controls={panelId}
      aria-expanded={!collapsed}
      onpointerdown={(e) => e.stopPropagation()}
      onclick={(e) => {
        e.stopPropagation();
        ontoggle();
      }}
    >
      <Icon name={toggleIcon} size={11} />
    </button>
  {/if}
</div>

<style>
  .splitter {
    flex: none;
    position: relative;
    z-index: 10;
  }
  .splitter.horizontal {
    width: 5px;
    margin: 0 -2px;
  }
  .splitter.vertical {
    height: 5px;
    margin: -2px 0;
  }
  .drag-highlight {
    position: absolute;
    inset: 0;
  }
  .horizontal .drag-highlight {
    cursor: ew-resize;
  }
  .vertical .drag-highlight {
    cursor: ns-resize;
  }
  .drag-highlight:hover,
  .splitter.dragging .drag-highlight {
    background: var(--selection);
    opacity: 0.5;
  }
  .splitter.collapsed .drag-highlight {
    pointer-events: none;
  }
  .toggle {
    position: absolute;
    top: 50%;
    left: 50%;
    z-index: 2;
    width: 16px;
    height: 32px;
    transform: translate(-100%, -50%);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--txt-faded);
    background: var(--bg-panel);
    border: 1px solid var(--sidesline);
    border-radius: 3px 0 0 3px;
    box-shadow: 0 0 5px rgba(0, 0, 0, 0.25);
  }
  .splitter.horizontal.animate .toggle {
    transition: transform 180ms cubic-bezier(0.2, 0, 0, 1);
  }
  /* Expanded controls live inside their panel with the inward-facing edge
     attached to the divider. Once collapsed, move the control across the
     divider so the zero-width panel cannot hide it. */
  .toggle.invert {
    transform: translate(0, -50%);
    border-radius: 0 3px 3px 0;
  }
  .splitter.collapsed .toggle:not(.invert) {
    transform: translate(0, -50%);
    border-radius: 0 3px 3px 0;
  }
  .splitter.collapsed .toggle.invert {
    transform: translate(-100%, -50%);
    border-radius: 3px 0 0 3px;
  }
  .toggle:hover,
  .toggle:focus-visible {
    color: var(--txt-full);
    background: var(--hover-row);
  }

  @media (prefers-reduced-motion: reduce) {
    .splitter.horizontal.animate .toggle {
      transition-duration: 0s;
    }
  }
</style>
