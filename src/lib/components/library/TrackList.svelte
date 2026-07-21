<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { playlists } from "$lib/stores/playlists.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import type { PlayContext, Track } from "$lib/api";
  import { formatTime, formatTitle, displayTitle, displayArtist, displayAlbumartist, displayName } from "$lib/format";
  import { openContextMenu, type MenuItem } from "../common/ContextMenu.svelte";
  import { addToPlaylistItem } from "$lib/menu";
  import VirtualList from "$lib/virtual/VirtualList.svelte";

  const ROW_H = 26;

  // Recount + reset whenever the filter (mode / value / query), the library (after
  // a rescan), or the separators toggle changes. reloadTrackView bumps
  // library.trackEpoch, which the range effect below watches to re-fetch.
  $effect(() => {
    library.mode;
    library.selectedValue;
    library.query;
    library.stats;
    library.reloadTrackView(prefs.values.track_separators);
  });

  // Fetch the pages covering the visible (flat) row range, on reset or scroll.
  let visFirst = $state(0);
  let visLast = $state(0);
  $effect(() => {
    library.trackEpoch;
    library.ensureRowRange(visFirst, visLast);
  });

  /** Play order matches the visible track order (both use LIBRARY_SORT), so the
      track index is the start index. A left-panel selection narrows to that value. */
  function playAt(trackIndex: number) {
    const ctx: PlayContext =
      library.selectedValue != null
        ? {
            type: "value_tracks",
            mode: library.mode,
            value: library.selectedValue,
            query: library.query,
            collapse: library.groupingCollapse,
          }
        : { type: "library_view", query: library.query, sort: null };
    player.playContext(ctx, trackIndex);
  }

  function trackMenu(e: MouseEvent, t: Track) {
    const items: MenuItem[] = [
      { label: "Add to queue", action: () => queue.add([t.id]) },
    ];
    if (t.album_id != null) {
      const albumId = t.album_id;
      items.push({ label: "Open album", action: () => library.openAlbumById(albumId) });
    }
    items.push(
      { separator: true, label: "" },
      addToPlaylistItem((pid) => playlists.addTracks(pid, [t.id])),
    );
    openContextMenu(e, items);
  }
</script>

<section class="view">
  <header class="viewheader">
    <span class="title">Library {#if library.query}(Filtered){/if}</span>
    <span class="items">{library.trackCount.toLocaleString()} tracks</span>
  </header>
  {#if library.trackCount === 0}
    <div class="empty">No tracks</div>
  {:else}
    <VirtualList
      count={library.trackListRowCount}
      itemHeight={ROW_H}
      padBottom={ui.bottombarH}
      onVisibleRange={(f, l) => {
        visFirst = f;
        visLast = l;
      }}
      row={rowSnippet}
    />
  {/if}
</section>

{#snippet rowSnippet(i: number)}
  {@const cell = library.rowAt(i)}
  {#if "trackIndex" in cell}
    {@const t = library.tracks[cell.trackIndex]}
    {@const alt = cell.trackIndex % 2 === 1}
    {#if t}
      {@const detail = formatTitle(prefs.values.track_detail_format, t)}
      <button
        class="trow"
        class:alt
        class:playing={player.track?.id === t.id}
        ondblclick={() => playAt(cell.trackIndex)}
        oncontextmenu={(e) => trackMenu(e, t)}
      >
        <span class="no">{t.track_no ?? cell.trackIndex + 1}</span>
        <span class="ttitle">{displayTitle(t)}</span>
        {#if prefs.values.track_separators}
          {#if t.artist && t.artist !== (t.albumartist ?? t.artist)}
            <span class="tartist">- {displayArtist(t)}</span>
          {/if}
        {:else if t.albumartist ?? t.artist}
          <span class="tartist">- {displayAlbumartist(t)}</span>
        {/if}
        <span class="tdetail" title={detail}>{detail}</span>
        <span class="dur">{formatTime(t.duration_ms)}</span>
      </button>
    {:else}
      <div class="trow placeholder" class:alt></div>
    {/if}
  {:else}
    <div class="sep">
      <button
        class="sepname"
        title={cell.sep.album ?? "?"}
        onclick={() => cell.albumId != null && library.openAlbumById(cell.albumId)}
      >
        {cell.sep.album ?? "?"}
      </button>
      <span class="separtist">{displayName(cell.sep.artist, cell.sep.artist_sort) ?? "?"}</span>
      {#if cell.sep.year}<span class="sepyear">{cell.sep.year}</span>{/if}
    </div>
  {/if}
{/snippet}

<style>
  .view {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .viewheader {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding: 10px 16px;
    border-bottom: 1px solid var(--hairline);
    flex: none;
  }
  .title {
    font-style: italic;
    font-weight: 350;
    font-size: 14px;
  }
  .items {
    color: var(--txt-faded);
    font-size: 11px;
  }
  .empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--txt-faded);
  }
  /* album separator */
  .sep {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 100%;
    padding: 0 16px 0 12px;
    background: var(--bg-panel);
    border-top: 1px solid var(--hairline);
  }
  .sepname {
    font-style: italic;
    font-weight: 500;
    color: var(--txt);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 60%;
    text-align: left;
  }
  .sepname:hover {
    color: var(--txt-full);
    text-decoration: underline;
  }
  .separtist {
    color: var(--txt-faded);
    font-size: 11px;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sepyear {
    margin-left: auto;
    color: var(--txt-faded);
    font-size: 11px;
    flex: none;
  }
  /* track row */
  .trow {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 100%;
    padding: 0 16px 0 12px;
    color: var(--txt);
    text-align: left;
  }
  .trow.alt {
    background: var(--row-alt);
  }
  .trow:hover:not(.placeholder) {
    background: var(--hover-row);
  }
  .trow.playing {
    color: var(--accent);
  }
  .no {
    width: 30px;
    text-align: right;
    color: var(--txt-faded);
    font-style: italic;
    flex: none;
  }
  .trow.playing .no {
    color: var(--accent);
  }
  .ttitle {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tartist {
    color: var(--txt-faded);
    font-style: italic;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tdetail {
    margin-left: auto;
    max-width: 45%;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--txt-faded);
  }
  .dur {
    color: var(--txt-faded);
    flex: none;
  }
</style>
