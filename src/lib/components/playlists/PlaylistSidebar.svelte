<script lang="ts">
  import { playlists } from "$lib/stores/playlists.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
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

  /** The header icon's menu. Check marks show only while the sort is locked,
      because that is when an order is actually being held; unlocked, the two
      sort entries are one-off actions. Locking needs a sort to hold, so it
      stays disabled until one has been picked. */
  function sortMenu(e: MouseEvent) {
    const key = prefs.values.playlist_sort;
    const locked = prefs.values.playlist_sort_locked;
    openContextMenu(e, [
      {
        label: "Sort A → Z",
        checked: locked && key === "name",
        action: () => playlists.applySort("name"),
      },
      {
        label: "Sort by track count",
        checked: locked && key === "count",
        action: () => playlists.applySort("count"),
      },
      { separator: true, label: "" },
      {
        label: "Lock sorting",
        checked: locked,
        disabled: key === "manual",
        action: () => playlists.setSortLocked(!locked),
      },
    ]);
  }
</script>

<aside class="sidebar">
  <div class="header" class:active={playlists.filter.length > 0}>
    <button
      class="sortbtn"
      class:locked={prefs.values.playlist_sort_locked}
      title="Sort playlists"
      aria-label="Sort playlists"
      onclick={sortMenu}
      oncontextmenu={sortMenu}
    >
      <Icon name="playlist_icon" size={14} />
    </button>
    <input
      bind:value={playlists.filter}
      type="text"
      placeholder="Playlists…"
      spellcheck="false"
      autocomplete="off"
    />
    {#if playlists.filter}
      <button class="clear" onclick={() => (playlists.filter = "")} title="Clear filter">✕</button>
    {/if}
    <span class="count">{playlists.visible.length}</span>
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
  {#if playlists.dropReport}
    <div class="report">
      {playlists.dropReport}
      <button onclick={() => (playlists.dropReport = null)}>✕</button>
    </div>
  {/if}
  <div class="items">
    {#if playlists.visible.length === 0 && playlists.filter}
      <div class="nomatch">No playlist matches “{playlists.filter}”</div>
    {/if}
    {#each playlists.visible as p (p.id)}
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
          class:selected={playlists.isSelected(p.id)}
          class:droptarget={playlists.dropTargetId === p.id}
          data-drop-playlist={p.id}
          onclick={(e) => {
            if (e.shiftKey) playlists.selectRange(p.id);
            else if (e.ctrlKey || e.metaKey) playlists.toggle(p.id);
            else playlists.select(p.id);
          }}
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
  /* Typing in the header filters the list, like the library's value filter. */
  .header:focus-within,
  .header.active {
    background: rgba(0, 0, 0, 0.25);
  }
  .header input {
    flex: 1;
    min-width: 0;
    font-size: 12px;
    color: var(--txt);
  }
  .header input::placeholder {
    color: var(--txt-faded);
  }
  .sortbtn {
    display: flex;
    flex: none;
    color: var(--txt-faded);
  }
  .sortbtn:hover,
  .sortbtn.locked {
    color: var(--txt);
  }
  .header .clear {
    flex: none;
    color: var(--txt-faded);
    font-size: 10px;
    padding: 2px 4px;
  }
  .header .clear:hover {
    color: var(--txt);
  }
  .header .count {
    margin-left: auto;
    font-size: 10px;
  }
  .nomatch {
    padding: 8px 12px;
    color: var(--txt-faded);
    font-size: 11px;
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
  /* Drag from the file manager is hovering this playlist. */
  .item.droptarget {
    background: var(--hover-row);
    box-shadow: inset 0 0 0 1px var(--selection);
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
