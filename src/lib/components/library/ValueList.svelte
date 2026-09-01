<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { call, type Track } from "$lib/api";
  import { openContextMenu } from "../common/ContextMenu.svelte";
  import VirtualList from "$lib/virtual/VirtualList.svelte";
  import {
    displayName,
    durationBucketLabel,
    lastPlayedBucketLabel,
    playCountLabel,
    LAST_PLAYED_SPANS,
  } from "$lib/format";

  // row 0 = "All", rows 1.. = values (narrowed by the filter bar)
  const count = $derived(library.filteredValues.length + 1);

  function label(v: string): string {
    if (library.isDurationMode) return durationBucketLabel(Number(v));
    if (library.isLastPlayedMode) return lastPlayedBucketLabel(Number(v));
    if (library.isPlayCountMode) return playCountLabel(Number(v));
    return v === "" ? "?" : v;
  }

  /** The span a last-played bucket covers, shown as the row's tooltip. */
  function rowTitle(v: string, sort: string | null): string | undefined {
    if (library.isLastPlayedMode) return LAST_PLAYED_SPANS[Number(v)];
    return sort && sort !== v ? v : undefined;
  }

  async function playValue(value: string) {
    if (library.projectionChanging) return;
    try {
      await player.playContext(
        {
          type: "value_tracks",
          mode: library.wireMode,
          values: [value],
          query: library.query,
          projection_revision: library.projectionRevision,
          sort: library.sort,
        },
        0,
      );
    } catch (reason) {
      if (await library.recoverStaleProjection(reason)) {
        library.projectionNotice = "Library view changed; try again";
        return;
      }
      throw reason;
    }
  }

  async function queueValue(value: string) {
    if (library.projectionChanging) return;
    const ids = await valueTrackIds(value);
    await queue.add(ids);
  }

  /** Plain click selects only this value; Ctrl (or Cmd) toggles it in/out of a
      multi-selection; Shift selects the visible range from the last plainly
      clicked/toggled value, so several values can show together. */
  function onRowClick(e: MouseEvent, value: string) {
    if (e.shiftKey) library.selectValueRange(value);
    else if (e.ctrlKey || e.metaKey) library.toggleValue(value);
    else library.selectValue(value);
  }

  async function valueTrackIds(value: string): Promise<number[]> {
    // Reuse browse_albums + album_tracks would be N calls; ask for the whole
    // value's track list in one range call is not filtered by value, so go
    // through albums for now (values rarely exceed a few hundred albums).
    const albums = await call<{ id: number }[]>("browse_albums", {
      mode: library.wireMode,
      values: [value],
      query: library.query,
      projectionRevision: library.projectionRevision,
    });
    const lists = await Promise.all(
      albums.map((a) => call<Track[]>("album_tracks", { albumId: a.id })),
    );
    return lists.flat().map((t) => t.id);
  }

  function onRowContext(e: MouseEvent, value: string) {
    if (library.projectionChanging) return;
    openContextMenu(e, [
      { label: "Play", action: () => playValue(value) },
      { label: "Add to queue", action: () => queueValue(value) },
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
      class:selected={library.selectedValues.length === 0}
      disabled={library.projectionChanging}
      onclick={() => library.selectValue(null)}
    >
      <span class="name">All ({library.values.length}
        {library.mode === "genre" ? "genres"
        : library.mode === "artist" ? "artists"
        : library.mode === "album" ? "albums"
        : library.mode === "albumartist" ? "album artists"
        : library.isGroupingMode ? "groups"
        : library.isDurationMode ? "durations"
        : library.isLastPlayedMode ? "spans"
        : library.isPlayCountMode ? "play counts" : "values"})</span>
    </button>
  {:else}
    {@const item = library.filteredValues[i - 1]}
    <button
      class="row"
      class:selected={library.selectedValues.includes(item.value)}
      disabled={library.projectionChanging}
      ondblclick={() => playValue(item.value)}
      oncontextmenu={(e) => onRowContext(e, item.value)}
      onclick={(e) => onRowClick(e, item.value)}
      title={rowTitle(item.value, item.sort_value)}
    >
      <span class="name">{displayName(label(item.value), item.sort_value) ?? "?"}</span>
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
    /* Stop the scroll viewport at the top edge of the bottom bar (as the track
       list and the album wall do), so the scroll thumb never goes under it. */
    padding-bottom: var(--bottombar-h);
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
