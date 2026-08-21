<script lang="ts">
  import { isTauri, call, artUrl } from "$lib/api";
  import { player } from "$lib/stores/player.svelte";
  import Icon from "$lib/components/common/Icon.svelte";

  const src = $derived.by(() => {
    const t = player.track;
    if (!t) return null;
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
  {#if src}
    <img {src} alt="Cover art" onerror={onError} />
  {:else}
    <span class="empty"><Icon name="palette" size={40} /></span>
  {/if}
</button>

<style>
  .thumb {
    height: 100%;
    aspect-ratio: 1;
    flex: none;
    overflow: hidden;
  }
  .empty {
    width: 100%;
    height: 100%;
    display: grid;
    place-items: center;
    border-right: 1px solid var(--hairline);
    color: var(--txt);
    transition: color 0.1s;
  }
  .thumb:hover .empty {
    color: var(--txt-full);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
</style>
