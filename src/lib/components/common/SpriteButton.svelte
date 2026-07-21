<script lang="ts">
  import Icon from "./Icon.svelte";

  interface Props {
    /** Eole glyph name (mapped to Lucide in icons.ts), e.g. "prev", "play". */
    icon: string;
    /** Button hit-area size in px. The glyph is drawn a little smaller for padding. */
    size?: number;
    title?: string;
    disabled?: boolean;
    /** Small badge drawn over the bottom-right corner, e.g. a state indicator. */
    overlay?: string;
    onclick?: (e: MouseEvent) => void;
    onwheel?: (e: WheelEvent) => void;
    oncontextmenu?: (e: MouseEvent) => void;
  }
  let {
    icon,
    size = 30,
    title = "",
    disabled = false,
    overlay,
    onclick,
    onwheel,
    oncontextmenu,
  }: Props = $props();

  const glyph = $derived(Math.round(size * 0.72));
  const badge = $derived(Math.round(size * 0.44));
</script>

<button
  class="sprite"
  class:disabled
  style:width={`${size}px`}
  style:height={`${size}px`}
  {title}
  aria-label={title}
  {disabled}
  {onclick}
  {onwheel}
  {oncontextmenu}
>
  <Icon name={icon} size={glyph} />
  {#if overlay}
    <span class="overlay"><Icon name={overlay} size={badge} /></span>
  {/if}
</button>

<style>
  .sprite {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
    border-radius: 6px;
    /* Eole glyphs were white; keep near-white at rest, full white on hover. */
    color: var(--txt);
    transition:
      color 0.1s,
      background-color 0.1s;
  }
  .sprite:hover:not(.disabled) {
    color: var(--txt-full);
    background-color: var(--hover-row);
  }
  .sprite.disabled {
    opacity: 0.35;
    cursor: default;
  }
  /* Corner state badge: accent-colored, ringed by the bar bg so it reads
     clearly over the glyph behind it. */
  .overlay {
    position: absolute;
    right: -1px;
    bottom: -1px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    color: var(--accent);
    background: var(--bg-headerbar);
    box-shadow: 0 0 0 1.5px var(--bg-headerbar);
    pointer-events: none;
  }
</style>
