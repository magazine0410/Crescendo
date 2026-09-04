<script lang="ts">
  import { library, SORT_KEYS } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { playlists } from "$lib/stores/playlists.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { call, type FileDragSource, type PlayContext, type SortKey, type Track } from "$lib/api";
  import { fileDrag, FILE_DRAG_LIMIT } from "$lib/fileDrag";
  import Icon from "../common/Icon.svelte";
  import { formatTime, formatTitle, displayTitle, displayArtist, displayAlbumartist, displayName, playsText } from "$lib/format";
  import { openContextMenu, type MenuItem } from "../common/ContextMenu.svelte";
  import { addToPlaylistItem, openTrackInFileManagerItem } from "$lib/menu";
  import VirtualList from "$lib/virtual/VirtualList.svelte";
  import { tick, untrack } from "svelte";

  let list = $state<VirtualList | null>(null);

  // Rows are two-line so the play counter can sit under the title (and the
  // optional second detail under the first). Uniform height keeps VirtualList on
  // its fast constant-height path.
  const ROW_H = 38;

  // Recount + reset whenever the filter (mode / value / query), the sort, the
  // library (after a rescan), or the separators toggle changes. reloadTrackView
  // bumps library.trackEpoch, which the range effect below watches to re-fetch.
  $effect(() => {
    library.mode;
    library.selectedValues;
    library.query;
    library.sort;
    library.stats;
    // Projection transitions reconstruct this view explicitly before they
    // re-enable interaction. Read the transition flag without subscribing to
    // it so the false edge does not immediately trigger a duplicate reload.
    if (!untrack(() => library.projectionChanging)) {
      library.reloadTrackView(prefs.values.track_separators);
    }
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

  // "Jump to now playing song". Unlike the intent above this one is watched
  // live, so it also lands while the tab is already open; the trackEpoch read
  // makes it wait for the first loaded view after the tab is opened again.
  $effect(() => {
    library.trackEpoch;
    if (!library.pendingJump || library.trackCount === 0) return;
    const id = player.track?.id;
    untrack(() => (library.pendingJump = null));
    if (id != null) void revealTrack(id);
  });

  async function revealTrack(id: number) {
    const idx = await library.indexOfTrack(id);
    if (idx == null) return;
    const row = library.rowForTrackIndex(idx);
    await tick(); // let VirtualList render with the new row count first
    list?.scrollToIndex(row);
  }

  /** Play order matches the visible track order (both use the same sort), so
      the track index is the start index. A left-panel selection narrows to that
      value. */
  function playContext(): PlayContext {
    return (
      library.selectedValues.length > 0
        ? {
            type: "value_tracks",
            mode: library.wireMode,
            values: library.selectedValues,
            query: library.query,
            projection_revision: library.projectionRevision,
            sort: library.sort,
          }
        : {
            type: "library_view",
            query: library.query,
            projection_revision: library.projectionRevision,
            sort: library.sort,
          }
    );
  }

  async function playAt(trackIndex: number) {
    if (library.projectionChanging) return;
    const trackId = library.tracks[trackIndex]?.id;
    if (trackId == null) {
      library.notice = "Library view changed; try again";
      return;
    }
    try {
      await player.playContext(playContext(), trackIndex);
    } catch (reason) {
      if (!(await library.recoverStaleProjection(reason))) throw reason;
      const newIndex = await library.indexOfTrack(trackId);
      if (newIndex == null) {
        library.notice = "Library view changed; try again";
        return;
      }
      try {
        await player.playContext(playContext(), newIndex);
      } catch {
        library.notice = "Library view changed; try again";
      }
    }
  }

  function onRowClick(e: MouseEvent, trackIndex: number) {
    if (library.projectionChanging) return;
    library.selectTrack(trackIndex, {
      ctrl: e.ctrlKey || e.metaKey,
      shift: e.shiftKey,
    });
  }

  async function trackDragSource(trackIndex: number, track: Track): Promise<FileDragSource> {
    if (!library.isSelected(trackIndex)) return { type: "track_ids", ids: [track.id] };
    const selected = await library.collectSelectedTracks(FILE_DRAG_LIMIT + 1);
    return { type: "track_ids", ids: selected.map((item) => item.id) };
  }

  async function trackMenu(e: MouseEvent, trackIndex: number, clickedTrack: Track) {
    e.preventDefault();
    e.stopPropagation();
    if (library.projectionChanging) return;
    // Right-clicking a row outside the current selection selects just that row.
    if (!library.isSelected(trackIndex)) library.selectTrack(trackIndex, {});

    const sel = await library.collectSelectedTracks();
    if (sel.length === 0) return;
    const ids = sel.map((t) => t.id);

    // "View album" only when the whole selection shares one album.
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
        label: "View album",
        disabled: !sameAlbum,
        action: sameAlbum ? () => library.openAlbumById(albumId) : undefined,
      },
      openTrackInFileManagerItem(clickedTrack),
      { separator: true, label: "" },
      addToPlaylistItem((pid) => playlists.addTracks(pid, ids), { memberOf }),
    ];
    openContextMenu(e, items);
  }
</script>

<section class="view">
  <header class="viewheader">
    <span class="title">Library {#if library.query}(Filtered){/if}</span>
    <div class="sortctl">
      <span class="sortlabel">Sort:</span>
      <select
        class="sortsel"
        title="Sort by"
        disabled={library.projectionChanging}
        value={library.sort?.key ?? ""}
        onchange={(e) => library.setSortKey((e.currentTarget.value || null) as SortKey | null)}
      >
        <option value="">Default</option>
        {#each SORT_KEYS as [key, label] (key)}
          <option value={key}>{label}</option>
        {/each}
      </select>
      <button
        class="dirbtn"
        disabled={library.projectionChanging || !library.sort}
        title={library.sort?.descending ? "Descending" : "Ascending"}
        onclick={() => library.toggleSortDirection()}
      >
        <Icon name={library.sort?.descending ? "triangle-down" : "triangle-up"} size={9} />
      </button>
      <select
        class="sortsel"
        title="Then by"
        disabled={library.projectionChanging || !library.sort}
        value={library.sort?.secondary ?? ""}
        onchange={(e) =>
          library.setSortSecondary((e.currentTarget.value || null) as SortKey | null)}
      >
        <option value="">Off</option>
        {#each SORT_KEYS as [key, label] (key)}
          {#if key !== library.sort?.key}
            <option value={key}>{label}</option>
          {/if}
        {/each}
      </select>
      <button
        class="dirbtn"
        class:following={library.sort?.secondary_descending == null}
        disabled={library.projectionChanging || !library.sort?.secondary}
        title={library.sort?.secondary_descending == null
          ? "Following the primary direction — click to set separately"
          : library.sortSecondaryDescending
            ? "Descending"
            : "Ascending"}
        onclick={() => library.toggleSortSecondaryDirection()}
      >
        <Icon
          name={library.sortSecondaryDescending ? "triangle-down" : "triangle-up"}
          size={9}
        />
      </button>
    </div>
    <span class="items">{library.trackCount.toLocaleString()} tracks</span>
  </header>
  {#if library.trackCount === 0}
    <div class="empty">No tracks</div>
  {:else}
    <VirtualList
      bind:this={list}
      count={library.trackListRowCount}
      itemHeight={ROW_H}
      initialScrollTop={library.trackScroll}
      scrollResetKey={library.scrollResetKey}
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
        use:fileDrag={{
          source: () => trackDragSource(cell.trackIndex, t),
          disabled: () => library.projectionChanging,
        }}
        class:alt
        class:selected={library.isSelected(cell.trackIndex)}
        class:playing={player.track?.id === t.id}
        disabled={library.projectionChanging}
        onclick={(e) => onRowClick(e, cell.trackIndex)}
        ondblclick={() => playAt(cell.trackIndex)}
        oncontextmenu={(e) => trackMenu(e, cell.trackIndex, t)}
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
        use:fileDrag={{
          source: () =>
            cell.albumId == null
              ? null
              : { type: "context", ctx: { type: "album", id: cell.albumId } },
          disabled: () => library.projectionChanging,
        }}
        disabled={library.projectionChanging}
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
    padding-bottom: var(--bottombar-h);
  }
  .viewheader {
    display: flex;
    align-items: center;
    gap: 12px;
    height: var(--header-h);
    padding: 0 16px;
    border-bottom: 1px solid var(--hairline);
    flex: none;
  }
  .title {
    font-style: italic;
    font-weight: 350;
    font-size: 14px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .items {
    color: var(--txt-faded);
    font-size: 11px;
    flex: none;
  }
  /* sort controls, sitting left of the track count */
  .sortctl {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
    flex: none;
  }
  .sortlabel {
    color: var(--txt-faded);
    font-size: 11px;
  }
  .sortsel {
    background-color: var(--bg-panel);
    border: 1px solid var(--border);
    color: var(--txt);
    font-size: 11px;
    padding: 2px 18px 2px 6px;
    /* WebKitGTK renders a native (light) combobox unless appearance is cleared */
    appearance: none;
    -webkit-appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='10' viewBox='0 0 24 24' fill='none' stroke='%236e6e6e' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 5px center;
    cursor: pointer;
  }
  .sortsel:disabled {
    color: var(--txt-faded);
    cursor: default;
  }
  .sortsel option {
    background: var(--bg-main);
    color: var(--txt);
  }
  .dirbtn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--txt);
  }
  .dirbtn:hover:not(:disabled) {
    color: var(--txt-full);
  }
  .dirbtn:disabled {
    color: var(--txt-faded);
    cursor: default;
  }
  /* secondary direction still mirroring the primary (not set on its own) */
  .dirbtn.following:not(:disabled):not(:hover) {
    color: var(--txt-semifaded);
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
  .trow.selected .no,
  .trow.selected .tartist,
  .trow.selected .tplays,
  .trow.selected .tdetail,
  .trow.selected .tdetail2,
  .trow.selected .dur {
    color: var(--txt-superfaded);
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
