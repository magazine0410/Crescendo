<script lang="ts">
  import { onMount } from "svelte";
  import { call, isTauri } from "$lib/api";

  interface Status { username: string | null; enabled: boolean; suspended: boolean; pending: number; error: string | null }
  /** Told the connected account's name, or null, whenever it is read. */
  let { onaccount }: { onaccount?: (username: string | null) => void } = $props();
  let status = $state<Status>({ username: null, enabled: false, suspended: false, pending: 0, error: null });
  let token = $state("");
  let busy = $state(false);
  let error = $state("");

  async function refresh() {
    try {
      status = await call<Status>("listenbrainz_status");
      onaccount?.(status.suspended ? null : status.username);
    } catch (e) { error = String(e); }
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
      if (command === "listenbrainz_connect") token = "";
    } catch (e) { error = String(e); }
    finally { await refresh(); busy = false; }
  }
</script>

<section aria-label="ListenBrainz">
  <h3>ListenBrainz</h3>
  {#if status.username}<p>Connected as <b>{status.username}</b></p>{/if}
  <label class="check">
    <input type="checkbox" checked={status.enabled} disabled={busy || !status.username || !isTauri}
      onchange={(e) => run("listenbrainz_set_enabled", { enabled: e.currentTarget.checked })} />
    Send listened songs to ListenBrainz
  </label>
  <p class="hint">Sends each song under its original tags after half the song or four minutes of listening. Tracks must be longer than 30 seconds. Applies from the next song.</p>
  {#if status.pending}<p class="hint">{status.pending} listen{status.pending === 1 ? "" : "s"} waiting to send.</p>{/if}
  {#if status.enabled || status.pending}<p class="hint">Turning this off discards waiting listens.</p>{/if}
  <label for="lb-token">ListenBrainz user token</label>
  <input id="lb-token" type="password" autocomplete="off" spellcheck="false" bind:value={token} disabled={busy} />
  <p class="hint">Copy the user token from your ListenBrainz settings page (listenbrainz.org/settings). Crescendo stores it in a private file on this computer. Besides sending listens, it is what lets Crescendo ask ListenBrainz which recording a song is, for Song history and the Now Playing tab.</p>
  <button class="btn" disabled={busy || !token.trim() || !isTauri}
    onclick={() => run("listenbrainz_connect", { token: token.trim() })}>
    {busy ? "Working…" : status.username ? "Connect again" : "Connect ListenBrainz"}
  </button>
  {#if status.username}<button class="btn" disabled={busy} onclick={() => run("listenbrainz_disconnect")}>Disconnect</button>{/if}
  {#if !isTauri}<p class="hint">Account connection is available in the desktop app.</p>{/if}
  {#if error || status.error}<p class="error" role="status">{error || status.error}</p>{/if}
</section>

<style>
  section { margin: 0 0 20px; }
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
