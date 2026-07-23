<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { playlists } from "$lib/stores/playlists.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { call, type PlayContext } from "$lib/api";
  import { formatTime, formatTitle, displayTitle, displayArtist, displayAlbumartist, displayName, playsText } from "$lib/format";
  import { openContextMenu, type MenuItem } from "../common/ContextMenu.svelte";
  import { addToPlaylistItem } from "$lib/menu";
  import VirtualList from "$lib/virtual/VirtualList.svelte";
  import { tick, untrack } from "svelte";

  let list = $state<VirtualList | null>(null);

  // Rows are two-line so the play counter can sit under the title (and the
  // optional second detail under the first). Uniform height keeps VirtualList on
  // its fast constant-height path.
  const ROW_H = 38;

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

  // After a reload that carries a "scroll to now-playing" intent (set when the
  // search bar is cleared while a track plays), reveal that track's row. The
  // intent is read untracked so setting it doesn't fire this early, on the
  // still-filtered view — only the trackEpoch bump (post-reload) triggers it.
  $effect(() => {
    library.trackEpoch;
    const id = untrack(() => library.scrollToTrackId);
    if (id == null) return;
    untrack(() => (library.scrollToTrackId = null));
    void revealTrack(id);
  });

  async function revealTrack(id: number) {
    const idx = await library.indexOfTrack(id);
    if (idx == null) return;
    const row = library.rowForTrackIndex(idx);
    await tick(); // let VirtualList render with the new row count first
    list?.scrollToIndex(row);
  }

  /** Play order matches the visible track order (both use LIBRARY_SORT), so the
      track index is the start index. A left-panel selection narrows to that value. */
  function playAt(trackIndex: number) {
    const ctx: PlayContext =
      library.selectedValue != null
        ? {
            type: "value_tracks",
            mode: library.wireMode,
            value: library.selectedValue,
            query: library.query,
            collapse: library.groupingCollapse,
          }
        : { type: "library_view", query: library.query, sort: null };
    player.playContext(ctx, trackIndex);
  }

  function onRowClick(e: MouseEvent, trackIndex: number) {
    library.selectTrack(trackIndex, {
      ctrl: e.ctrlKey || e.metaKey,
      shift: e.shiftKey,
    });
  }

  async function trackMenu(e: MouseEvent, trackIndex: number) {
    e.preventDefault();
    e.stopPropagation();
    // Right-clicking a row outside the current selection selects just that row.
    if (!library.isSelected(trackIndex)) library.selectTrack(trackIndex, {});

    const sel = await library.collectSelectedTracks();
    if (sel.length === 0) return;
    const ids = sel.map((t) => t.id);

    // "Open album" only when the whole selection shares one album.
    const albumId = sel[0].album_id;
    const sameAlbum = albumId != null && sel.every((t) => t.album_id === albumId);

    // For a single track, grey out playlists it already belongs to.
    let memberOf: Set<number> | undefined;
    if (ids.length === 1) {
      memberOf = new Set(await call<number[]>("track_playlist_ids", { trackId: ids[0] }));
    }

    const items: MenuItem[] = [
      {
        label: ids.length > 1 ? `Add ${ids.length} tracks to queue` : "Add to queue",
        action: () => queue.add(ids),
      },
      {
        label: "Open album",
        disabled: !sameAlbum,
        action: sameAlbum ? () => library.openAlbumById(albumId) : undefined,
      },
      { separator: true, label: "" },
      addToPlaylistItem((pid) => playlists.addTracks(pid, ids), { memberOf }),
    ];
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
      bind:this={list}
      count={library.trackListRowCount}
      itemHeight={ROW_H}
      padBottom={ui.bottombarH}
      initialScrollTop={library.trackScroll}
      onScrollChange={(y) => (library.trackScroll = y)}
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
      {@const detail2 = formatTitle(prefs.values.track_detail_format_2, t)}
      <button
        class="trow"
        class:alt
        class:selected={library.isSelected(cell.trackIndex)}
        class:playing={player.track?.id === t.id}
        onclick={(e) => onRowClick(e, cell.trackIndex)}
        ondblclick={() => playAt(cell.trackIndex)}
        oncontextmenu={(e) => trackMenu(e, cell.trackIndex)}
      >
        <span class="no">{t.track_no ?? cell.trackIndex + 1}</span>
        <div class="tmain">
          <div class="tline">
            <span class="ttitle">{displayTitle(t)}</span>
            {#if prefs.values.track_separators}
              {#if t.artist && t.artist !== (t.albumartist ?? t.artist)}
                <span class="tartist">- {displayArtist(t)}</span>
              {/if}
            {:else if t.albumartist ?? t.artist}
              <span class="tartist">- {displayAlbumartist(t)}</span>
            {/if}
          </div>
          {#if playsText(t)}<span class="tplays">{playsText(t)}</span>{/if}
        </div>
        <div class="tdetails">
          <span class="tdetail" title={detail}>{detail}</span>
          {#if detail2}<span class="tdetail2" title={detail2}>{detail2}</span>{/if}
        </div>
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
    align-items: center;
    justify-content: space-between;
    height: var(--header-h);
    padding: 0 16px;
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
    user-select: none;
  }
  .trow.alt {
    background: var(--row-alt);
  }
  .trow:hover:not(.placeholder) {
    background: var(--hover-row);
  }
  .trow.selected,
  .trow.selected:hover {
    background: var(--selection);
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
  /* left column: title line on top, small play counter under it */
  .tmain {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    justify-content: center;
    line-height: 1.25;
  }
  .tline {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
  }
  .tplays {
    font-size: 10px;
    color: var(--txt-faded);
  }
  .ttitle {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tartist {
    /* brighter than --txt-faded so italic strokes don't read dimmer than the
       (upright) track details. */
    color: var(--txt-semifaded);
    font-style: italic;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    /* italic glyphs slant past their advance width; without this the last
       character's tail is clipped by overflow:hidden even when it fits. */
    padding-right: 3px;
  }
  .tdetails {
    margin-left: auto;
    max-width: 45%;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    justify-content: center;
    line-height: 1.25;
  }
  .tdetail,
  .tdetail2 {
    max-width: 100%;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--txt-faded);
  }
  .tdetail2 {
    font-size: 10px;
  }
  .dur {
    color: var(--txt-faded);
    flex: none;
  }
</style>
