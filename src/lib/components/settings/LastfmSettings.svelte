<script lang="ts">
  import { onMount } from "svelte";
  import { call, isTauri } from "$lib/api";
  import { prefs } from "$lib/stores/prefs.svelte";

  interface Status { username: string | null; enabled: boolean; pending: number; error: string | null; authorizing: boolean }
  let status = $state<Status>({ username: null, enabled: false, pending: 0, error: null, authorizing: false });
  let secret = $state("");
  let authorizing = $state(false);
  let busy = $state(false);
  let error = $state("");

  async function refresh() {
    try { status = await call<Status>("lastfm_status"); authorizing = status.authorizing; }
    catch (e) { error = String(e); }
  }
  onMount(() => {
    void refresh();
    const timer = setInterval(() => { if (!busy) void refresh(); }, 15000);
    return () => clearInterval(timer);
  });

  async function run(command: string, args?: Record<string, unknown>) {
    busy = true;
    error = "";
    try {
      await call(command, args);
      if (command === "lastfm_connect") { authorizing = true; secret = ""; }
      if (command === "lastfm_finish_connect" || command === "lastfm_disconnect") authorizing = false;
    } catch (e) { error = String(e); }
    finally { await refresh(); busy = false; }
  }
</script>

<section aria-label="Last.fm scrobbling">
  <h3>Last.fm scrobbling</h3>
  {#if status.username}<p>Connected as <b>{status.username}</b></p>{/if}
  <label class="check">
    <input type="checkbox" checked={status.enabled} disabled={busy || !status.username || !isTauri}
      onchange={(e) => run("lastfm_set_enabled", { enabled: e.currentTarget.checked })} />
    Scrobble listened songs to Last.fm
  </label>
  <p class="hint">Submits recognized tracks using their original tags after half the song or four minutes of listening. Tracks must be longer than 30 seconds. Applies from the next song.</p>
  {#if status.pending}<p class="hint">{status.pending} scrobble{status.pending === 1 ? "" : "s"} waiting to send.</p>{/if}
  {#if status.enabled || status.pending}<p class="hint">Turning scrobbling off discards waiting submissions.</p>{/if}
  {#if authorizing}
    <p class="hint">Authorize Crescendo on Last.fm in your browser, then finish connecting here.</p>
    <button class="btn" disabled={busy} onclick={() => run("lastfm_finish_connect")}>Finish connecting</button>
    <button class="btn" disabled={busy} onclick={() => run("lastfm_disconnect")}>Cancel</button>
  {:else}
    <label for="lfm-secret">Last.fm API shared secret</label>
    <input id="lfm-secret" type="password" autocomplete="off" spellcheck="false" bind:value={secret} disabled={busy} />
    <p class="hint">Use the shared secret belonging to the API key above, from your Last.fm API account. Crescendo stores it and your session in a private file on this computer.</p>
    <button class="btn" disabled={busy || !secret.trim() || !prefs.values.lastfm_api_key || !isTauri}
      onclick={() => run("lastfm_connect", { apiKey: prefs.values.lastfm_api_key.trim(), secret: secret.trim() })}>
      {busy ? "Working…" : status.username ? "Reconnect Last.fm" : "Connect Last.fm"}
    </button>
    {#if status.username}<button class="btn" disabled={busy} onclick={() => run("lastfm_disconnect")}>Disconnect</button>{/if}
  {/if}
  {#if !isTauri}<p class="hint">Account connection is available in the desktop app.</p>{/if}
  {#if error || status.error}<p class="error" role="status">{error || status.error}</p>{/if}
</section>

<style>
  section { margin: 20px 0; border-top: 1px solid var(--border); padding-top: 14px; }
  h3 { font-size: 13px; font-weight: 500; margin-bottom: 10px; }
  p { margin-bottom: 8px; }
  label { display: block; color: var(--txt-faded); margin-bottom: 4px; font-size: 12px; }
  .check { display: flex; align-items: center; gap: 8px; color: var(--txt); margin: 10px 0; }
  input[type="password"] { width: 100%; padding: 6px 8px; background: var(--bg-panel); border: 1px solid var(--border); color: var(--txt); }
  .hint { color: var(--txt-faded); font-size: 11px; line-height: 1.5; margin: 6px 0 10px; }
  .btn { border: 1px solid var(--border); padding: 5px 12px; background: var(--bg-panel); margin-right: 6px; }
  .btn:hover:enabled { background: var(--hover-row); }
  .btn:disabled { opacity: .5; cursor: default; }
  .error { color: var(--accent); margin-top: 8px; overflow-wrap: anywhere; }
</style>
