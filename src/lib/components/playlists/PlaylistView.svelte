<script lang="ts">
  import { playlists } from "$lib/stores/playlists.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { artUrl, type Track } from "$lib/api";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { formatTime, formatTitle, displayTitle, displayArtist, displayName, playsText } from "$lib/format";
  import { openContextMenu } from "../common/ContextMenu.svelte";
  import VirtualList from "$lib/virtual/VirtualList.svelte";

  const HEADER_H = 84;
  // Two-line track rows so the play counter can sit under the title.
  const TRACK_H = 38;

  interface HeaderRow {
    type: "header";
    albumId: number | null;
    album: string | null;
    artist: string | null;
    artistSort: string | null;
    year: number | null;
    durationMs: number;
    count: number;
  }
  interface TrackRow {
    type: "track";
    track: Track;
    /** absolute position in the playlist (play index) */
    index: number;
  }
  type Row = HeaderRow | TrackRow;

  /** Filter by the active search (plain substring; full query filtering runs
      against the library — playlist views filter client-side). */
  const filtered = $derived.by(() => {
    const q = ui.searchQuery.trim().toLowerCase();
    const withIndex = playlists.tracks.map((track, index) => ({ track, index }));
    if (!q) return withIndex;
    return withIndex.filter(({ track }) =>
      [track.title, track.artist, track.album, track.albumartist, track.genre].some((f) =>
        f?.toLowerCase().includes(q),
      ),
    );
  });

  /** Flatten into album-grouped rows (consecutive same-album runs share a header). */
  const rows = $derived.by(() => {
    const out: Row[] = [];
    let currentKey: string | null = null;
    let header: HeaderRow | null = null;
    for (const { track, index } of filtered) {
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
        };
        out.push(header);
      }
      if (header) {
        header.durationMs += track.duration_ms;
        header.count += 1;
      }
      out.push({ type: "track", track, index });
    }
    return out;
  });

  // Track rows are a fixed two-line height; only the header/track split varies,
  // so the function's identity changes with `rows` and VirtualList recomputes
  // its row offsets on any content change.
  const rowHeight = $derived.by(() => {
    return (i: number): number => (rows[i]?.type === "header" ? HEADER_H : TRACK_H);
  });

  async function playAt(index: number) {
    if (playlists.selectedId == null) return;
    await player.playContext({ type: "playlist", id: playlists.selectedId }, index);
  }

  function trackMenu(e: MouseEvent, r: TrackRow) {
    openContextMenu(e, [
      { label: "Play", action: () => playAt(r.index) },
      { label: "Add to queue", action: () => queue.add([r.track.id]) },
      { separator: true, label: "" },
      {
        label: "Remove from playlist",
        action: () =>
          playlists.selectedId != null &&
          playlists.removeTracks(playlists.selectedId, [r.index]),
      },
    ]);
  }

  function onImgError(e: Event) {
    (e.currentTarget as HTMLImageElement).src = "/eole/no_cover.png";
  }

  const info = $derived(playlists.selected());
</script>

<section class="view">
  <header class="viewheader">
    <span class="title">
      Playlist : {info?.name ?? "—"}
      {#if ui.searchQuery.trim()}(Filtered){/if}
    </span>
    <span class="items">{filtered.length} items</span>
  </header>
  {#if rows.length === 0}
    <div class="empty">
      {info ? "This playlist is empty" : "Select or create a playlist"}
    </div>
  {:else}
    <VirtualList
      count={rows.length}
      itemHeight={rowHeight}
      padBottom={ui.bottombarH}
      row={rowSnippet}
    />
  {/if}
</section>

{#snippet rowSnippet(i: number)}
  {@const r = rows[i]}
  {#if r.type === "header"}
    <div class="ah">
      <img class="acover" src={r.albumId != null ? artUrl("album", r.albumId) : "/eole/no_cover.png"} alt="" loading="lazy" onerror={onImgError} />
      <div class="ainfo">
        <div class="aline1">{r.album ?? "?"}</div>
        <div class="aline2">{displayName(r.artist, r.artistSort) ?? "?"}</div>
      </div>
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
      class:playing={player.track?.id === r.track.id}
      ondblclick={() => playAt(r.index)}
      oncontextmenu={(e) => trackMenu(e, r)}
    >
      <span class="no">{r.track.track_no ?? r.index + 1}</span>
      <div class="tmain">
        <div class="tline">
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
  /* album group header */
  .ah {
    display: flex;
    gap: 14px;
    align-items: center;
    height: 100%;
    padding: 10px 16px 6px;
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
  .trow:hover {
    background: var(--hover-row);
  }
  .trow.playing {
    color: var(--accent);
  }
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
