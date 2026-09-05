<script lang="ts">
  import { onMount } from "svelte";
  import { call } from "$lib/api";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { displayAlbumartist, displayTitle } from "$lib/format";
  import { historyDate, historyAgo, type HistoryEntry } from "$lib/history";
  import Icon from "../common/Icon.svelte";
  import { openContextMenu, closeContextMenu } from "../common/ContextMenu.svelte";

  let rows = $state<HistoryEntry[]>([]);
  let urls = $state<Record<number, string | null>>({});
  let cursors = $state<(number | null)[]>([null]);
  let loading = $state(true);
  let error = $state("");
  let lookupError = $state("");
  let currentTime = $state(Date.now());
  let request = 0;
  let alive = true;
  let box: HTMLDivElement;

  async function resolveLinks(entries: HistoryEntry[], version: number) {
    if (!prefs.values.lastfm_api_key) return;
    const pending = [...new Map(entries.map(r => [r.track.id, r])).values()]
      .filter(r => urls[r.track.id] === undefined);
    // At most two requests; don't queue pages the user has already left.
    let index = 0;
    const worker = async () => {
      while (alive && version === request && index < pending.length) {
        const row = pending[index++];
        try {
          const url = await call<string | null>("lastfm_track_url", { trackId: row.track.id });
          if (!alive || version !== request) return;
          urls[row.track.id] = url;
        } catch {
          if (alive && version === request) lookupError = "Last.fm availability could not be checked. Refresh to retry.";
          return;
        }
      }
    };
    await Promise.all([worker(), worker()]);
  }

  async function load(nextCursors = cursors, quiet = false) {
    const version = ++request;
    if (!quiet) loading = true;
    error = "";
    lookupError = "";
    try {
      const result = await call<HistoryEntry[]>("song_history", { before: nextCursors.at(-1) ?? null });
      if (!alive || version !== request) return;
      rows = result;
      cursors = nextCursors;
      if (!quiet && box) box.scrollTop = 0;
      void resolveLinks(result, version);
    } catch (e) { if (alive && version === request) error = String(e); }
    finally { if (alive && version === request) loading = false; }
  }

  onMount(() => {
    void load([null]);
    const timer = setInterval(() => {
      currentTime = Date.now();
      if (cursors.length === 1 && !loading) void load(cursors, true);
    }, 30000);
    return () => { alive = false; request++; clearInterval(timer); closeContextMenu(); };
  });

  function context(event: MouseEvent, row: HistoryEntry) {
    if (!urls[row.track.id]) { event.preventDefault(); closeContextMenu(); return; }
    openContextMenu(event, [{ label: "Open on Last.fm", action: () => {
      void call("lastfm_open_track", { trackId: row.track.id }).catch(e => error = String(e));
    } }]);
  }
</script>

<section aria-label="Song history">
  <div class="heading"><h3>Song history</h3><button class="btn" disabled={loading} onclick={() => load([null])}>Refresh</button></div>
  <label for="history-time">History timestamps</label>
  <select id="history-time" value={prefs.values.history_time_format}
    onchange={(e) => prefs.set("history_time_format", e.currentTarget.value as "absolute" | "relative").catch(e => error = String(e))}>
    <option value="absolute">Date and time · 05-09-26, 13:37</option>
    <option value="relative">Time ago · 1 week ago</option>
  </select>
  <p class="hint">Songs appear after listening to more than half, whether or not scrobbling is enabled. Times show when playback started. Hover over a time for the exact date.</p>
  {#if !prefs.values.lastfm_api_key}<p class="hint">Add a Last.fm API key above to check song availability.</p>{/if}
  <div class="history" bind:this={box} aria-busy={loading}>
    {#each rows as row (row.id)}
      <button class="history-row" oncontextmenu={(e) => context(e, row)}
        aria-label={`${displayAlbumartist(row.track) ?? "Unknown artist"} - ${displayTitle(row.track)}, ${historyDate(row.played_at)}`}>
        <span class="availability" title={urls[row.track.id] ? "Available on Last.fm" : undefined}>
          {#if urls[row.track.id]}<Icon name="music-2" size={14} />{/if}
        </span>
        <span class="song" title={`${displayAlbumartist(row.track) ?? "Unknown artist"} - ${displayTitle(row.track)}`}>
          {displayAlbumartist(row.track) ?? "Unknown artist"} - {displayTitle(row.track)}
        </span>
        <time datetime={new Date(row.played_at * 1000).toISOString()} title={historyDate(row.played_at)}>
          {prefs.values.history_time_format === "relative" ? historyAgo(row.played_at, currentTime) : historyDate(row.played_at)}
        </time>
      </button>
    {:else}
      <p class="empty">{loading ? "Loading history…" : "No listening history yet."}</p>
    {/each}
  </div>
  <div class="paging">
    <button class="btn" disabled={loading || cursors.length < 2} onclick={() => load(cursors.slice(0, -1))}>Newer</button>
    <span>Page {cursors.length}</span>
    <button class="btn" disabled={loading || rows.length < 50} onclick={() => load([...cursors, rows.at(-1)!.id])}>Older</button>
  </div>
  {#if lookupError}<p class="hint" role="status">{lookupError}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</section>

<style>
  section { border-top: 1px solid var(--border); padding-top: 14px; margin-top: 18px; }
  .heading { display: flex; justify-content: space-between; align-items: center; margin-bottom: 10px; }
  h3 { font-size: 13px; font-weight: 500; }
  label { display: block; color: var(--txt-faded); font-size: 12px; margin-bottom: 4px; }
  select {
    width: 100%;
    background-color: var(--bg-panel);
    border: 1px solid var(--border);
    padding: 6px 26px 6px 8px;
    color: var(--txt);
    /* WebKitGTK renders a native (light) combobox unless appearance is cleared */
    appearance: none;
    -webkit-appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%236e6e6e' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 8px center;
    cursor: pointer;
  }
  select option { background: var(--bg-main); color: var(--txt); }
  .hint { color: var(--txt-faded); font-size: 11px; line-height: 1.5; margin: 8px 0; }
  .history { max-height: 280px; overflow-y: auto; border: 1px solid var(--border); background: var(--bg-panel); }
  .history-row { display: flex; align-items: center; gap: 8px; width: 100%; min-height: 30px; padding: 5px 8px; text-align: left; }
  .history-row:nth-child(even) { background: var(--row-alt); }
  .history-row:hover, .history-row:focus-visible { background: var(--hover-row); }
  .availability { width: 14px; height: 14px; flex: none; color: var(--txt-faded); }
  .song { min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  time { color: var(--txt-faded); white-space: nowrap; font-size: 11px; font-variant-numeric: tabular-nums; }
  .empty { padding: 16px; color: var(--txt-faded); }
  .paging { display: flex; justify-content: space-between; align-items: center; margin-top: 8px; color: var(--txt-faded); font-size: 11px; }
  .btn { padding: 4px 10px; border: 1px solid var(--border); background: var(--bg-panel); }
  .btn:hover:enabled { background: var(--hover-row); }
  .btn:disabled { opacity: .4; }
  .error { margin-top: 8px; color: var(--accent); overflow-wrap: anywhere; }
</style>
