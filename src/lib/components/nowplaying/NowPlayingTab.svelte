<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { call, artUrl, isTauri, type ArtistInfo, type AlbumInfo } from "$lib/api";
  import { formatTime, displayTitle, displayAlbumartist } from "$lib/format";

  let artistInfo = $state<ArtistInfo | null>(null);
  let albumInfo = $state<AlbumInfo | null>(null);
  let loadedFor = $state<string>("");

  $effect(() => {
    const t = player.track;
    if (!t) {
      artistInfo = null;
      albumInfo = null;
      loadedFor = "";
      return;
    }
    const artist = t.albumartist ?? t.artist ?? "";
    const key = `${artist}${t.album ?? ""}`;
    if (key === loadedFor) return;
    loadedFor = key;
    artistInfo = null;
    albumInfo = null;
    if (!isTauri || !artist) return;
    call<ArtistInfo | null>("lastfm_artist_info", { artist })
      .then((r) => (artistInfo = r))
      .catch(() => {});
    if (t.album) {
      call<AlbumInfo | null>("lastfm_album_info", { artist, album: t.album })
        .then((r) => (albumInfo = r))
        .catch(() => {});
    }
  });

  const track = $derived(player.track);

  function onImgError(e: Event) {
    (e.currentTarget as HTMLImageElement).src = "/eole/no_cover.png";
  }
</script>

{#if !track}
  <div class="nothing">
    <img src="/eole/controls/nothing_played.png" alt="" />
    <p>Nothing played</p>
  </div>
{:else}
  <div class="np" style:padding-bottom={`${ui.bottombarH + 16}px`}>
    <div class="left">
      <img
        class="bigcover"
        src={track.album_id != null
          ? artUrl("album", track.album_id, "full")
          : artUrl("track", track.id, "full")}
        alt="Cover art"
        onerror={onImgError}
      />
      <div class="local">
        <div class="row"><span>Album</span><b>{track.album ?? "?"}</b></div>
        {#if track.year}<div class="row"><span>Year</span><b>{track.year}</b></div>{/if}
        {#if track.genre}<div class="row"><span>Genre</span><b>{track.genre}</b></div>{/if}
        <div class="row"><span>Codec</span><b>{track.format}</b></div>
        <div class="row"><span>Length</span><b>{formatTime(track.duration_ms)}</b></div>
      </div>
    </div>
    <div class="right">
      <h1>{displayAlbumartist(track) ?? "?"}</h1>
      <h2>{displayTitle(track)}</h2>

      {#if artistInfo?.tags?.length}
        <div class="tags">
          {#each artistInfo.tags.slice(0, 6) as tag (tag)}
            <span class="tag">{tag}</span>
          {/each}
        </div>
      {/if}

      {#if artistInfo?.bio_full || artistInfo?.bio_summary}
        <section>
          <h3>About the artist</h3>
          <p class="bio">{artistInfo.bio_full ?? artistInfo.bio_summary}</p>
          {#if artistInfo.listeners}
            <p class="stats">
              {artistInfo.listeners.toLocaleString()} listeners on Last.fm
              {#if artistInfo.playcount}· {artistInfo.playcount.toLocaleString()} plays{/if}
            </p>
          {/if}
        </section>
      {/if}

      {#if albumInfo?.summary || albumInfo?.full}
        <section>
          <h3>About the album</h3>
          <p class="bio">{albumInfo.full ?? albumInfo.summary}</p>
        </section>
      {/if}

      {#if artistInfo?.similar?.length}
        <section>
          <h3>Similar artists</h3>
          <p class="similar">{artistInfo.similar.join(" · ")}</p>
        </section>
      {/if}

      {#if !artistInfo && !albumInfo}
        <p class="offline">
          No Last.fm info{isTauri ? "" : " (browser preview)"} — showing local tags.
        </p>
      {/if}
    </div>
  </div>
{/if}

<style>
  .nothing {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    color: var(--txt-faded);
  }
  .nothing img {
    height: 46px;
    opacity: 0.4;
  }
  .np {
    height: 100%;
    display: flex;
    gap: 34px;
    padding: 28px 34px;
    overflow-y: auto;
  }
  .left {
    flex: none;
    width: 320px;
  }
  .bigcover {
    width: 320px;
    height: 320px;
    object-fit: cover;
    background: var(--bg-panel);
    margin-bottom: 18px;
  }
  .local .row {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding: 4px 0;
    border-bottom: 1px solid var(--sidesline);
  }
  .local span {
    color: var(--txt-faded);
    flex: none;
  }
  .local b {
    font-weight: 400;
    text-align: right;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .right {
    flex: 1;
    min-width: 0;
    max-width: 760px;
  }
  h1 {
    font-size: 30px;
    font-weight: 350;
    font-style: italic;
    margin-bottom: 2px;
  }
  h2 {
    font-size: 15px;
    font-weight: 400;
    color: var(--txt-faded);
    margin-bottom: 14px;
  }
  .tags {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 18px;
  }
  .tag {
    border: 1px solid var(--border);
    padding: 2px 10px;
    border-radius: 2px;
    color: var(--txt-faded);
    font-size: 11px;
  }
  section {
    margin-bottom: 22px;
  }
  h3 {
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--txt-faded);
    margin-bottom: 8px;
  }
  .bio {
    line-height: 1.65;
    white-space: pre-line;
    user-select: text;
    cursor: text;
  }
  .stats {
    margin-top: 8px;
    color: var(--txt-faded);
    font-size: 11px;
  }
  .similar {
    color: var(--txt);
    line-height: 1.6;
  }
  .offline {
    color: var(--txt-faded);
    font-style: italic;
  }
</style>
