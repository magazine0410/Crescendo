<script lang="ts">
  import { prefs, type CollectionsPlacement } from "$lib/stores/prefs.svelte";
  import { library } from "$lib/stores/library.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { call, isTauri } from "$lib/api";
  import { formatLongDuration } from "$lib/format";

  interface Props {
    open: boolean;
    onclose: () => void;
  }
  let { open, onclose }: Props = $props();

  let keyTest = $state<"idle" | "testing" | "ok" | "bad">("idle");
  let autostart = $state(false);

  /** Ordering follows this pref in SQL (see queries.rs), so the pref has to be
      written before anything re-queries — hence the reload here rather than an
      effect on prefs.values, which would fire while the old value is still
      stored. */
  async function setPreferRomanized(on: boolean) {
    await prefs.set("prefer_romanized", on);
    await library.loadValues();
    await library.loadAlbums();
    await library.reloadTrackView(prefs.values.track_separators);
  }

  /** Same shape as setPreferRomanized: lyrics_for_track reads the pref from
      the DB, so the write has to land before the Now playing pane asks again. */
  async function setLyricsOnline(on: boolean) {
    await prefs.set("lyrics_online", on);
    ui.lyricsEpoch++;
  }

  // Reflect the actual on-disk autostart state whenever the dialog opens.
  $effect(() => {
    if (open) {
      call<boolean>("autostart_get")
        .then((v) => (autostart = v))
        .catch(() => {});
    }
  });

  async function toggleAutostart(enabled: boolean) {
    autostart = enabled;
    try {
      await call("autostart_set", { enabled });
    } catch {
      autostart = !enabled; // revert on failure
    }
  }

  async function browseLibrary() {
    if (!isTauri) return;
    const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
    const dir = await openDialog({ directory: true });
    if (typeof dir === "string") await prefs.set("library_path", dir);
  }

  async function testKey() {
    keyTest = "testing";
    try {
      const ok = await call<boolean>("lastfm_test_key", {
        apiKey: prefs.values.lastfm_api_key,
      });
      keyTest = ok ? "ok" : "bad";
    } catch {
      keyTest = "bad";
    }
  }

  async function openAbout() {
    if (isTauri) {
      await call("open_about_window");
    } else {
      window.open("/about", "crescendo-about", "width=500,height=600");
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
  }
</script>

<svelte:window onkeydown={open ? onKeydown : undefined} />

{#if open}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
    <div class="dialog" role="dialog" aria-label="Settings">
      <header>
        <span>Settings</span>
        <button class="x" onclick={onclose}>✕</button>
      </header>

      <section>
        <h3>General</h3>
        <label class="check">
          <input
            type="checkbox"
            checked={autostart}
            onchange={(e) => toggleAutostart(e.currentTarget.checked)}
          />
          Start when computer starts
        </label>

        <label for="deftab">Default tab</label>
        <select
          id="deftab"
          value={prefs.values.default_tab}
          onchange={(e) =>
            prefs.set("default_tab", e.currentTarget.value as "library" | "playlists")}
        >
          <option value="library">Library</option>
          <option value="playlists">Playlists</option>
        </select>
        <p class="hint">Which tab opens when Crescendo starts.</p>
      </section>

      <section>
        <h3>Library</h3>
        <label for="libpath">Music folder</label>
        <div class="pathrow">
          <input
            id="libpath"
            type="text"
            value={prefs.values.library_path}
            onchange={(e) => prefs.set("library_path", e.currentTarget.value)}
          />
          <button class="btn" onclick={browseLibrary}>Browse…</button>
        </div>

        {#if library.scanning}
          {@const scanned = library.scanProgress?.scanned ?? 0}
          {@const total = library.stats?.tracks ?? 0}
          {@const pct = total > 0 ? Math.min(100, Math.round((scanned / total) * 100)) : null}
          <div class="scanprogress">
            <div class="track" class:indeterminate={pct === null}>
              <div class="fill" style:width={pct !== null ? `${pct}%` : undefined}></div>
            </div>
            <div class="pct">
              {pct !== null ? `${pct}%` : `${scanned.toLocaleString()} files scanned`}
            </div>
          </div>
        {/if}

        <div class="scanrow">
          <button class="btn" disabled={library.scanning} onclick={() => library.startScan(false)}>
            Scan for changes
          </button>
          <button class="btn" disabled={library.scanning} onclick={() => library.startScan(true)}>
            Full rescan
          </button>
          {#if library.scanning}
            <button class="btn" onclick={() => library.cancelScan()}>Cancel</button>
          {/if}
        </div>
        {#if library.stats}
          <p class="stats">
            {library.stats.tracks.toLocaleString()} tracks ·
            {library.stats.albums.toLocaleString()} albums ·
            {formatLongDuration(library.stats.total_duration_ms)}
          </p>
        {/if}
        <label for="threads">Scan threads (tag readers)</label>
        <input
          id="threads"
          class="short"
          type="number"
          min="1"
          max="16"
          value={prefs.values.scan_threads}
          onchange={(e) => prefs.set("scan_threads", parseInt(e.currentTarget.value, 10) || 4)}
        />
      </section>

      <section>
        <h3>Playlists</h3>
        <label for="collplacement">Collections</label>
        <select
          id="collplacement"
          value={prefs.values.collections_placement}
          onchange={(e) =>
            prefs.set("collections_placement", e.currentTarget.value as CollectionsPlacement)}
        >
          <option value="above">Above the playlist list, with a separator</option>
          <option value="within">Within the playlist list</option>
          <option value="below">Below the playlist list, with a separator</option>
        </select>
        <p class="hint">
          Where grouped playlists appear in the sidebar. Group playlists by
          right-clicking them and choosing "Set collection…".
        </p>
      </section>

      <section>
        <h3>Display</h3>
        <label for="libview">Library view</label>
        <select
          id="libview"
          value={prefs.values.library_view}
          onchange={(e) =>
            prefs.set("library_view", e.currentTarget.value as "album_wall" | "track_list")}
        >
          <option value="album_wall">Album wall</option>
          <option value="track_list">Track list</option>
        </select>
        <p class="hint">
          Album wall shows a grid of album covers; Track list shows every track as a
          flat, scrollable list. Both respect the left panel and search.
        </p>

        <label class="check" class:disabled={prefs.values.library_view !== "track_list"}>
          <input
            type="checkbox"
            checked={prefs.values.track_separators}
            disabled={prefs.values.library_view !== "track_list"}
            onchange={(e) => prefs.set("track_separators", e.currentTarget.checked)}
          />
          Show clickable album separators in the track list
        </label>

        <label class="check">
          <input
            type="checkbox"
            checked={prefs.values.show_waveform}
            onchange={(e) => prefs.set("show_waveform", e.currentTarget.checked)}
          />
          Show loudness waveform under the seek bar
        </label>

        <label class="check">
          <input
            type="checkbox"
            checked={prefs.values.prefer_romanized}
            onchange={(e) => setPreferRomanized(e.currentTarget.checked)}
          />
          Prefer romanized names where available
        </label>
        <p class="hint">
          Shows the romanized titlesort / artistsort / albumartistsort tag wherever a
          title or artist that contains Japanese/CJK text renders — track lists,
          albums, the playlist, the queue and the Now playing tab, plus %title%,
          %artist% and %albumartist% in the formats below. Sorting follows it too, so
          a row sits where its visible name says it should. The Now playing tab keeps
          its "Original title" row. Turn this off to always show the original tags.
        </p>

        <label for="fmt">Now-playing format</label>
        <input
          id="fmt"
          type="text"
          value={prefs.values.titlebar_format}
          onchange={(e) => prefs.set("titlebar_format", e.currentTarget.value)}
        />
        <p class="hint">
          %tag% placeholders: albumartist, artist, title, album, grouping, genre,
          tracknumber, discnumber, year, length, codec, titlesort, artistsort,
          albumartistsort. The three *sort tags print the file's sort tag exactly as
          stored — the tick box does not change them, and they show nothing on tracks
          without one. [brackets] render only when every tag inside resolves.
        </p>

        <label for="fmt2">Now-playing format 2</label>
        <input
          id="fmt2"
          type="text"
          value={prefs.values.titlebar_format_2}
          onchange={(e) => prefs.set("titlebar_format_2", e.currentTarget.value)}
        />
        <p class="hint">
          An optional smaller second line in the transport bar, using the same
          %tag% placeholders and [bracket] rules. Leave blank to keep the current
          single-line layout. It appears when the transport bar is tall enough.
        </p>

        <label for="rowfmt">Track detail (shown left of the duration)</label>
        <input
          id="rowfmt"
          type="text"
          value={prefs.values.track_detail_format}
          onchange={(e) => prefs.set("track_detail_format", e.currentTarget.value)}
        />
        <p class="hint">
          Same %tag% placeholders as above (e.g. %grouping%, %genre%, %codec%, %year%).
          Leave blank to hide. [brackets] render only when every tag inside resolves.
          Long values are truncated with an ellipsis; hover to see the full text.
        </p>

        <label for="rowfmt2">Track detail 2 (shown under the first track detail)</label>
        <input
          id="rowfmt2"
          type="text"
          value={prefs.values.track_detail_format_2}
          onchange={(e) => prefs.set("track_detail_format_2", e.currentTarget.value)}
        />
        <p class="hint">
          A smaller second line under the track detail, right-aligned to the duration.
          Same %tag% placeholders. Leave blank to keep rows single-line.
        </p>
      </section>

      <section>
        <h3>Now playing</h3>
        <label for="lfmkey">Last.fm API key (get one at last.fm/api)</label>
        <div class="pathrow">
          <input
            id="lfmkey"
            type="text"
            spellcheck="false"
            value={prefs.values.lastfm_api_key}
            onchange={(e) => {
              prefs.set("lastfm_api_key", e.currentTarget.value.trim());
              keyTest = "idle";
            }}
          />
          <button class="btn" disabled={keyTest === "testing" || !prefs.values.lastfm_api_key} onclick={testKey}>
            {keyTest === "testing" ? "Testing…" : "Test key"}
          </button>
        </div>
        {#if keyTest === "ok"}<p class="ok">Key works.</p>{/if}
        {#if keyTest === "bad"}<p class="bad">Key rejected by Last.fm.</p>{/if}
        <p class="hint">Without a key, Now Playing falls back to local tags.</p>

        <label class="check">
          <input
            type="checkbox"
            checked={prefs.values.lyrics_online}
            onchange={(e) => setLyricsOnline(e.currentTarget.checked)}
          />
          Look up lyrics online
        </label>
        <p class="hint">
          Sends the playing track's title, artist, album and duration to VocaDB and
          LRCLIB when there is no sidecar .lrc/.txt file or embedded lyrics tag. Off
          keeps lyrics to those local sources only.
        </p>
      </section>

      <section>
        <h3>Visualizer</h3>
        <p class="hint">
          Change the bar color from the swatch in the Visualizer tab. More options coming here later.
        </p>
      </section>

      <section>
        <h3>About</h3>
        <button class="btn" onclick={openAbout}>About Crescendo</button>
      </section>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 300;
  }
  .dialog {
    width: 520px;
    max-height: 80vh;
    overflow-y: auto;
    background: var(--bg-main);
    border: 1px solid var(--popup-border);
    box-shadow: 0 8px 40px rgba(0, 0, 0, 0.7);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 18px;
    border-bottom: 1px solid var(--hairline);
    font-weight: 600;
  }
  .x {
    color: var(--txt-faded);
  }
  .x:hover {
    color: var(--txt);
  }
  section {
    padding: 14px 18px;
    border-bottom: 1px solid var(--sidesline);
  }
  h3 {
    font-size: 12px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--txt-faded);
    margin-bottom: 10px;
  }
  label {
    display: block;
    color: var(--txt-faded);
    margin-bottom: 4px;
    font-size: 12px;
  }
  input {
    width: 100%;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    padding: 6px 8px;
    color: var(--txt);
    margin-bottom: 8px;
  }
  select {
    width: 100%;
    background-color: var(--bg-panel);
    border: 1px solid var(--border);
    padding: 6px 26px 6px 8px;
    color: var(--txt);
    margin-bottom: 8px;
    /* WebKitGTK renders a native (light) combobox unless appearance is cleared */
    appearance: none;
    -webkit-appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%236e6e6e' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 8px center;
    cursor: pointer;
  }
  select option {
    background: var(--bg-main);
    color: var(--txt);
  }
  input.short {
    width: 80px;
  }
  label.check {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--txt);
    font-size: 12px;
    margin-bottom: 12px;
    cursor: pointer;
  }
  label.check input {
    width: auto;
    margin: 0;
    flex: none;
    accent-color: var(--txt-full);
    cursor: pointer;
  }
  label.check.disabled {
    color: var(--txt-faded);
    cursor: default;
  }
  label.check.disabled input {
    cursor: default;
  }
  .pathrow {
    display: flex;
    gap: 8px;
  }
  .pathrow input {
    flex: 1;
  }
  .btn {
    border: 1px solid var(--border);
    padding: 5px 12px;
    color: var(--txt);
    flex: none;
    height: 30px;
  }
  .btn:hover:not(:disabled) {
    background: var(--hover-row);
  }
  .btn:disabled {
    color: var(--txt-faded);
    cursor: default;
  }
  .scanrow {
    display: flex;
    gap: 8px;
    margin: 6px 0;
  }
  .scanprogress {
    margin: 8px 0;
  }
  .scanprogress .track {
    height: 6px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.12);
    overflow: hidden;
  }
  .scanprogress .fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
    transition: width 0.2s ease;
  }
  /* First-ever scan: no baseline count, so animate an indeterminate sweep. */
  .scanprogress .track.indeterminate .fill {
    width: 35%;
    animation: scan-sweep 1.1s ease-in-out infinite;
  }
  @keyframes scan-sweep {
    0% { margin-left: -35%; }
    100% { margin-left: 100%; }
  }
  .scanprogress .pct {
    color: var(--txt-faded);
    font-size: 11px;
    margin-top: 4px;
    text-align: center;
  }
  .stats {
    color: var(--txt-faded);
    font-size: 11px;
    margin-bottom: 8px;
  }
  .hint {
    color: var(--txt-faded);
    font-size: 11px;
    line-height: 1.5;
    margin-bottom: 14px;
  }
  .ok {
    color: #5fbf5f;
    font-size: 11px;
  }
  .bad {
    color: var(--search-invalid);
    font-size: 11px;
  }
</style>
