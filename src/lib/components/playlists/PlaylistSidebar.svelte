<script lang="ts">
  import { playlists } from "$lib/stores/playlists.svelte";
  import { isTauri } from "$lib/api";
  import { openContextMenu } from "../common/ContextMenu.svelte";
  import Icon from "../common/Icon.svelte";

  let renamingId = $state<number | null>(null);
  let renameValue = $state("");
  let importReport = $state<string | null>(null);

  async function createPlaylist() {
    const base = "New Playlist";
    let name = base;
    let n = 2;
    while (playlists.list.some((p) => p.name === name)) name = `${base} ${n++}`;
    const id = await playlists.create(name);
    startRename(id, name);
  }

  function startRename(id: number, current: string) {
    renamingId = id;
    renameValue = current;
  }

  async function commitRename() {
    if (renamingId != null && renameValue.trim()) {
      await playlists.rename(renamingId, renameValue.trim());
    }
    renamingId = null;
  }

  async function importM3u() {
    if (!isTauri) {
      importReport = "Import needs the desktop app";
      return;
    }
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const file = await open({
        multiple: false,
        filters: [{ name: "Playlists", extensions: ["m3u", "m3u8"] }],
      });
      if (typeof file !== "string") return;
      const r = await playlists.importM3u(file);
      importReport =
        r.missing.length === 0
          ? `Imported "${r.name}": ${r.added} tracks`
          : `Imported "${r.name}": ${r.added} tracks, ${r.missing.length} not in library`;
    } catch (e) {
      importReport = `Import failed: ${e}`;
    }
  }

  function menu(e: MouseEvent, id: number, name: string) {
    openContextMenu(e, [
      { label: "Rename", action: () => startRename(id, name) },
      { label: "Delete playlist", action: () => playlists.remove(id) },
    ]);
  }
</script>

<aside class="sidebar">
  <div class="header">
    <Icon name="playlist_icon" size={14} />
    <span>Playlists…</span>
    <span class="count">{playlists.list.length}</span>
  </div>
  <button class="create" onclick={createPlaylist}>
    <span class="plus">+</span> Create Playlist
  </button>
  <button class="create" onclick={importM3u}>
    <span class="plus">⤓</span> Import .m3u / .m3u8
  </button>
  {#if importReport}
    <div class="report">
      {importReport}
      <button onclick={() => (importReport = null)}>✕</button>
    </div>
  {/if}
  <div class="items">
    {#each playlists.list as p (p.id)}
      {#if renamingId === p.id}
        <input
          class="rename"
          bind:value={renameValue}
          onblur={commitRename}
          onkeydown={(e) => {
            if (e.key === "Enter") commitRename();
            if (e.key === "Escape") renamingId = null;
          }}
          {@attach (el) => el.focus()}
        />
      {:else}
        <button
          class="item"
          class:selected={playlists.selectedId === p.id}
          onclick={() => playlists.select(p.id)}
          oncontextmenu={(e) => menu(e, p.id, p.name)}
        >
          <span class="name">{p.name}</span>
          <span class="n">{p.track_count}</span>
        </button>
      {/if}
    {/each}
  </div>
</aside>

<style>
  .sidebar {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border-right: 1px solid var(--sidesline);
    overflow: hidden;
  }
  .header {
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--header-h);
    padding: 0 12px;
    color: var(--txt-faded);
    border-bottom: 1px solid var(--hairline);
    flex: none;
  }
  .header .count {
    margin-left: auto;
    font-size: 10px;
  }
  .create {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    color: var(--txt);
    text-align: left;
    flex: none;
  }
  .create:hover {
    background: var(--hover-row);
  }
  .plus {
    color: var(--txt-faded);
    width: 12px;
    text-align: center;
  }
  .report {
    padding: 6px 12px;
    font-size: 11px;
    color: var(--txt-faded);
    display: flex;
    gap: 8px;
    align-items: baseline;
    border-bottom: 1px solid var(--hairline);
  }
  .items {
    flex: 1;
    overflow-y: auto;
    padding-bottom: var(--bottombar-h);
  }
  .item {
    display: flex;
    align-items: center;
    width: 100%;
    gap: 8px;
    padding: 7px 12px;
    color: var(--txt);
    text-align: left;
  }
  .item:hover {
    background: var(--hover-row);
  }
  .item.selected {
    background: var(--selection);
    color: var(--txt-full);
  }
  .name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .n {
    margin-left: auto;
    color: var(--txt-faded);
    font-size: 10px;
  }
  .item.selected .n {
    color: var(--txt-full);
  }
  .rename {
    width: calc(100% - 24px);
    margin: 4px 12px;
    padding: 4px 6px;
    border: 1px solid var(--selection);
    color: var(--txt);
    background: var(--bg-main);
  }
</style>
