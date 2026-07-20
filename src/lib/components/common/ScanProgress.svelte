<script lang="ts">
  import { library } from "$lib/stores/library.svelte";

  const p = $derived(library.scanProgress);
</script>

{#if p || library.scanError}
  <div class="strip" class:error={!!library.scanError}>
    {#if library.scanError}
      <span>Scan failed: {library.scanError}</span>
      <button onclick={() => (library.scanError = null)}>✕</button>
    {:else if p}
      {#if p.done}
        <span>
          Scan complete — {p.scanned.toLocaleString()} files
          ({p.added} added, {p.updated} updated, {p.removed} removed{p.errors
            ? `, ${p.errors} errors`
            : ""})
        </span>
      {:else}
        <span class="busy">
          Scanning… {p.scanned.toLocaleString()} files
          ({p.added} added, {p.updated} updated)
          <span class="path">{p.current_path}</span>
        </span>
        <button onclick={() => library.cancelScan()}>Cancel</button>
      {/if}
    {/if}
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
