<script lang="ts">
  import { onMount } from "svelte";
  import { call } from "$lib/api";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { displayAlbumartist, displayTitle } from "$lib/format";
  import { historyDate, historyAgo, type HistoryEntry } from "$lib/history";
  import Icon from "../common/Icon.svelte";
  import Select from "../common/Select.svelte";
  import { openContextMenu, closeContextMenu } from "../common/ContextMenu.svelte";

  /** Whether a ListenBrainz account is connected: its pages are looked up
      with the account's token. */
  let { connected }: { connected: boolean } = $props();
  let rows = $state<HistoryEntry[]>([]);
  /** ListenBrainz pages by track id; undefined until looked up, null for a
      song ListenBrainz does not match. */
  let listenbrainz = $state<Record<number, string | null>>({});
  let cursors = $state<(number | null)[]>([null]);
  let loading = $state(true);
  let error = $state("");
  let lookupError = $state("");
  let currentTime = $state(Date.now());
  let request = 0;
  let alive = true;
  let box: HTMLDivElement;

  /** The page a play links to, if it has one. */
  function link(row: HistoryEntry): string | null {
    return row.service === "lastfm" ? row.url : (listenbrainz[row.track.id] ?? null);
  }

  /** One request for every play on the page that is not yet looked up. */
  async function resolveLinks(entries: HistoryEntry[], version: number) {
    if (!connected) return;
    const trackIds = [...new Set(entries.filter(r => r.service === "listenbrainz").map(r => r.track.id))]
      .filter(id => listenbrainz[id] === undefined);
    if (!trackIds.length) return;
    try {
      const found = await call<{ urls: Record<number, string>; failed: boolean }>("listenbrainz_links", { trackIds });
      if (!alive || version !== request) return;
      // A failed lookup says nothing about a song, so it stays unknown.
      for (const id of trackIds) {
        if (found.urls[id]) listenbrainz[id] = found.urls[id];
        else if (!found.failed) listenbrainz[id] = null;
      }
      if (found.failed) lookupError = "ListenBrainz availability could not be checked. Refresh to retry.";
    } catch {
      if (alive && version === request) lookupError = "ListenBrainz availability could not be checked. Refresh to retry.";
    }
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

  const serviceName = { lastfm: "Last.fm", listenbrainz: "ListenBrainz" };

  /** What the note in front of a play says, or null for no note. Every play
      since the move to ListenBrainz gets one once its match is known: white
      when ListenBrainz matches the song, faint when it does not. An older
      play gets a faint one when Last.fm has a page for it. */
  function mark(row: HistoryEntry): { title: string; matched: boolean } | null {
    if (row.service === "lastfm") return row.url ? { title: "Available on Last.fm", matched: false } : null;
    const page = listenbrainz[row.track.id];
    if (page === undefined) return null;
    return page ? { title: "Available on ListenBrainz", matched: true } : { title: "Not matched on ListenBrainz", matched: false };
  }

  function context(event: MouseEvent, row: HistoryEntry) {
    const url = link(row);
    if (!url) { event.preventDefault(); closeContextMenu(); return; }
    openContextMenu(event, [{ label: `Open on ${serviceName[row.service]}`, action: () => {
      void call("open_info_link", { url }).catch(e => error = String(e));
    } }]);
  }
</script>

<section aria-label="Song history">
  <div class="heading"><h3>Song history</h3><button class="btn" disabled={loading} onclick={() => load([null])}>Refresh</button></div>
  <label for="history-time">History timestamps</label>
  <Select
    id="history-time"
    value={prefs.values.history_time_format}
    options={[
      { value: "absolute", label: "Date and time · 05-09-26, 13:37" },
      { value: "relative", label: "Time ago · 1 week ago" },
    ]}
    onchange={(value) => prefs.set("history_time_format", value as "absolute" | "relative").catch(e => error = String(e))}
  />
  <p class="hint">Songs appear after listening to more than half, whether or not scrobbling is enabled. Times show when playback started. Hover over a time for the exact date.</p>
  <p class="hint">Songs played since Crescendo moved to ListenBrainz have a white note when ListenBrainz matches them and a grey one when it does not. Older songs have a grey note when Last.fm has a page for them. Right-click a song to open its page, where it has one.</p>
  {#if !connected}<p class="hint">Connect ListenBrainz above to check song availability.</p>{/if}
  <div class="history" bind:this={box} aria-busy={loading}>
    {#each rows as row (row.id)}
      {@const note = mark(row)}
      <button class="history-row" oncontextmenu={(e) => context(e, row)}
        aria-label={`${displayAlbumartist(row.track) ?? "Unknown artist"} - ${displayTitle(row.track)}, ${historyDate(row.played_at)}`}>
        <span class="availability" class:matched={note?.matched} title={note?.title}>
          {#if note}<Icon name="music-2" size={14} />{/if}
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
  .hint { color: var(--txt-faded); font-size: 11px; line-height: 1.5; margin: 8px 0; }
  .history { max-height: 280px; overflow-y: auto; border: 1px solid var(--border); background: var(--bg-panel); }
  .history-row { display: flex; align-items: center; gap: 8px; width: 100%; min-height: 30px; padding: 5px 8px; text-align: left; }
  .history-row:nth-child(even) { background: var(--row-alt); }
  .history-row:hover, .history-row:focus-visible { background: var(--hover-row); }
  .availability { width: 14px; height: 14px; flex: none; color: var(--txt-faded); }
  .availability.matched { color: var(--txt-full); }
  .song { min-width: 0; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  time { color: var(--txt-faded); white-space: nowrap; font-size: 11px; font-variant-numeric: tabular-nums; }
  .empty { padding: 16px; color: var(--txt-faded); }
  .paging { display: flex; justify-content: space-between; align-items: center; margin-top: 8px; color: var(--txt-faded); font-size: 11px; }
  .btn { padding: 4px 10px; border: 1px solid var(--border); background: var(--bg-panel); }
  .btn:hover:enabled { background: var(--hover-row); }
  .btn:disabled { opacity: .4; }
  .error { margin-top: 8px; color: var(--accent); overflow-wrap: anywhere; }
</style>
