<script lang="ts">
  import { player } from "$lib/stores/player.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { call, artUrl, isTauri, type Album, type ArtistInfo, type AlbumInfo, type LyricsResult } from "$lib/api";
  import { formatTime, displayOriginalTitle, displayAlbumartist, latinTitleSort } from "$lib/format";

  let artistInfo = $state<ArtistInfo | null>(null);
  let albumInfo = $state<AlbumInfo | null>(null);
  let lyrics = $state<LyricsResult | null>(null);
  let lyricsLoading = $state(false);
  /** Track id and ui.lyricsEpoch the lyrics (or their absence) belong to;
      guards the async calls from refiring on every player:state event, which
      hands out a new Track object each time (volume, mute, pause, shuffle,
      …). Plain `let`, not $state: nothing renders them, and making the effect
      depend on what it writes would just cost it a second pass. */
  let lyricsFor: number | null = null;
  let lyricsEpoch = -1;
  let localAlbum = $state<Album | null>(null);
  let loadedFor = $state<string>("");
  let groupingClipped = $state(false);

  $effect(() => {
    const t = player.track;
    if (!t) {
      artistInfo = null;
      albumInfo = null;
      localAlbum = null;
      loadedFor = "";
      return;
    }
    const artist = t.albumartist ?? t.artist ?? "";
    const key = `${artist}${t.album ?? ""}${t.album_id ?? ""}`;
    if (key === loadedFor) return;
    loadedFor = key;
    artistInfo = null;
    albumInfo = null;
    localAlbum = null;
    if (t.album_id != null) {
      call<Album | null>("album_by_id", { albumId: t.album_id })
        .then((r) => {
          if (loadedFor === key) localAlbum = r;
        })
        .catch(() => {});
    }
    if (!isTauri || !artist) return;
    call<ArtistInfo | null>("lastfm_artist_info", { artist })
      .then((r) => {
        if (loadedFor === key) artistInfo = r;
      })
      .catch(() => {});
    if (t.album) {
      call<AlbumInfo | null>("lastfm_album_info", { artist, album: t.album })
        .then((r) => {
          if (loadedFor === key) albumInfo = r;
        })
        .catch(() => {});
    }
  });

  $effect(() => {
    const t = player.track;
    const epoch = ui.lyricsEpoch;
    if (!t) {
      lyrics = null;
      lyricsLoading = false;
      lyricsFor = null;
      return;
    }
    if (t.id === lyricsFor && epoch === lyricsEpoch) return;
    lyricsFor = t.id;
    lyricsEpoch = epoch;
    lyrics = null;
    lyricsLoading = true;
    followUntil = 0;
    call<LyricsResult | null>("lyrics_for_track", { trackId: t.id })
      .then((r) => {
        if (lyricsFor !== t.id || lyricsEpoch !== epoch) return;
        lyrics = r;
        lyricsLoading = false;
      })
      .catch(() => {
        if (lyricsFor === t.id && lyricsEpoch === epoch) lyricsLoading = false;
      });
  });

  /** Index of the line being sung, or -1 before the first one. */
  const activeLine = $derived.by(() => {
    const lines = lyrics?.lines;
    if (!lines?.length) return -1;
    const ms = player.positionMs;
    let lo = 0;
    let hi = lines.length - 1;
    let found = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (lines[mid].ms <= ms) {
        found = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    return found;
  });

  let lyricsBox = $state<HTMLElement | null>(null);
  /** Auto-scroll stands down until this time, so following the song never
      fights someone reading ahead. Plain `let`: only the effect below reads
      it, and it must not re-run when it changes. */
  let followUntil = 0;

  function pauseFollow() {
    followUntil = Date.now() + 6000;
  }

  // Keep the line being sung in the middle of the pane.
  $effect(() => {
    const i = activeLine;
    const box = lyricsBox;
    if (!box || i < 0 || Date.now() < followUntil) return;
    const el = box.children[i] as HTMLElement | undefined;
    if (!el) return;
    box.scrollTo({
      top: Math.max(0, el.offsetTop - box.clientHeight / 2 + el.offsetHeight / 2),
      behavior: "smooth",
    });
  });

  const track = $derived(player.track);
  const romanizedTitle = $derived(track ? latinTitleSort(track) : null);
  const originalTitle = $derived(track ? displayOriginalTitle(track) : "");

  function onImgError(e: Event) {
    (e.currentTarget as HTMLImageElement).src = "/eole/no_cover.png";
  }

  function measureGrouping(node: HTMLElement, _grouping: string) {
    let frame = 0;
    const measure = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        groupingClipped = node.scrollHeight > node.clientHeight + 1;
      });
    };
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    measure();
    return {
      update(_nextGrouping: string) {
        measure();
      },
      destroy() {
        cancelAnimationFrame(frame);
        observer.disconnect();
      },
    };
  }
</script>

{#if !track}
  <div class="nothing">
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
        {#if romanizedTitle}<div class="row"><span>Original title</span><b>{originalTitle}</b></div>{/if}
        {#if track.year}<div class="row"><span>Year</span><b>{track.year}</b></div>{/if}
        {#if track.genre}<div class="row"><span>Genre</span><b>{track.genre}</b></div>{/if}
        <div class="row"><span>Codec</span><b>{track.format}</b></div>
        <div class="row"><span>Length</span><b>{formatTime(track.duration_ms)}</b></div>
        {#if track.disc_no != null}<div class="row"><span>Disc number</span><b>{track.disc_no}</b></div>{/if}
        {#if track.track_no != null && localAlbum}
          <div class="row"><span>Track number</span><b>{track.track_no}/{localAlbum.track_count}</b></div>
        {/if}
        {#if track.grouping}
          <div class="row">
            <span>Grouping</span>
            <b class="grouping" title={track.grouping}>
              <span class="grouping-text" use:measureGrouping={track.grouping}>{track.grouping}</span>
              {#if groupingClipped}<span class="grouping-ellipsis" aria-hidden="true">…</span>{/if}
            </b>
          </div>
        {/if}
        {#if track.play_count > 0}<div class="row"><span>Play count</span><b>{track.play_count}</b></div>{/if}
      </div>
    </div>
    <div class="right">
      <h1>{displayAlbumartist(track) ?? "?"}</h1>
      <h2>{romanizedTitle ?? originalTitle}</h2>

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
    {#if lyrics}
      <div class="lyrics-pane">
        <h3>Lyrics · {lyrics.source}{lyrics.lines.length ? " · synced" : ""}</h3>
        {#if lyrics.lines.length}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="lyrics-scroll synced"
            bind:this={lyricsBox}
            onwheel={pauseFollow}
            onpointerdown={pauseFollow}
          >
            {#each lyrics.lines as line, i}
              <p class="line" class:active={i === activeLine}>{line.text}</p>
            {/each}
          </div>
        {:else}
          <div class="lyrics-scroll"><p class="lyrics">{lyrics.text}</p></div>
        {/if}
      </div>
    {:else if lyricsLoading}
      <div class="lyrics-pane">
        <h3>Lyrics</h3>
        <p class="lyrics searching">Searching…</p>
      </div>
    {/if}
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
  .np {
    height: 100%;
    display: flex;
    gap: 34px;
    padding: 28px 34px;
    overflow-y: auto;
  }
  .local,
  .right,
  .lyrics-pane {
    user-select: text;
    cursor: text;
  }
  .lyrics-pane {
    flex: none;
    width: 340px;
    min-width: 0;
    overflow-wrap: anywhere;
    /* Capped at the tab height and scrolled internally, so long lyrics never
       stretch the row; sticky keeps them in view while .np scrolls. */
    align-self: flex-start;
    position: sticky;
    top: 0;
    max-height: 100%;
    display: flex;
    flex-direction: column;
  }
  .lyrics-scroll {
    min-height: 0;
    overflow-y: auto;
    padding-right: 8px;
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
  .local b.grouping {
    position: relative;
    display: block;
    min-width: 0;
    white-space: normal;
  }
  .grouping-text {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
    overflow-wrap: anywhere;
  }
  .grouping-ellipsis {
    position: absolute;
    right: 0;
    bottom: 0;
    padding-left: 0.15em;
    background: var(--bg-main);
  }
  .local .grouping-text,
  .local .grouping-ellipsis {
    color: inherit;
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
  .lyrics {
    line-height: 1.65;
    white-space: pre-line;
    user-select: text;
    cursor: text;
  }
  .lyrics.searching {
    color: var(--txt-faded);
    font-style: italic;
  }
  /* Synced view: every line dim, the one being sung in the now-playing
     orange, same signal the track list uses. */
  .synced .line {
    line-height: 1.5;
    padding: 3px 0;
    min-height: 1.5em;
    color: var(--txt-semifaded);
    transition: color 120ms linear;
  }
  .synced .line.active {
    color: var(--accent);
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
