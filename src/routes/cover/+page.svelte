<script lang="ts">
  import { page } from "$app/state";
  import { artUrl, isTauri } from "$lib/api";

  const albumId = $derived(page.url.searchParams.get("album"));
  const trackId = $derived(page.url.searchParams.get("track"));

  const src = $derived.by(() => {
    if (albumId != null) return artUrl("album", Number(albumId), "full");
    if (trackId != null) return artUrl("track", Number(trackId), "full");
    return "/eole/no_cover.png";
  });

  async function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().close();
    }
  }

  function onImgError(e: Event) {
    (e.currentTarget as HTMLImageElement).src = "/eole/no_cover.png";
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="coverwin">
  <img {src} alt="Cover art" onerror={onImgError} />
</div>

<style>
  .coverwin {
    position: fixed;
    inset: 0;
    background: #000;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
</style>
