<script lang="ts">
  import { library } from "$lib/stores/library.svelte";

  const scan = $derived(library.scan);
  const p = $derived(scan.phase === "running" || scan.phase === "cancelling" ? scan.progress : null);
</script>

{#if scan.phase === "failed"}
  <div class="strip error">
    <span>Scan failed: {scan.message}</span>
    <button onclick={() => library.dismissScanFailure()}>✕</button>
  </div>
{:else if scan.phase === "finished"}
  <div class="strip">
    <span>
      {scan.progress.cancelled ? "Scan cancelled" : "Scan complete"} — {scan.progress.scanned.toLocaleString()} files
      ({scan.progress.added} added, {scan.progress.updated} updated, {scan.progress.removed} removed{scan.progress.errors
        ? `, ${scan.progress.errors} errors`
        : ""})
    </span>
  </div>
{:else if scan.phase === "running" || scan.phase === "cancelling"}
  <div class="strip">
    <span class="busy">
      {#if p}
        {scan.phase === "cancelling" ? "Cancelling…" : "Scanning…"} {p.scanned.toLocaleString()} files
        ({p.added} added, {p.updated} updated)
        <span class="path">{p.current_path}</span>
      {:else}
        {scan.phase === "cancelling" ? "Cancelling…" : "Scanning…"}
      {/if}
    </span>
    <button disabled={scan.phase === "cancelling"} onclick={() => library.cancelScan()}>Cancel</button>
  </div>
{/if}

<style>
  .strip {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 5px 14px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--hairline);
    color: var(--txt-faded);
    font-size: 11px;
    flex: none;
  }
  .strip.error {
    color: var(--search-invalid);
  }
  .busy {
    display: flex;
    gap: 10px;
    min-width: 0;
    align-items: baseline;
  }
  .path {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 45vw;
    color: var(--txt-superfaded);
  }
  button {
    color: var(--txt-faded);
    flex: none;
  }
  button:hover {
    color: var(--txt);
  }
</style>
