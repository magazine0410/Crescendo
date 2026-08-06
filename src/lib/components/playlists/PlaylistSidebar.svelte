<script lang="ts">
  import { playlists, type SidebarRow } from "$lib/stores/playlists.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { isTauri, type PlaylistInfo } from "$lib/api";
  import { openContextMenu, type MenuItem } from "../common/ContextMenu.svelte";
  import { openSetCollection } from "./SetCollectionDialog.svelte";
  import Icon from "../common/Icon.svelte";

  let renamingId = $state<number | null>(null);
  let renamingCollectionId = $state<number | null>(null);
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

  function startCollectionRename(id: number, current: string) {
    renamingCollectionId = id;
    renameValue = current;
  }

  async function commitCollectionRename() {
    if (renamingCollectionId != null && renameValue.trim()) {
      await playlists.renameCollection(renamingCollectionId, renameValue.trim());
    }
    renamingCollectionId = null;
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

  /** Right-clicking inside a multi-selection acts on the whole selection;
      right-clicking elsewhere acts on the row under the pointer. */
  function menu(e: MouseEvent, id: number, name: string) {
    const ids =
      playlists.isSelected(id) && playlists.selectedIds.length > 1 ? playlists.selectedIds : [id];
    const many = ids.length > 1;
    const inCollection = ids.some(
      (i) => playlists.list.find((p) => p.id === i)?.collection_id != null,
    );
    const items: MenuItem[] = [
      { label: "Rename", disabled: many, action: () => startRename(id, name) },
      {
        label: many ? `Delete ${ids.length} playlists` : "Delete playlist",
        action: async () => {
          for (const i of ids) await playlists.remove(i);
        },
      },
      { separator: true, label: "" },
      {
        label: many ? `Set collection… (${ids.length} playlists)` : "Set collection…",
        action: () => openSetCollection(ids),
      },
    ];
    if (inCollection) {
      items.push({
        label: "Remove from collection",
        action: () => playlists.setCollection(ids, ""),
      });
    }
    openContextMenu(e, items);
  }

  function collectionMenu(e: MouseEvent, id: number, name: string) {
    openContextMenu(e, [
      { label: "Rename collection", action: () => startCollectionRename(id, name) },
      {
        // The playlists inside outlive the group that held them.
        label: "Remove collection (keep playlists)",
        action: () => playlists.removeCollection(id),
      },
    ]);
  }

  // --- Dragging playlists into and out of collections ---
  //
  // Pointer events, not HTML5 drag-and-drop: the webview reserves drag-and-drop
  // for files coming from the file manager (see events.ts), and the rest of
  // this app hand-rolls its drags the same way (Splitter, ColorPicker).

  /** Playlists being dragged; empty when no drag is in progress. */
  let dragIds = $state<number[]>([]);
  /** Collection under the pointer: an id, null for the loose area, or
      undefined when the pointer is outside the list (dropping there does
      nothing). */
  let dragTarget = $state<number | null | undefined>(undefined);
  let dragFrom: { x: number; y: number; id: number } | null = null;
  /** A drag ends with a click event on the row; that click must not also
      change the selection. */
  let suppressClick = false;

  const DRAG_THRESHOLD = 4;

  function onRowPointerDown(e: PointerEvent, id: number) {
    if (e.button !== 0) return;
    dragFrom = { x: e.clientX, y: e.clientY, id };
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp, { once: true });
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragFrom) return;
    if (dragIds.length === 0) {
      const far =
        Math.abs(e.clientX - dragFrom.x) > DRAG_THRESHOLD ||
        Math.abs(e.clientY - dragFrom.y) > DRAG_THRESHOLD;
      if (!far) return;
      // Dragging a row of a multi-selection drags the whole selection, the
      // same way the right-click menu acts on it.
      dragIds =
        playlists.isSelected(dragFrom.id) && playlists.selectedIds.length > 1
          ? [...playlists.selectedIds]
          : [dragFrom.id];
    }
    dragTarget = collectionAt(e.clientX, e.clientY);
  }

  async function onPointerUp() {
    window.removeEventListener("pointermove", onPointerMove);
    const ids = dragIds;
    const target = dragTarget;
    const dragged = ids.length > 0;
    dragFrom = null;
    dragIds = [];
    dragTarget = undefined;
    if (!dragged) return;
    suppressClick = true;
    setTimeout(() => (suppressClick = false));
    if (target !== undefined) await applyDrop(ids, target);
  }

  /** Which collection the pointer is over: an id, null for the loose area of
      the list, undefined for anywhere else (including outside the sidebar). */
  function collectionAt(x: number, y: number): number | null | undefined {
    const el = document.elementFromPoint(x, y);
    if (!el?.closest(".items")) return undefined;
    const marker = el.closest<HTMLElement>("[data-collection]");
    return marker ? Number(marker.dataset.collection) : null;
  }

  async function applyDrop(ids: number[], target: number | null) {
    const where = (id: number) => playlists.list.find((p) => p.id === id)?.collection_id ?? null;
    if (ids.every((id) => where(id) === target)) return; // already there
    const name =
      target == null ? "" : (playlists.collections.find((c) => c.id === target)?.name ?? "");
    if (target != null && !name) return;
    await playlists.setCollection(ids, name);
  }

  /** True while the drag would take playlists out of a collection, so the
      list can show that dropping here is a real change. */
  const draggingOutOfCollection = $derived(
    dragIds.some((id) => playlists.list.find((p) => p.id === id)?.collection_id != null),
  );

  /** Stable key per drawn row; collections and playlists share the list. */
  function rowKey(row: SidebarRow): string {
    return row.kind === "collection"
      ? `c${row.collection.id}`
      : row.kind === "playlist"
        ? `p${row.playlist.id}`
        : "sep";
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
    <span class="count">{playlists.matching.length}</span>
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
  <div
    class="items"
    class:dragging={dragIds.length > 0}
    class:dragloose={dragIds.length > 0 && dragTarget === null && draggingOutOfCollection}
  >
    {#if playlists.matching.length === 0 && playlists.filter}
      <div class="nomatch">No playlist matches “{playlists.filter}”</div>
    {/if}
    {#each playlists.sidebar as row (rowKey(row))}
      {#if row.kind === "separator"}
        <div class="csep"></div>
      {:else if row.kind === "collection"}
        {#if renamingCollectionId === row.collection.id}
          <input
            class="rename"
            bind:value={renameValue}
            onblur={commitCollectionRename}
            onkeydown={(e) => {
              if (e.key === "Enter") commitCollectionRename();
              if (e.key === "Escape") renamingCollectionId = null;
            }}
            {@attach (el) => el.focus()}
          />
        {:else}
          <button
            class="citem"
            class:dragover={dragIds.length > 0 && dragTarget === row.collection.id}
            data-collection={row.collection.id}
            title={`${row.collection.playlist_count} playlists`}
            onclick={() => !suppressClick && playlists.toggleCollection(row.collection.id)}
            oncontextmenu={(e) => collectionMenu(e, row.collection.id, row.collection.name)}
          >
            <span class="chev" class:open={row.expanded}>
              <Icon name="chevron-right" size={12} />
            </span>
            <span class="cname">{row.collection.name}</span>
            <span class="n">{row.collection.track_count}</span>
          </button>
        {/if}
        {#if row.expanded}
          {#each row.children as p (p.id)}
            {@render playlistRow(p, true, row.collection.id)}
          {/each}
        {/if}
      {:else}
        {@render playlistRow(row.playlist, false, null)}
      {/if}
    {/each}
  </div>
</aside>

{#snippet playlistRow(p: PlaylistInfo, nested: boolean, collectionId: number | null)}
  {#if renamingId === p.id}
    <input
      class="rename"
      class:nested
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
      class:nested
      class:selected={playlists.isSelected(p.id)}
      class:droptarget={playlists.dropTargetId === p.id}
      class:dragging={dragIds.includes(p.id)}
      class:dragover={dragIds.length > 0 &&
        !dragIds.includes(p.id) &&
        dragTarget === collectionId &&
        collectionId !== null}
      data-drop-playlist={p.id}
      data-collection={collectionId}
      onpointerdown={(e) => onRowPointerDown(e, p.id)}
      onclick={(e) => {
        if (suppressClick) return;
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
{/snippet}

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
  /* collection row: a fold-out group of playlists */
  .citem {
    display: flex;
    align-items: center;
    width: 100%;
    gap: 6px;
    padding: 7px 12px;
    color: var(--txt);
    text-align: left;
  }
  .citem:hover {
    background: var(--hover-row);
  }
  .chev {
    display: flex;
    flex: none;
    color: var(--txt-faded);
    transition: transform 0.12s ease;
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .cname {
    font-weight: 600;
    color: var(--txt-full);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* a playlist drawn inside a collection */
  .item.nested {
    padding-left: 30px;
  }
  .rename.nested {
    margin-left: 30px;
    width: calc(100% - 42px);
  }
  .csep {
    height: 1px;
    margin: 6px 12px;
    background: var(--hairline);
  }
  /* dragging a playlist into or out of a collection */
  .items.dragging {
    cursor: grabbing;
  }
  .items.dragloose {
    box-shadow: inset 0 0 0 1px var(--selection);
  }
  .item.dragging {
    opacity: 0.5;
  }
  .citem.dragover,
  .item.dragover {
    background: var(--hover-row);
    box-shadow: inset 0 0 0 1px var(--selection);
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
