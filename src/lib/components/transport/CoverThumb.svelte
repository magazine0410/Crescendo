<script lang="ts">
  import { isTauri, call, artUrl } from "$lib/api";
  import { player } from "$lib/stores/player.svelte";

  const src = $derived.by(() => {
    const t = player.track;
    if (!t) return "/eole/controls/nothing_played.png";
    if (isTauri && t.album_id != null) return artUrl("album", t.album_id, "thumb");
    if (isTauri) return artUrl("track", t.id, "thumb");
    return "/eole/no_cover.png";
  });

  function openBig() {
    if (!player.track) return;
    if (isTauri) call("open_cover_window", { albumId: player.track.album_id, trackId: player.track.id });
  }

  function onError(e: Event) {
    (e.currentTarget as HTMLImageElement).src = "/eole/no_cover.png";
  }
</script>

<button class="thumb" onclick={openBig} title="Show cover art">
  <img {src} alt="Cover art" onerror={onError} />
</button>

<style>
  .thumb {
    height: 100%;
    aspect-ratio: 1;
    flex: none;
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
</style>
