<script lang="ts">
  import type { Snippet } from "svelte";
  import VirtualList from "./VirtualList.svelte";

  interface Props {
    count: number;
    cellWidth: number;
    cellHeight: number;
    gap?: number;
    padBottom?: number;
    cell: Snippet<[number]>;
    initialScrollTop?: number;
    scrollResetKey?: number;
    onScrollChange?: (scrollTop: number) => void;
  }
  let {
    count,
    cellWidth,
    cellHeight,
    gap = 14,
    padBottom = 0,
    cell,
    initialScrollTop = 0,
    scrollResetKey = 0,
    onScrollChange,
  }: Props = $props();

  let containerW = $state(0);

  const cols = $derived(Math.max(1, Math.floor((containerW + gap) / (cellWidth + gap))));
  const rows = $derived(Math.ceil(count / cols));
</script>

<div class="grid-container" bind:clientWidth={containerW}>
  <VirtualList
    count={rows}
    itemHeight={cellHeight + gap}
    {padBottom}
    {initialScrollTop}
    {scrollResetKey}
    {onScrollChange}
    row={gridRow}
  />
</div>

{#snippet gridRow(r: number)}
  <div class="grid-row" style:gap={`${gap}px`} style:padding-left={`${gap}px`}>
    {#each { length: Math.min(cols, count - r * cols) } as _, c (r * cols + c)}
      <div style:width={`${cellWidth}px`} style:height={`${cellHeight}px`}>
        {@render cell(r * cols + c)}
      </div>
    {/each}
  </div>
{/snippet}

<style>
  .grid-container {
    height: 100%;
  }
  .grid-row {
    display: flex;
    align-items: flex-start;
  }
</style>
