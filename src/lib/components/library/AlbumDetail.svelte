<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { playlists } from "$lib/stores/playlists.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { artUrl } from "$lib/api";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { formatTime, formatLongDuration, formatTitle, displayName, displayTitle, displayArtist } from "$lib/format";
  import { openContextMenu } from "../common/ContextMenu.svelte";

  const album = $derived(library.selectedAlbum);

  function onImgError(e: Event) {
    (e.currentTarget as HTMLImageElement).src = "/eole/no_cover.png";
  }

  function trackMenu(e: MouseEvent, trackId: number) {
    openContextMenu(e, [
      { label: "Add to queue", action: () => queue.add([trackId]) },
      ...playlists.list.map((p) => ({
        label: `Add to playlist: ${p.name}`,
        action: () => playlists.addTracks(p.id, [trackId]),
      })),
    ]);
  }
</script>

{#if album}
  <div class="detail" style:padding-bottom={`${ui.bottombarH + 10}px`}>
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
        <button
          class="trow"
          class:playing={player.track?.id === t.id}
          ondblclick={() => player.playContext({ type: "album", id: album.id }, i)}
          oncontextmenu={(e) => trackMenu(e, t.id)}
        >
          <span class="no">{t.track_no ?? i + 1}</span>
          <span class="title">{displayTitle(t)}</span>
          {#if t.artist && t.artist !== album.albumartist}
            <span class="tartist">- {displayArtist(t)}</span>
          {/if}
          <span class="tdetail" title={detail}>{detail}</span>
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
    height: 100%;
    overflow-y: auto;
    padding: 14px 20px;
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
    height: 26px;
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
  .title {
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
