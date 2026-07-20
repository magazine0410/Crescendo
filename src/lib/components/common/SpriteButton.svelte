<script lang="ts">
  import Icon from "./Icon.svelte";

  interface Props {
    /** Eole glyph name (mapped to Lucide in icons.ts), e.g. "prev", "play". */
    icon: string;
    /** Button hit-area size in px. The glyph is drawn a little smaller for padding. */
    size?: number;
    title?: string;
    disabled?: boolean;
    onclick?: (e: MouseEvent) => void;
    onwheel?: (e: WheelEvent) => void;
  }
  let { icon, size = 30, title = "", disabled = false, onclick, onwheel }: Props = $props();

  const glyph = $derived(Math.round(size * 0.72));
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
>
  <Icon name={icon} size={glyph} />
</button>

<style>
  .sprite {
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
</style>
