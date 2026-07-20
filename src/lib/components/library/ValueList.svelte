<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { call, type Track } from "$lib/api";
  import { openContextMenu } from "../common/ContextMenu.svelte";
  import VirtualList from "$lib/virtual/VirtualList.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { displayName } from "$lib/format";

  // row 0 = "All", rows 1.. = values
  const count = $derived(library.values.length + 1);

  function label(v: string): string {
    return v === "" ? "?" : v;
  }

  async function playValue(value: string) {
    await player.playContext(
      { type: "value_tracks", mode: library.mode, value, query: library.query },
      0,
    );
  }

  async function queueValue(value: string) {
    const ids = await valueTrackIds(value);
    await queue.add(ids);
  }

  async function valueTrackIds(value: string): Promise<number[]> {
    // Reuse browse_albums + album_tracks would be N calls; ask for the whole
    // value's track list in one range call is not filtered by value, so go
    // through albums for now (values rarely exceed a few hundred albums).
    const albums = await call<{ id: number }[]>("browse_albums", {
      mode: library.mode,
      value,
      query: library.query,
    });
    const lists = await Promise.all(
      albums.map((a) => call<Track[]>("album_tracks", { albumId: a.id })),
    );
    return lists.flat().map((t) => t.id);
  }

  function onRowContext(e: MouseEvent, value: string) {
    openContextMenu(e, [
      { label: "Play", action: () => playValue(value) },
      { label: "Add to queue", action: () => queueValue(value) },
    ]);
  }
</script>

<div class="valuelist">
  <VirtualList {count} itemHeight={26} padBottom={ui.bottombarH} row={rowSnippet} />
</div>

{#snippet rowSnippet(i: number)}
  {#if i === 0}
    <button
      class="row all"
      class:selected={library.selectedValue === null}
      onclick={() => library.selectValue(null)}
    >
      <span class="name">All ({library.values.length}
        {library.mode === "genre" ? "genres"
        : library.mode === "artist" ? "artists"
        : library.mode === "album" ? "albums" : "groups"})</span>
    </button>
  {:else}
    {@const [value, n, sort] = library.values[i - 1]}
    <button
      class="row"
      class:selected={library.selectedValue === value}
      ondblclick={() => playValue(value)}
      oncontextmenu={(e) => onRowContext(e, value)}
      onclick={() => library.selectValue(value)}
      title={sort && sort !== value ? value : undefined}
    >
      <span class="name">{displayName(label(value), sort || null) ?? "?"}</span>
      <span class="count">{n}</span>
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
