<script lang="ts">
  import { prefs } from "$lib/stores/prefs.svelte";
  import { library } from "$lib/stores/library.svelte";
  import { call, isTauri } from "$lib/api";
  import { formatLongDuration } from "$lib/format";

  interface Props {
    open: boolean;
    onclose: () => void;
  }
  let { open, onclose }: Props = $props();

  let keyTest = $state<"idle" | "testing" | "ok" | "bad">("idle");

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
        <h3>Display</h3>
        <label class="check">
          <input
            type="checkbox"
            checked={prefs.values.show_waveform}
            onchange={(e) => prefs.set("show_waveform", e.currentTarget.checked)}
          />
          Show loudness waveform under the seek bar
        </label>

        <label for="fmt">Now-playing title format</label>
        <input
          id="fmt"
          type="text"
          value={prefs.values.titlebar_format}
          onchange={(e) => prefs.set("titlebar_format", e.currentTarget.value)}
        />
        <p class="hint">
          %tag% placeholders: albumartist, artist, title, album, grouping, genre,
          tracknumber, discnumber, year, length, codec, titlesort, artistsort,
          albumartistsort. The _display variants (title_display, artist_display,
          albumartist_display) automatically substitute the romanized sort tag when
          the original contains Japanese/CJK text. [brackets] render only when every
          tag inside resolves.
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
      </section>

      <section>
        <h3>Last.fm</h3>
        <label for="lfmkey">API key (get one at last.fm/api)</label>
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
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--txt-faded);
    margin-bottom: 10px;
  }
  label {
    display: block;
    color: var(--txt-faded);
    margin-bottom: 4px;
    font-size: 11px;
  }
  input {
    width: 100%;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    padding: 6px 8px;
    color: var(--txt);
    margin-bottom: 8px;
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
  .stats {
    color: var(--txt-faded);
    font-size: 11px;
    margin-bottom: 8px;
  }
  .hint {
    color: var(--txt-superfaded);
    font-size: 11px;
    line-height: 1.5;
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
