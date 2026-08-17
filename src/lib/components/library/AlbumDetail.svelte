<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { playlists } from "$lib/stores/playlists.svelte";
  import { artUrl, type Track } from "$lib/api";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { formatTime, formatLongDuration, formatTitle, displayName, displayTitle, displayArtist, playsText } from "$lib/format";
  import { openContextMenu } from "../common/ContextMenu.svelte";
  import { addToPlaylistItem, openTrackInFileManagerItem } from "$lib/menu";

  const album = $derived(library.selectedAlbum);

  function onImgError(e: Event) {
    (e.currentTarget as HTMLImageElement).src = "/eole/no_cover.png";
  }

  function trackMenu(e: MouseEvent, track: Track) {
    openContextMenu(e, [
      { label: "Add to queue", action: () => queue.add([track.id]) },
      openTrackInFileManagerItem(track),
      { separator: true, label: "" },
      addToPlaylistItem((pid) => playlists.addTracks(pid, [track.id])),
    ]);
  }
</script>

{#if album}
  <div class="detail">
    <button class="back" onclick={() => library.selectAlbum(null)}>
      {prefs.values.library_view === "track_list" ? "‹ Back" : "‹ Albums"}
    </button>
    <header>
      <img class="cover" src={artUrl("album", album.id, "thumb")} alt="" onerror={onImgError} />
      <div class="info">
        <h1>{album.album ?? "?"}</h1>
        <div class="artist">{displayName(album.albumartist, album.albumartist_sort) ?? "?"}</div>
        <div class="stats">
          {#if album.year}{album.year} · {/if}{album.track_count} tracks ·
          {formatLongDuration(album.total_duration_ms)}
        </div>
        <div class="actions">
          <button
            class="play"
            onclick={() => player.playContext({ type: "album", id: album.id }, 0)}
          >Play</button>
          <button
            class="queue-btn"
            onclick={() => queue.add(library.albumTracks.map((t) => t.id))}
          >Add to queue</button>
        </div>
      </div>
    </header>
    <div class="tracks">
      {#each library.albumTracks as t, i (t.id)}
        {@const detail = formatTitle(prefs.values.track_detail_format, t)}
        {@const detail2 = formatTitle(prefs.values.track_detail_format_2, t)}
        <button
          class="trow"
          class:playing={player.track?.id === t.id}
          ondblclick={() => player.playContext({ type: "album", id: album.id }, i)}
          oncontextmenu={(e) => trackMenu(e, t)}
        >
          <span class="no">{t.track_no ?? i + 1}</span>
          <div class="tmain">
            <div class="tline">
              <span class="title">{displayTitle(t)}</span>
              {#if t.artist && t.artist !== album.albumartist}
                <span class="tartist">- {displayArtist(t)}</span>
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
      {/each}
    </div>
  </div>
{/if}

<style>
  .detail {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 14px 20px;
    /* Margin, not padding — and no height: 100%, so the flex row stretches this
       to the space that is left. The scroll box then ends at the top edge of
       the bottom bar and its thumb never goes under it. */
    margin-bottom: var(--bottombar-h);
  }
  .back {
    color: var(--txt-faded);
    margin-bottom: 12px;
  }
  .back:hover {
    color: var(--txt);
  }
  header {
    display: flex;
    gap: 20px;
    margin-bottom: 18px;
  }
  .cover {
    width: 168px;
    height: 168px;
    object-fit: cover;
    flex: none;
    background: var(--bg-panel);
  }
  .info h1 {
    font-weight: 350;
    font-style: italic;
    font-size: 22px;
    margin-bottom: 4px;
  }
  .artist {
    color: var(--txt);
    margin-bottom: 8px;
  }
  .stats {
    color: var(--txt-faded);
    margin-bottom: 16px;
  }
  .actions {
    display: flex;
    gap: 10px;
  }
  .actions button {
    border: 1px solid var(--border);
    padding: 5px 16px;
    color: var(--txt);
  }
  .actions button:hover {
    background: var(--hover-row);
  }
  .tracks {
    display: flex;
    flex-direction: column;
  }
  .trow {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 38px;
    padding: 0 10px;
    text-align: left;
    color: var(--txt);
  }
  .trow:nth-child(even) {
    background: var(--row-alt);
  }
  .trow:hover {
    background: var(--hover-row);
  }
  .trow.playing {
    color: var(--accent);
  }
  .no {
    width: 26px;
    color: var(--txt-faded);
    font-style: italic;
    flex: none;
    text-align: right;
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
  .title {
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
