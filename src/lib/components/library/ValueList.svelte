<script lang="ts">
  import { library, type Facet } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { type FileDragSource, type PlayContext } from "$lib/api";
  import { fileDrag } from "$lib/fileDrag";
  import { openContextMenu } from "../common/ContextMenu.svelte";
  import { addContextToPlaylistItem } from "$lib/menu";
  import VirtualList from "$lib/virtual/VirtualList.svelte";
  import { displayName, LAST_PLAYED_SPANS } from "$lib/format";

  let { facet }: { facet: Facet } = $props();

  // row 0 = "All", rows 1.. = values (narrowed by the filter bar)
  const count = $derived(facet.filteredValues.length + 1);

  /** The span a last-played bucket covers, shown as the row's tooltip. */
  function rowTitle(v: string, sort: string | null): string | undefined {
    if (facet.isLastPlayed) return LAST_PLAYED_SPANS[Number(v)];
    return sort && sort !== v ? v : undefined;
  }

  /** One descriptor for every action on rows: these values in this category
      together with the other categories' selections, and the search, sort and
      projection revision the visible list was built from. No values is the
      "All" row. */
  function valuesContext(values: string[]): Extract<PlayContext, { type: "value_tracks" }> {
    return {
      type: "value_tracks",
      filters: library.filtersWith(facet, values),
      query: library.query,
      projection_revision: library.projectionRevision,
      sort: library.sort,
    };
  }

  function valueContext(value: string) {
    return valuesContext([value]);
  }

  function valueDragSource(value: string): FileDragSource {
    const values = facet.selectedValues.includes(value) ? facet.selectedValues : [value];
    return { type: "context", ctx: valuesContext(values) };
  }

  function allDragSource(): FileDragSource {
    return { type: "context", ctx: valuesContext([]) };
  }

  async function playValue(value: string) {
    if (library.interactionBlocked) return;
    try {
      await player.playContext(valueContext(value), 0);
    } catch (reason) {
      if (await library.recoverStaleProjection(reason)) {
        library.notice = "Library view changed; try again";
        return;
      }
      throw reason;
    }
  }

  async function queueValue(value: string) {
    if (library.interactionBlocked) return;
    try {
      await queue.addContext(valueContext(value));
    } catch (reason) {
      if (await library.recoverStaleProjection(reason)) {
        library.notice = "Library view changed; try again";
        return;
      }
      throw reason;
    }
  }

  /** Plain click selects only this value, or deselects it when it is already
      selected; Ctrl (or Cmd) toggles it in/out of a multi-selection; Shift
      selects the visible range from the last plainly clicked/toggled value, so
      several values can show together. */
  function onRowClick(e: MouseEvent, value: string) {
    // The second click of a double-click belongs to onRowDblClick; taken as a
    // click of its own it would undo what the first one selected.
    if (e.detail > 1) return;
    if (e.shiftKey) library.selectValueRange(facet, value);
    else if (e.ctrlKey || e.metaKey || facet.selectedValues.includes(value)) {
      library.toggleValue(facet, value);
    } else library.selectValue(facet, value);
  }

  /** Play the value and leave it selected, so the tracks shown are the ones
      playing whether the first click selected it or deselected it. */
  function onRowDblClick(value: string) {
    if (!facet.selectedValues.includes(value)) void library.selectValue(facet, value);
    void playValue(value);
  }

  function onRowContext(e: MouseEvent, value: string) {
    if (library.interactionBlocked) return;
    openContextMenu(e, [
      { label: "Play", action: () => playValue(value) },
      { label: "Add to queue", action: () => queueValue(value) },
      { separator: true, label: "" },
      addContextToPlaylistItem(valueContext(value)),
    ]);
  }
</script>

<div class="valuelist">
  <VirtualList {count} itemHeight={26} row={rowSnippet} />
</div>

{#snippet rowSnippet(i: number)}
  {#if i === 0}
    <button
      class="row all"
      use:fileDrag={{
        source: allDragSource,
        disabled: () => library.interactionBlocked,
      }}
      class:selected={facet.selectedValues.length === 0}
      disabled={library.interactionBlocked}
      onclick={() => library.selectValue(facet, null)}
    >
      <span class="name">All ({facet.values.length} {facet.noun})</span>
    </button>
  {:else}
    {@const item = facet.filteredValues[i - 1]}
    <button
      class="row"
      use:fileDrag={{
        source: () => valueDragSource(item.value),
        disabled: () => library.interactionBlocked,
      }}
      class:selected={facet.selectedValues.includes(item.value)}
      disabled={library.interactionBlocked}
      ondblclick={() => onRowDblClick(item.value)}
      oncontextmenu={(e) => onRowContext(e, item.value)}
      onclick={(e) => onRowClick(e, item.value)}
      title={rowTitle(item.value, item.sort_value)}
    >
      <span class="name">{displayName(facet.label(item.value), item.sort_value) ?? "?"}</span>
      <span class="count">{item.count}</span>
    </button>
  {/if}
{/snippet}

<style>
  .valuelist {
    flex: 1;
    min-height: 0;
    background: var(--bg-panel);
    border-right: 1px solid var(--sidesline);
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    height: 100%;
    padding: 0 12px;
    color: var(--txt);
    text-align: left;
    gap: 8px;
  }
  .row:nth-child(odd) {
    background: transparent;
  }
  .row:hover {
    background: var(--hover-row);
  }
  .row.selected {
    background: var(--selection);
    color: var(--txt-full);
  }
  .row.all .name {
    color: var(--txt-faded);
  }
  .row.selected.all .name {
    color: var(--txt-full);
  }
  .name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .count {
    color: var(--txt-faded);
    font-size: 10px;
    flex: none;
  }
  .row.selected .count {
    color: var(--txt-full);
  }
</style>
