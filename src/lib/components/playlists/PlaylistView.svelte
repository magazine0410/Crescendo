<script lang="ts">
  import { playlists } from "$lib/stores/playlists.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { artUrl, type PlayContext, type Track } from "$lib/api";
  import { COVER_MISSING, COVER_NONE } from "$lib/icons";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { formatTime, formatTitle, displayTitle, displayArtist, displayName, playsText } from "$lib/format";
  import { openContextMenu } from "../common/ContextMenu.svelte";
  import Icon from "../common/Icon.svelte";
  import VirtualList from "$lib/virtual/VirtualList.svelte";
  import { openTrackInFileManagerItem, tagEditItem } from "$lib/menu";
  import { viewAlbum } from "$lib/navigate";
  import { tick, untrack } from "svelte";
  import { fileDrag, FILE_DRAG_LIMIT } from "$lib/fileDrag";

  let list = $state<VirtualList | null>(null);
  let selectedIds = $state<number[]>([]);
  let selectionAnchor = $state<number | null>(null);

  const HEADER_H = 84;
  // Two-line track rows so the play counter can sit under the title.
  const TRACK_H = 38;
  // Playlist band, drawn only when several playlists are selected at once.
  const PLAYLIST_H = 30;

  interface HeaderRow {
    type: "header";
    albumId: number | null;
    album: string | null;
    artist: string | null;
    artistSort: string | null;
    year: number | null;
    durationMs: number;
    count: number;
    trackIds: number[];
  }
  interface PlaylistRow {
    type: "playlist";
    playlistId: number;
    name: string;
    durationMs: number;
    count: number;
  }
  interface TrackRow {
    type: "track";
    track: Track;
    /** Place in the unfiltered combined list of every selected playlist. */
    index: number;
    /** Playlist this row came from, and its position inside that playlist.
        Removal needs these: with several playlists selected they are no
        longer the same as `index`. */
    playlistId: number;
    position: number;
    /** Contiguous ordinal among the tracks currently visible after filtering.
        Row striping follows this instead of the playback-context index. */
    visibleIndex: number;
  }
  type Row = PlaylistRow | HeaderRow | TrackRow;
  type PlaylistEntry = Omit<TrackRow, "type" | "visibleIndex"> & { searchBlob: string };

  /** Every selected playlist's tracks, back to back, each row carrying both
      of its numbers. */
  const entries = $derived.by(() => {
    const out: PlaylistEntry[] = [];
    let index = 0;
    for (const g of playlists.groups) {
      g.tracks.forEach((track, position) => {
        const searchBlob = [track.title, track.artist, track.album, track.albumartist, track.genre]
          .filter((field): field is string => Boolean(field))
          .join("\u001f")
          .toLowerCase();
        out.push({ track, index: index++, playlistId: g.playlist_id, position, searchBlob });
      });
    }
    return out;
  });

  const multi = $derived(playlists.renderedIds.length > 1);

  // Playlist views remain client-side, but wait for typing to settle before an
  // O(n) row rebuild. Clearing the query is immediate.
  let filterQuery = $state("");
  $effect(() => {
    const query = ui.searchQuery.trim().toLowerCase();
    if (!query) {
      filterQuery = "";
      return;
    }
    const timer = setTimeout(() => (filterQuery = query), 200);
    return () => clearTimeout(timer);
  });

  /** Filter by the active search using text pre-lowercased when tracks change. */
  const filtered = $derived.by(() => {
    const q = filterQuery;
    if (!q) return entries;
    return entries.filter(({ searchBlob }) => searchBlob.includes(q));
  });

  /** Flatten into album-grouped rows (consecutive same-album runs share a header). */
  const rows = $derived.by(() => {
    const out: Row[] = [];
    let currentPlaylist: number | null = null;
    let band: PlaylistRow | null = null;
    let currentKey: string | null = null;
    let header: HeaderRow | null = null;
    let visibleIndex = 0;
    for (const { track, index, playlistId, position } of filtered) {
      if (playlistId !== currentPlaylist) {
        currentPlaylist = playlistId;
        currentKey = null;
        band = null;
        if (multi) {
          band = {
            type: "playlist",
            playlistId,
            name: playlists.list.find((p) => p.id === playlistId)?.name ?? "—",
            durationMs: 0,
            count: 0,
          };
          out.push(band);
        }
      }
      const key = `${track.albumartist ?? track.artist ?? ""}${track.album ?? ""}`;
      if (key !== currentKey) {
        currentKey = key;
        header = {
          type: "header",
          albumId: track.album_id,
          album: track.album,
          artist: track.albumartist ?? track.artist,
          artistSort: track.albumartistsort ?? track.artistsort,
          year: track.year,
          durationMs: 0,
          count: 0,
          trackIds: [],
        };
        out.push(header);
      }
      if (header) {
        header.durationMs += track.duration_ms;
        header.count += 1;
        header.trackIds.push(track.id);
      }
      if (band) {
        band.durationMs += track.duration_ms;
        band.count += 1;
      }
      out.push({ type: "track", track, index, playlistId, position, visibleIndex: visibleIndex++ });
    }
    return out;
  });

  // Track rows are a fixed two-line height; only the header/track split varies,
  // so the function's identity changes with `rows` and VirtualList recomputes
  // its row offsets on any content change.
  const rowHeight = $derived.by(() => {
    return (i: number): number => {
      const t = rows[i]?.type;
      return t === "header" ? HEADER_H : t === "playlist" ? PLAYLIST_H : TRACK_H;
    };
  });

  /** Resolve exactly what is visible: while filtered, the backend rebuilds
      the matches and the clicked row is addressed by its visible ordinal. */
  function playlistContext(ids = playlists.renderedIds): PlayContext | null {
    if (ids.length === 0) return null;
    const query = filterQuery;
    return query
      ? ids.length === 1
        ? { type: "filtered_playlist", id: ids[0], query }
        : { type: "filtered_playlists", ids, query }
      : ids.length === 1
        ? { type: "playlist", id: ids[0] }
        : { type: "playlists", ids };
  }

  async function playAt(row: TrackRow, context = playlistContext()) {
    if (!context) return;
    try {
      await player.playContext(
        context,
        context.type.startsWith("filtered_") ? row.visibleIndex : row.index,
        row.track.id,
      );
    } catch {
      playlists.selectionError = "Playlist changed; try again";
    }
  }

  function trackMenu(e: MouseEvent, r: TrackRow) {
    if (!selectedIds.includes(r.track.id)) selectedIds = [r.track.id];
    const albumId = r.track.album_id;
    const context = playlistContext();
    openContextMenu(e, [
      { label: "Play", action: () => playAt(r, context) },
      { label: "Add to queue", action: () => queue.add([r.track.id]) },
      {
        label: "View album",
        disabled: albumId == null,
        action: albumId == null ? undefined : () => viewAlbum(albumId),
      },
      openTrackInFileManagerItem(r.track),
      tagEditItem(filtered.filter(entry => selectedIds.includes(entry.track.id)).map(entry => entry.track)),
      { separator: true, label: "" },
      {
        // Removal takes the row's position inside its own playlist, which is
        // not its index in the combined view.
        label: "Remove from playlist",
        action: () => playlists.removeTracks(r.playlistId, [r.position]),
      },
    ]);
  }

  // "Jump to now playing song". `rows` is read live, so the jump lands whether
  // it was asked for with this view on screen or from another tab, in which
  // case it happens on the first render after the tab is opened again. The
  // song is read now, not when the jump was asked for, so a long detour on the
  // Now playing tab still reveals what is playing rather than what was.
  $effect(() => {
    if (!playlists.pendingJump) return;
    const all = rows;
    const id = player.track?.id;
    untrack(() => (playlists.pendingJump = false));
    const i = all.findIndex((r) => r.type === "track" && r.track.id === id);
    if (i >= 0) void tick().then(() => list?.scrollToIndex(i));
  });

  function onImgError(e: Event) {
    (e.currentTarget as HTMLImageElement).src = COVER_MISSING;
  }

  function selectTrack(event: MouseEvent, track: Track) {
    if (event.shiftKey && selectionAnchor != null) {
      const start = filtered.findIndex(entry => entry.track.id === selectionAnchor), end = filtered.findIndex(entry => entry.track.id === track.id);
      selectedIds = start < 0 ? [track.id] : filtered.slice(Math.min(start, end), Math.max(start, end) + 1).map(entry => entry.track.id);
    } else if (event.ctrlKey || event.metaKey) {
      selectedIds = selectedIds.includes(track.id) ? selectedIds.filter(id => id !== track.id) : [...selectedIds, track.id];
      selectionAnchor = track.id;
    } else { selectedIds = [track.id]; selectionAnchor = track.id; }
  }

  const info = $derived(playlists.list.find((p) => p.id === playlists.renderedId));
</script>

<section
  class="view"
  class:droptarget={playlists.renderedId != null &&
    playlists.dropTargetId === playlists.renderedId}
  data-drop-playlist={playlists.renderedId}
>
  <header
    class="viewheader"
    use:fileDrag={{
      source: () => {
        const ctx = playlistContext();
        return ctx ? { type: "context", ctx } : null;
      },
    }}
  >
    <span class="title">
      {#if multi}
        Playlists : {playlists.renderedIds.length} selected
      {:else}
        Playlist : {info?.name ?? "—"}
      {/if}
      {#if ui.searchQuery.trim()}(Filtered){/if}
    </span>
    <span class="items">{filtered.length} items</span>
  </header>
  {#if playlists.selectionError}
    <div role="alert">{playlists.selectionError}</div>
  {/if}
  {#if rows.length === 0}
    <div class="empty">
      {playlists.renderedIds.length === 0
        ? "Select or create a playlist"
        : multi
          ? "These playlists are empty"
          : "This playlist is empty"}
    </div>
  {:else}
    <VirtualList
      bind:this={list}
      count={rows.length}
      itemHeight={rowHeight}
      initialScrollTop={playlists.viewScroll}
      scrollResetKey={playlists.viewScrollKey}
      onScrollChange={(y) => playlists.setViewScroll(y)}
      row={rowSnippet}
    />
  {/if}
</section>

{#snippet rowSnippet(i: number)}
  {@const r = rows[i]}
  {#if r.type === "playlist"}
    <div class="pband">
      <span
        class="pdrag"
        use:fileDrag={{
          source: () => {
            const ctx = playlistContext([r.playlistId]);
            return ctx ? { type: "context", ctx } : null;
          },
        }}
      >
        <span class="pname">{r.name}</span>
        <span class="pmeta">{r.count} items · {formatTime(r.durationMs)}</span>
      </span>
    </div>
  {:else if r.type === "header"}
    <div class="ah">
      <button
        class="album-link"
        use:fileDrag={{
          source: () => ({
            type: "track_ids",
            ids: r.trackIds.slice(0, FILE_DRAG_LIMIT + 1),
          }),
        }}
        disabled={r.albumId == null}
        aria-label={r.albumId != null ? `View album ${r.album ?? "Unknown album"}` : undefined}
        onclick={() => {
          if (r.albumId != null) viewAlbum(r.albumId);
        }}
      >
        <img class="acover" src={r.albumId != null ? artUrl("album", r.albumId) : COVER_NONE} alt="" loading="lazy" onerror={onImgError} />
        <div class="ainfo">
          <div class="aline1">{r.album ?? "?"}</div>
          <div class="aline2">{displayName(r.artist, r.artistSort) ?? "?"}</div>
        </div>
      </button>
      <div class="aright">
        <div>{formatTime(r.durationMs)}</div>
        {#if r.year}<div>{r.year}</div>{/if}
      </div>
    </div>
  {:else}
    {@const detail = formatTitle(prefs.values.track_detail_format, r.track)}
    {@const detail2 = formatTitle(prefs.values.track_detail_format_2, r.track)}
    <button
      class="trow"
      use:fileDrag={{ source: () => ({ type: "track_ids", ids: [r.track.id] }) }}
      class:alt={r.visibleIndex % 2 === 1}
      class:playing={player.track?.id === r.track.id}
      class:missing={r.track.missing}
      class:selected={selectedIds.includes(r.track.id)}
      onclick={(event) => selectTrack(event, r.track)}
      ondblclick={() => playAt(r)}
      oncontextmenu={(e) => trackMenu(e, r)}
    >
      <span class="no">{r.track.track_no ?? r.index + 1}</span>
      <div class="tmain">
        <div class="tline">
          {#if r.track.missing}
            <span class="warn" title="File missing"><Icon name="triangle-alert" size={13} /></span>
          {/if}
          <span class="ttitle">{displayTitle(r.track)}</span>
          {#if r.track.artist && r.track.artist !== (r.track.albumartist ?? r.track.artist)}
            <span class="tartist">- {displayArtist(r.track)}</span>
          {/if}
        </div>
        {#if playsText(r.track)}<span class="tplays">{playsText(r.track)}</span>{/if}
      </div>
      <div class="tdetails">
        <span class="tdetail" title={detail}>{detail}</span>
        {#if detail2}<span class="tdetail2" title={detail2}>{detail2}</span>{/if}
      </div>
      <span class="dur">{formatTime(r.track.duration_ms)}</span>
    </button>
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
  /* Drag from the file manager is hovering the open playlist. */
  .view.droptarget {
    box-shadow: inset 0 0 0 1px var(--selection);
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
  /* playlist band, shown only with several playlists selected */
  .pband {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 100%;
    padding: 0 16px;
    background: var(--bg-panel);
    border-top: 1px solid var(--hairline);
    border-bottom: 1px solid var(--hairline);
  }
  .pdrag {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    height: 100%;
  }
  .pname {
    font-weight: 600;
    color: var(--txt-full);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pmeta {
    margin-left: auto;
    color: var(--txt-faded);
    font-size: 10px;
    flex: none;
  }
  /* album group header */
  .ah {
    display: flex;
    gap: 14px;
    align-items: center;
    height: 100%;
    padding: 10px 16px 6px;
  }
  .album-link {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 14px;
    color: inherit;
    text-align: left;
  }
  .album-link:disabled {
    cursor: default;
  }
  .album-link:not(:disabled):hover .aline1,
  .album-link:not(:disabled):focus-visible .aline1 {
    color: var(--txt-full);
  }
  .album-link:not(:disabled):focus-visible {
    outline: 1px solid var(--selection);
    outline-offset: 3px;
  }
  .acover {
    width: 56px;
    height: 56px;
    object-fit: cover;
    flex: none;
    background: var(--bg-panel);
  }
  .ainfo {
    min-width: 0;
  }
  .aline1 {
    font-style: italic;
    font-weight: 350;
    font-size: 15px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .aline2 {
    color: var(--txt-faded);
    margin-top: 2px;
  }
  .aright {
    margin-left: auto;
    text-align: right;
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
    padding: 0 16px 0 24px;
    color: var(--txt);
    text-align: left;
  }
  .trow.alt {
    background: var(--row-alt);
  }
  .trow:hover {
    background: var(--hover-row);
  }
  .trow.playing {
    color: var(--accent);
  }
  .trow.selected { background: var(--selection); }
  .no {
    width: 26px;
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
  /* A track whose file was gone at the last scan. It stays in the playlist
     (removing it would shift every later row's play index), so mark it. */
  .trow.missing .ttitle,
  .trow.missing .tartist,
  .trow.missing .no {
    color: var(--missing);
  }
  .warn {
    display: flex;
    align-self: center;
    flex: none;
    color: var(--missing);
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
