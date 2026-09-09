<script lang="ts">
  import { library } from "$lib/stores/library.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { queue } from "$lib/stores/queue.svelte";
  import { playlists } from "$lib/stores/playlists.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { artUrl, call, type Album, type Track } from "$lib/api";
  import { openContextMenu, type MenuItem } from "../common/ContextMenu.svelte";
  import { addToPlaylistItem, openInFileManager } from "$lib/menu";
  import VirtualGrid from "$lib/virtual/VirtualGrid.svelte";
  import { displayName } from "$lib/format";
  import { fileDrag } from "$lib/fileDrag";

  const CELL_W = 150;
  const CELL_H = 196; // 150 art + text

  async function playAlbum(a: Album) {
    if (library.interactionBlocked) return;
    await player.playContext({ type: "album", id: a.id }, 0);
  }

  function showAlbum(a: Album) {
    if (library.interactionBlocked) return;
    // A direct Library drill-in starts a fresh navigation path; do not retain
    // an origin tab from an earlier cross-tab album visit.
    ui.albumReturnTab = null;
    void library.selectAlbum(a);
  }

  async function albumMenu(e: MouseEvent, a: Album) {
    if (library.interactionBlocked) return;
    const items: MenuItem[] = [
      { label: "Play", action: () => playAlbum(a) },
      { label: "Show tracks", action: () => showAlbum(a) },
      {
        label: "Open in file manager",
        action: async () => {
          const tracks = await call<Track[]>("album_tracks", { albumId: a.id });
          const first = tracks.find((t) => !t.missing);
          if (first) await openInFileManager(first.path);
        },
      },
      {
        label: "Add to queue",
        action: async () => {
          const tracks = await call<Track[]>("album_tracks", { albumId: a.id });
          await queue.add(tracks.map((t) => t.id));
        },
      },
      { separator: true, label: "" },
      addToPlaylistItem(async (pid) => {
        const tracks = await call<Track[]>("album_tracks", { albumId: a.id });
        await playlists.addTracks(pid, tracks.map((t) => t.id));
      }),
    ];
    openContextMenu(e, items);
  }

  function onImgError(e: Event) {
    (e.currentTarget as HTMLImageElement).src = "/eole/no_cover.png";
  }
</script>

<div class="wall">
  <VirtualGrid
    count={library.albums.length}
    cellWidth={CELL_W}
    cellHeight={CELL_H}
    initialScrollTop={library.wallScroll}
    scrollResetKey={library.scrollResetKey}
    onScrollChange={(y) => (library.wallScroll = y)}
    cell={cellSnippet}
  />
</div>

{#snippet cellSnippet(i: number)}
  {@const a = library.albums[i]}
  <button
    class="cell"
    use:fileDrag={{
      source: () => ({ type: "context", ctx: { type: "album", id: a.id } }),
      disabled: () => library.interactionBlocked,
    }}
    disabled={library.interactionBlocked}
    onclick={() => showAlbum(a)}
    ondblclick={() => playAlbum(a)}
    oncontextmenu={(e) => albumMenu(e, a)}
    title={`${a.album ?? "?"} — ${a.albumartist ?? "?"}`}
  >
    <div class="art">
      {#if a.year}<span class="year">{a.year}</span>{/if}
      <img src={artUrl("album", a.id)} alt="" loading="lazy" onerror={onImgError} />
    </div>
    <div class="meta">
      <div class="artist">{displayName(a.albumartist, a.albumartist_sort) ?? "?"}</div>
      <div class="album">{a.album ?? "?"}</div>
    </div>
  </button>
{/snippet}

<style>
  .wall {
    height: 100%;
    min-width: 0;
    flex: 1;
    padding-bottom: var(--bottombar-h);
  }
  .cell {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    text-align: left;
    gap: 5px;
    padding-top: 14px;
  }
  .art {
    position: relative;
    width: 100%;
    aspect-ratio: 1;
    background: var(--bg-panel);
    overflow: hidden;
  }
  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .year {
    position: absolute;
    top: 0;
    left: 0;
    z-index: 2;
    background: rgba(0, 0, 0, 0.65);
    color: var(--txt);
    font-size: 10px;
    padding: 2px 6px;
  }
  .cell:hover .art {
    outline: 2px solid var(--scroll-thumb-hover);
    outline-offset: -2px;
  }
  .meta {
    line-height: 1.35;
    overflow: hidden;
  }
  .artist {
    color: var(--txt);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 600;
  }
  .album {
    color: var(--txt-faded);
    font-style: italic;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
