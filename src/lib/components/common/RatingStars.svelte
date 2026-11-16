<script lang="ts">
  import type { Track } from "$lib/api";
  import { lucideIcons } from "$lib/icons";
  import { ratings } from "$lib/stores/ratings.svelte";

  /** Five-star rating scale for a track row, in half-star steps. Shown
      whenever the track has a rating; an unrated track shows empty stars only
      while its row (any `.trow`) is hovered. Clicking the current rating
      clears it. */
  interface Props {
    track: Track;
    /** One star's size in px. */
    size?: number;
  }
  let { track, size = 13 }: Props = $props();

  const STARS = 5;
  const STAR = lucideIcons["star"];
  const rating = $derived(ratings.of(track));
  /** The rating under the pointer, shown in place of the saved one. */
  let preview = $state<number | null>(null);
  /** The value just clicked: no preview until the pointer leaves it, so a
      click that clears the rating shows the cleared scale. */
  let clicked: number | null = null;
  const shown = $derived(preview ?? rating ?? 0);

  function valueAt(e: MouseEvent): number {
    const box = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const x = Math.min(Math.max(e.clientX - box.left, 0), box.width);
    return Math.min(STARS, Math.max(0.5, Math.ceil((x / box.width) * STARS * 2) / 2));
  }

  function onMove(e: PointerEvent) {
    const value = valueAt(e);
    if (value === clicked) return;
    clicked = null;
    preview = value;
  }

  function onLeave() {
    clicked = null;
    preview = null;
  }

  function onClick(e: MouseEvent) {
    // The row is a button: keep the click from selecting or playing it.
    e.stopPropagation();
    // A double-click's second click would clear the rating its first set.
    if (e.detail > 1) return;
    const value = valueAt(e);
    clicked = value;
    preview = null;
    void ratings.set(track, value === rating ? null : value);
  }

  /** The row's file-drag action listens for pointerdown on the row itself, and
      Svelte handles `onpointerdown` only at the document root, after it. A
      native listener here stops the press before it reaches the row. */
  function holdPress(node: HTMLElement) {
    const stop = (e: PointerEvent) => e.stopPropagation();
    node.addEventListener("pointerdown", stop);
    return { destroy: () => node.removeEventListener("pointerdown", stop) };
  }
</script>

<!-- The row button owns keyboard focus; the stars are a pointer shortcut. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<span
  class="rating"
  class:unrated={rating == null}
  class:previewing={preview != null}
  style:--star-size="{size}px"
  title={rating == null ? "Rate" : `${rating} of 5 stars (click it again to clear)`}
  onpointermove={onMove}
  onpointerleave={onLeave}
  use:holdPress
  onclick={onClick}
  ondblclick={(e) => e.stopPropagation()}
>
  <span class="layer">
    {#each [0, 1, 2, 3, 4] as i (i)}
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"
        stroke-linejoin="round" aria-hidden="true">{@html STAR}</svg>
    {/each}
  </span>
  <span class="layer filled" style:width="{(shown / STARS) * 100}%">
    {#each [0, 1, 2, 3, 4] as i (i)}
      <svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.5"
        stroke-linejoin="round" aria-hidden="true">{@html STAR}</svg>
    {/each}
  </span>
</span>

<style>
  .rating {
    position: relative;
    display: block;
    flex: none;
    width: calc(var(--star-size) * 5);
    height: var(--star-size);
    color: var(--txt-faded);
    cursor: pointer;
  }
  /* Unrated: hidden until the row is hovered. visibility, not display, so
     the row's text does not move. */
  .rating.unrated {
    visibility: hidden;
  }
  :global(.trow:hover) .rating.unrated {
    visibility: visible;
  }
  .layer {
    position: absolute;
    inset: 0 auto 0 0;
    display: flex;
    overflow: hidden;
  }
  .layer.filled {
    color: var(--txt-semifaded);
  }
  .rating.previewing .layer.filled {
    color: var(--txt);
  }
  svg {
    display: block;
    flex: none;
    width: var(--star-size);
    height: var(--star-size);
  }
</style>
