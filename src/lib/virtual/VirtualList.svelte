<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    count: number;
    /** fixed row height, or per-index for mixed-height lists */
    itemHeight: number | ((index: number) => number);
    overscan?: number;
    /** extra scrollable space at the end (e.g. bottom-bar clearance) */
    padBottom?: number;
    row: Snippet<[number]>;
    onNearEnd?: () => void;
    /** Fires whenever the visible (overscanned) index range changes — used by
        windowed lists to fetch the pages covering [first, last]. */
    onVisibleRange?: (first: number, last: number) => void;
    /** Restore the viewport to this scroll offset (px) on mount. */
    initialScrollTop?: number;
    /** Bump to explicitly re-apply `initialScrollTop`, including zero. */
    scrollResetKey?: number;
    /** Fires on every scroll with the current offset (px), for persistence. */
    onScrollChange?: (scrollTop: number) => void;
  }
  let {
    count,
    itemHeight,
    overscan = 8,
    padBottom = 0,
    row,
    onNearEnd,
    onVisibleRange,
    initialScrollTop = 0,
    scrollResetKey = 0,
    onScrollChange,
  }: Props = $props();

  let viewport: HTMLDivElement | null = $state(null);
  let scrollTop = $state(0);
  let viewportH = $state(0);

  // Restore on mount, then only when the caller explicitly bumps its reset key.
  // `initialScrollTop` itself changes on every normal scroll, so it cannot be
  // used as the reset signal without making the viewport fight the user.
  let appliedResetKey: number | null = null;
  $effect(() => {
    const target = viewport;
    const key = scrollResetKey;
    if (target && appliedResetKey !== key) {
      appliedResetKey = key;
      target.scrollTop = Math.max(0, initialScrollTop);
      scrollTop = target.scrollTop;
    }
  });

  const uniform = $derived(typeof itemHeight === "number");

  // Prefix sums for mixed-height lists: offsets[i] = y of row i, length count+1.
  const offsets = $derived.by(() => {
    if (typeof itemHeight === "number") return null;
    const fn = itemHeight;
    const arr = new Float64Array(count + 1);
    let y = 0;
    for (let i = 0; i < count; i++) {
      arr[i] = y;
      y += fn(i);
    }
    arr[count] = y;
    return arr;
  });

  const totalH = $derived.by(() => {
    if (typeof itemHeight === "number") return count * itemHeight;
    return offsets ? offsets[count] : 0;
  });

  function indexAt(y: number): number {
    if (typeof itemHeight === "number") {
      return Math.max(0, Math.min(count - 1, Math.floor(y / itemHeight)));
    }
    const off = offsets!;
    let lo = 0,
      hi = count - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (off[mid] <= y) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  const first = $derived(count === 0 ? 0 : Math.max(0, indexAt(scrollTop) - overscan));
  const last = $derived(
    count === 0 ? -1 : Math.min(count - 1, indexAt(scrollTop + viewportH) + overscan),
  );

  $effect(() => {
    if (onVisibleRange && count > 0) onVisibleRange(first, last);
  });

  function offsetOf(i: number): number {
    if (typeof itemHeight === "number") return i * itemHeight;
    return offsets![i];
  }
  function heightOf(i: number): number {
    if (typeof itemHeight === "number") return itemHeight;
    return offsets![i + 1] - offsets![i];
  }

  function onScroll() {
    if (!viewport) return;
    scrollTop = viewport.scrollTop;
    onScrollChange?.(scrollTop);
    if (onNearEnd && scrollTop + viewportH > totalH - 2000) onNearEnd();
  }

  export function scrollToIndex(i: number) {
    if (!viewport) return;
    const y = offsetOf(Math.max(0, Math.min(count - 1, i)));
    if (y < viewport.scrollTop || y > viewport.scrollTop + viewportH - 40) {
      viewport.scrollTop = Math.max(0, y - viewportH / 3);
    }
  }
</script>

<div
  class="viewport"
  bind:this={viewport}
  bind:clientHeight={viewportH}
  onscroll={onScroll}
>
  <div class="spacer" style:height={`${totalH + padBottom}px`}>
    {#each { length: Math.max(0, last - first + 1) } as _, k (first + k)}
      {@const i = first + k}
      <div
        class="vrow"
        style:transform={`translateY(${offsetOf(i)}px)`}
        style:height={`${heightOf(i)}px`}
      >
        {@render row(i)}
      </div>
    {/each}
  </div>
</div>

<style>
  .viewport {
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    position: relative;
  }
  .spacer {
    position: relative;
    width: 100%;
  }
  .vrow {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    will-change: transform;
  }
</style>
