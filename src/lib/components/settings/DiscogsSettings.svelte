<script lang="ts">
  import { onMount } from "svelte";
  import { call, isTauri } from "$lib/api";

  /** The Discogs account whose token is saved. The token itself stays in the backend. */
  let username = $state<string | null>(null);
  let token = $state("");
  let busy = $state(false);
  let error = $state("");

  onMount(() => {
    call<string | null>("discogs_status").then((name) => (username = name)).catch((e) => (error = String(e)));
  });

  async function save(value: string) {
    busy = true;
    error = "";
    try {
      username = await call<string | null>("discogs_set_token", { token: value });
      token = "";
    } catch (e) { error = String(e); }
    finally { busy = false; }
  }
</script>

<section aria-label="Discogs">
  <label for="discogs-token">Discogs personal access token</label>
  <div class="row">
    <input id="discogs-token" type="password" autocomplete="off" spellcheck="false" bind:value={token} disabled={busy} />
    <button class="btn" disabled={busy || !token.trim() || !isTauri} onclick={() => save(token.trim())}>
      {busy ? "Checking…" : "Save"}
    </button>
    {#if username}<button class="btn" disabled={busy} onclick={() => save("")}>Remove</button>{/if}
  </div>
  {#if username}<p class="hint">Discogs is asked as <b>{username}</b>.</p>{/if}
  <p class="hint">Discogs answers only signed-in requests, so it is skipped until a token is saved. Create one under Settings > Developers on discogs.com ("Generate new token"). Crescendo stores it in a private file on this computer.</p>
  <p class="hint">This application uses Discogs' API but is not affiliated with, sponsored or endorsed by Discogs. 'Discogs' is a trademark of Zink Media, LLC.</p>
  {#if error}<p class="error" role="status">{error}</p>{/if}
</section>

<style>
  section { margin: 0 0 14px; }
  label { display: block; color: var(--txt-faded); margin-bottom: 4px; font-size: 12px; }
  .row { display: flex; gap: 8px; }
  input { flex: 1; min-width: 0; padding: 6px 8px; background: var(--bg-panel); border: 1px solid var(--border); color: var(--txt); }
  .hint { color: var(--txt-faded); font-size: 11px; line-height: 1.5; margin: 6px 0 0; }
  .btn { border: 1px solid var(--border); padding: 5px 12px; background: var(--bg-panel); flex: none; }
  .btn:hover:enabled { background: var(--hover-row); }
  .btn:disabled { opacity: .5; cursor: default; }
  .error { color: var(--accent); margin-top: 8px; overflow-wrap: anywhere; }
</style>
