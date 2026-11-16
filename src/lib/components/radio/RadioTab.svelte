<script lang="ts">
  import { onMount } from "svelte";
  import { radio } from "$lib/stores/radio.svelte";
  import { player } from "$lib/stores/player.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import type { DirectoryStation, Station } from "$lib/api";
  import CollapsiblePanel from "../common/CollapsiblePanel.svelte";
  import Select from "../common/Select.svelte";
  import Icon from "../common/Icon.svelte";
  import StationLogo from "./StationLogo.svelte";
  import { openContextMenu } from "../common/ContextMenu.svelte";

  let adding = $state(false);
  let newName = $state("");
  let newUrl = $state("");
  let addError = $state<string | null>(null);
  let addBusy = $state(false);
  let renamingId = $state<number | null>(null);
  let renameText = $state("");
  let selectedId = $state<number | null>(null);

  onMount(() => {
    if (!radio.loaded) void radio.load();
    void radio.loadFilters();
    if (!radio.results.length && !radio.searchError) void radio.search();
  });

  // Search as the filters change; typing waits for a pause.
  let firstRun = true;
  $effect(() => {
    radio.query;
    radio.tag;
    radio.countrycode;
    if (firstRun) {
      firstRun = false;
      return;
    }
    const timer = setTimeout(() => void radio.search(), 300);
    return () => clearTimeout(timer);
  });

  const genreOptions = $derived([
    { value: "", label: "All genres" },
    ...radio.tags.map((t) => ({ value: t.name, label: t.name })),
  ]);
  const countryOptions = $derived([
    { value: "", label: "All countries" },
    ...radio.countries.map((c) => ({ value: c.code, label: c.name })),
  ]);

  function isPlaying(id: number | null, uuid: string | null): boolean {
    const s = player.onStation ? player.station : null;
    if (!s) return false;
    return (id != null && s.id === id) || (uuid != null && s.uuid === uuid);
  }

  function playSaved(station: Station) {
    void player.playStation({ id: station.id });
  }

  function playListed(station: DirectoryStation) {
    void player.playStation({ uuid: station.uuid });
  }

  function savedMenu(e: MouseEvent, station: Station) {
    selectedId = station.id;
    openContextMenu(e, [
      { label: "Play", action: () => playSaved(station) },
      {
        label: "Rename",
        action: () => {
          renamingId = station.id;
          renameText = station.name;
        },
      },
      { label: "", separator: true },
      { label: "Remove from my stations", action: () => void radio.remove(station.id) },
    ]);
  }

  function listedMenu(e: MouseEvent, station: DirectoryStation) {
    const saved = radio.stations.find((s) => s.uuid === station.uuid);
    openContextMenu(e, [
      { label: "Play", action: () => playListed(station) },
      saved
        ? { label: "Remove from my stations", action: () => void radio.remove(saved.id) }
        : { label: "Save to my stations", action: () => void radio.save(station.uuid) },
    ]);
  }

  function toggleSaved(station: DirectoryStation) {
    const saved = radio.stations.find((s) => s.uuid === station.uuid);
    if (saved) void radio.remove(saved.id);
    else void radio.save(station.uuid);
  }

  async function submitAdd(e: SubmitEvent) {
    e.preventDefault();
    addBusy = true;
    addError = null;
    try {
      await radio.addUrl(newName, newUrl);
      adding = false;
      newName = "";
      newUrl = "";
    } catch (error) {
      addError = error instanceof Error ? error.message : String(error);
    } finally {
      addBusy = false;
    }
  }

  function finishRename() {
    if (renamingId != null) void radio.rename(renamingId, renameText);
    renamingId = null;
  }

  function focusSelect(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  function details(s: { country: string | null; codec: string | null; bitrate: number | null }) {
    return [s.country, s.codec, s.bitrate ? `${s.bitrate} kbps` : null].filter(Boolean).join(" · ");
  }
</script>

<div class="tab">
  {#if player.stationError || radio.notice}
    <button
      class="status error"
      title="Dismiss"
      onclick={() => {
        player.stationError = null;
        radio.notice = null;
      }}
    >
      {player.stationError ?? radio.notice}
    </button>
  {/if}
  <div class="cols">
    <CollapsiblePanel
      side="left"
      size={ui.radioPanelW}
      min={170}
      max={480}
      persistKey="radioPanelW"
      collapsePersistKey="radioSidebarCollapsed"
      collapsed={ui.radioSidebarCollapsed}
      panelId="radio-stations-panel"
      label="My stations"
      transitionsReady={ui.layoutReady}
      onresize={(px) => (ui.radioPanelW = px)}
      oncollapsedchange={(collapsed) => (ui.radioSidebarCollapsed = collapsed)}
    >
      <div class="left">
        <div class="panelhead">My stations</div>
        <div class="saved">
          {#if radio.loaded && radio.stations.length === 0}
            <p class="hint">
              No saved stations yet. Save one from the directory with its star, or add a
              stream address below.
            </p>
          {/if}
          {#each radio.stations as station (station.id)}
            {#if renamingId === station.id}
              <input
                class="rename"
                bind:value={renameText}
                use:focusSelect
                onblur={finishRename}
                onkeydown={(e) => {
                  if (e.key === "Enter") finishRename();
                  if (e.key === "Escape") renamingId = null;
                }}
              />
            {:else}
              <button
                class="srow"
                class:selected={selectedId === station.id}
                class:playing={isPlaying(station.id, station.uuid)}
                title={details(station) || station.stream_url}
                onclick={() => (selectedId = station.id)}
                ondblclick={() => playSaved(station)}
                oncontextmenu={(e) => savedMenu(e, station)}
              >
                <StationLogo logo={station.logo} />
                <span class="sname">{station.name}</span>
                {#if isPlaying(station.id, station.uuid)}
                  <Icon name="volume-2" size={14} />
                {/if}
              </button>
            {/if}
          {/each}
        </div>
        {#if adding}
          <form class="addform" onsubmit={submitAdd}>
            <input placeholder="Name" bind:value={newName} disabled={addBusy} use:focusSelect />
            <input
              placeholder="Stream or playlist address (http…)"
              bind:value={newUrl}
              disabled={addBusy}
            />
            {#if addError}<p class="adderror">{addError}</p>{/if}
            <div class="addbuttons">
              <button type="button" class="btn" disabled={addBusy} onclick={() => (adding = false)}>
                Cancel
              </button>
              <button type="submit" class="btn" disabled={addBusy || !newName.trim() || !newUrl.trim()}>
                {addBusy ? "Checking…" : "Save"}
              </button>
            </div>
          </form>
        {:else}
          <button class="addbtn" onclick={() => (adding = true)}>
            <Icon name="plus" size={14} /> Add by URL
          </button>
        {/if}
      </div>
    </CollapsiblePanel>

    <section class="main">
      <header class="viewheader">
        <input
          class="search"
          type="search"
          placeholder="Search stations…"
          spellcheck="false"
          bind:value={radio.query}
        />
        <div class="filters">
          <Select
            ariaLabel="Genre"
            value={radio.tag}
            options={genreOptions}
            onchange={(value) => (radio.tag = value)}
          />
          <Select
            ariaLabel="Country"
            value={radio.countrycode}
            options={countryOptions}
            onchange={(value) => (radio.countrycode = value)}
          />
        </div>
        <span class="count">
          {radio.searching ? "Searching…" : `${radio.results.length}${radio.hasMore ? "+" : ""} stations`}
        </span>
      </header>
      <div class="colhead">
        <span class="c-logo"></span>
        <span class="c-name">Name</span>
        <span class="c-country">Country</span>
        <span class="c-tags">Tags</span>
        <span class="c-codec">Codec</span>
        <span class="c-kbps">kbps</span>
        <span class="c-save"></span>
      </div>
      <div class="results" style:padding-bottom={`${ui.bottombarH}px`}>
        {#if radio.searchError}
          <p class="empty error">{radio.searchError}</p>
        {:else if !radio.searching && radio.results.length === 0}
          <p class="empty">No stations found.</p>
        {/if}
        {#each radio.results as station, i (station.uuid)}
          {@const saved = radio.savedUuids.has(station.uuid)}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div
            class="rrow"
            class:alt={i % 2 === 1}
            class:playing={isPlaying(null, station.uuid)}
            title={station.homepage ?? undefined}
            ondblclick={() => playListed(station)}
            oncontextmenu={(e) => listedMenu(e, station)}
          >
            <span class="c-logo"><StationLogo logo={station.logo} size={24} /></span>
            <span class="c-name">
              <button class="play" title="Play" onclick={() => playListed(station)}>
                <Icon name="play" size={12} />
              </button>
              <span class="rname">{station.name}</span>
            </span>
            <span class="c-country" title={station.country ?? undefined}>{station.country ?? ""}</span>
            <span class="c-tags" title={station.tags ?? undefined}>{(station.tags ?? "").replaceAll(",", ", ")}</span>
            <span class="c-codec">{station.codec ?? ""}</span>
            <span class="c-kbps">{station.bitrate ?? ""}</span>
            <span class="c-save">
              <button
                class="star"
                class:on={saved}
                title={saved ? "Remove from my stations" : "Save to my stations"}
                onclick={() => toggleSaved(station)}
              >
                <Icon name="star" size={14} />
              </button>
            </span>
          </div>
        {/each}
        {#if radio.hasMore && !radio.searchError}
          <button class="more" disabled={radio.searching} onclick={() => void radio.search(true)}>
            {radio.searching ? "Loading…" : "Show more"}
          </button>
        {/if}
      </div>
    </section>
  </div>
</div>

<style>
  .tab {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .cols {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .status {
    flex: none;
    width: 100%;
    min-height: 24px;
    padding: 3px 12px;
    text-align: left;
    background: color-mix(in srgb, var(--accent) 18%, var(--bg-panel));
    color: var(--txt);
    border-bottom: 1px solid var(--hairline);
  }
  .status.error {
    color: var(--warning, #ffba66);
  }
  .left {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg-panel);
    /* The transport bar floats over the bottom of the tab. */
    padding-bottom: var(--bottombar-h);
  }
  .panelhead {
    flex: none;
    display: flex;
    align-items: center;
    height: var(--header-h);
    padding: 0 12px;
    border-bottom: 1px solid var(--hairline);
    color: var(--txt-faded);
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  .saved {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 0;
  }
  .hint {
    padding: 8px 12px;
    color: var(--txt-faded);
    font-size: 11px;
    line-height: 1.4;
  }
  .srow {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 32px;
    padding: 0 10px;
    color: var(--txt);
    text-align: left;
  }
  .srow:hover {
    background: var(--hover-row);
  }
  .srow.selected {
    background: var(--selection);
  }
  .srow.playing {
    color: var(--accent);
  }
  .sname {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rename {
    width: calc(100% - 16px);
    margin: 3px 8px;
  }
  .addbtn {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 12px;
    color: var(--txt-faded);
    border-top: 1px solid var(--hairline);
  }
  .addbtn:hover {
    color: var(--txt-full);
  }
  .addform {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px;
    border-top: 1px solid var(--hairline);
  }
  .adderror {
    color: var(--warning, #ffba66);
    font-size: 11px;
  }
  .addbuttons {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .viewheader {
    display: flex;
    align-items: center;
    gap: 10px;
    height: var(--header-h);
    padding: 0 16px;
    border-bottom: 1px solid var(--hairline);
    flex: none;
    --select-width: 170px;
    --select-padding: 2px 18px 2px 6px;
    --select-font-size: 11px;
    --select-arrow-size: 10px;
    --select-arrow-inset: 5px;
  }
  .search {
    flex: 1;
    min-width: 120px;
    max-width: 360px;
  }
  .filters {
    display: flex;
    gap: 6px;
  }
  .count {
    margin-left: auto;
    color: var(--txt-faded);
    font-size: 11px;
    flex: none;
  }
  .colhead,
  .rrow {
    display: grid;
    grid-template-columns: 30px minmax(0, 3fr) minmax(0, 1.4fr) minmax(0, 2fr) 54px 44px 28px;
    align-items: center;
    gap: 10px;
    padding: 0 16px 0 12px;
  }
  .colhead {
    flex: none;
    height: 24px;
    color: var(--txt-faded);
    font-size: 11px;
    border-bottom: 1px solid var(--hairline);
  }
  .results {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .rrow {
    height: 36px;
    color: var(--txt);
    user-select: none;
  }
  .rrow.alt {
    background: var(--row-alt);
  }
  .rrow:hover {
    background: var(--hover-row);
  }
  .rrow.playing {
    color: var(--accent);
  }
  .rrow > span {
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .c-country,
  .c-tags,
  .c-codec,
  .c-kbps {
    color: var(--txt-faded);
  }
  .rrow.playing .c-country,
  .rrow.playing .c-tags,
  .rrow.playing .c-codec,
  .rrow.playing .c-kbps {
    color: inherit;
  }
  .c-kbps {
    text-align: right;
  }
  .c-name {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .rname {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .play {
    display: grid;
    place-items: center;
    flex: none;
    width: 20px;
    height: 20px;
    color: var(--txt-faded);
    visibility: hidden;
  }
  .rrow:hover .play {
    visibility: visible;
  }
  .play:hover,
  .star:hover {
    color: var(--txt-full);
  }
  .star {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    color: var(--txt-superfaded);
  }
  .rrow:hover .star {
    color: var(--txt-faded);
  }
  .star.on,
  .rrow:hover .star.on {
    color: var(--accent);
  }
  .star.on :global(svg) {
    fill: currentColor;
  }
  .empty {
    padding: 24px 16px;
    color: var(--txt-faded);
  }
  .empty.error {
    color: var(--warning, #ffba66);
  }
  .btn {
    border: 1px solid var(--border);
    padding: 3px 12px;
    color: var(--txt);
  }
  .btn:hover:not(:disabled) {
    background: var(--hover-row);
  }
  .btn:disabled {
    color: var(--txt-faded);
    cursor: default;
  }
  .more {
    display: block;
    margin: 10px auto;
    padding: 4px 14px;
    border: 1px solid var(--border);
    color: var(--txt);
  }
  .more:hover:not(:disabled) {
    color: var(--txt-full);
  }
</style>
