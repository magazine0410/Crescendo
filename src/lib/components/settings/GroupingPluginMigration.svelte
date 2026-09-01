<script lang="ts">
  import { call } from "$lib/api";
  import { plugins } from "$lib/stores/plugins.svelte";

  let busy = $state(false);
  let error = $state<string | null>(null);
  let scheduled = $state(false);
  const pending = $derived(plugins.snapshot?.grouping_migration_pending ?? false);

  async function act(action: "install" | "dismiss") {
    busy = true;
    error = null;
    try {
      await plugins.groupingMigration(action);
      scheduled = action === "install";
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }
</script>

{#if pending || scheduled}
  <aside class="migration" aria-live="polite">
    {#if scheduled}
      <div>
        <strong>Primary Grouping is ready.</strong>
        <span>Restart Crescendo to enable the optional grouping behavior.</span>
      </div>
      <button class="primary" onclick={() => call("plugin_restart_app")}>Restart Crescendo</button>
    {:else}
      <div>
        <strong>Grouping behavior is now an optional plugin.</strong>
        <span>
          Install Primary Grouping to keep collapsing semicolon-separated titles under their
          first grouping.
        </span>
        {#if error}<span class="error">{error}</span>{/if}
      </div>
      <button class="primary" disabled={busy} onclick={() => act("install")}>Install plugin</button>
      <button disabled={busy} onclick={() => act("dismiss")}>Don’t offer again</button>
    {/if}
  </aside>
{/if}

<style>
  .migration {
    position: absolute;
    z-index: 40;
    top: 44px;
    left: 50%;
    transform: translateX(-50%);
    width: min(720px, calc(100% - 32px));
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    background: var(--bg-panel);
    border: 1px solid var(--accent);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
  }
  .migration > div {
    display: flex;
    flex: 1;
    min-width: 0;
    flex-direction: column;
    gap: 3px;
  }
  .migration span {
    color: var(--txt-faded);
    font-size: 12px;
  }
  .migration .error {
    color: #f39a82;
  }
  button {
    flex: none;
    padding: 5px 10px;
    border: 1px solid var(--border);
  }
  button.primary {
    border-color: var(--accent);
    color: var(--txt-full);
  }
</style>
