<script lang="ts">
  import type { Catalog, CatalogEntry } from "$lib/api";
  import { plugins } from "$lib/stores/plugins.svelte";

  let { onresult }: { onresult: (message: string | null, error: string | null) => void } =
    $props();

  let catalog = $state<Catalog | null>(null);
  let loading = $state(false);
  let installing = $state<string | null>(null);
  let error = $state<string | null>(null);

  /** GitHub is contacted only when asked, never when Settings opens. */
  async function load() {
    loading = true;
    error = null;
    try {
      catalog = await plugins.catalog();
    } catch (reason) {
      error = String(reason);
    } finally {
      loading = false;
    }
  }

  async function install(entry: CatalogEntry) {
    installing = entry.id;
    try {
      const result = await plugins.catalogInstall(entry.id);
      onresult(result.message, null);
      await load();
    } catch (reason) {
      onresult(null, String(reason));
    } finally {
      installing = null;
    }
  }

  function action(entry: CatalogEntry): string | null {
    switch (entry.status.state) {
      case "not_installed":
        return "Install";
      case "update_available":
        return `Update to ${entry.version}`;
      default:
        return null;
    }
  }

  function note(entry: CatalogEntry): string | null {
    switch (entry.status.state) {
      case "up_to_date":
        return "Installed";
      case "installed_newer":
        return `Installed ${entry.installed_version}, newer than listed`;
      case "incompatible":
        return entry.status.reason;
      default:
        return null;
    }
  }

  function size(bytes: number): string {
    return bytes < 1024 * 1024
      ? `${Math.max(1, Math.round(bytes / 1024))} KB`
      : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  }
</script>

<section class="catalog" aria-label="Available plugins">
  <div class="catalog-head">
    <div>
      <strong>Available plugins</strong>
      <p class="hint">
        Optional plugins from the Crescendo plugins repository on GitHub. The list is signed
        with Crescendo's release key, and each download is checked against it. Installed
        plugins start disabled.
      </p>
    </div>
    <button class="btn" disabled={loading || installing !== null} onclick={load}>
      {loading ? "Checking…" : catalog ? "Check again" : "Show available plugins"}
    </button>
  </div>

  {#if error}<p class="message error">{error}</p>{/if}

  {#if catalog}
    {#if catalog.plugins.length === 0}
      <p class="empty">No plugins are listed in {catalog.release}.</p>
    {:else}
      <ul class="catalog-list">
        {#each catalog.plugins as entry (entry.id)}
          <li>
            <div class="entry-text">
              <div class="entry-title">
                <span>{entry.name}</span>
                <small>v{entry.version} · {size(entry.size)}</small>
              </div>
              <p>{entry.description}</p>
              {#if entry.permissions.length}
                <p class="hint">
                  Asks for:
                  {#each entry.permissions as permission, i (permission)}<code>{permission}</code
                    >{i < entry.permissions.length - 1 ? ", " : ""}{/each}
                </p>
              {/if}
              {#if note(entry)}<p class="hint">{note(entry)}</p>{/if}
            </div>
            {#if action(entry)}
              <button
                class="btn"
                disabled={installing !== null || loading}
                onclick={() => install(entry)}
              >
                {installing === entry.id ? "Installing…" : action(entry)}
              </button>
            {/if}
          </li>
        {/each}
      </ul>
      <p class="hint release">From release {catalog.release}.</p>
    {/if}
  {/if}
</section>

<style>
  .catalog {
    margin-top: 16px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
  }
  .catalog-head,
  li {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
  }
  .catalog-head .hint { margin: 3px 0 10px; }
  .btn {
    height: 30px;
    flex: none;
    padding: 5px 12px;
    color: var(--txt);
    border: 1px solid var(--border);
  }
  .btn:hover:not(:disabled) { background: var(--hover-row); }
  .btn:disabled { color: var(--txt-faded); cursor: default; }
  .hint { color: var(--txt-faded); font-size: 11px; line-height: 1.45; }
  .catalog-list {
    margin: 0;
    padding: 0;
    list-style: none;
    border: 1px solid var(--border);
  }
  li { padding: 10px; border-bottom: 1px solid var(--hairline); }
  li:last-child { border-bottom: none; }
  .entry-text { min-width: 0; }
  .entry-title { display: flex; align-items: baseline; gap: 8px; }
  .entry-title small { color: var(--txt-faded); font-size: 10px; }
  p { margin: 4px 0 0; font-size: 12px; }
  code { color: var(--txt-faded); font-size: 10px; }
  .message { padding: 7px 9px; font-size: 11px; border: 1px solid var(--border); }
  .message.error { color: var(--search-invalid); }
  .empty { color: var(--txt-faded); padding: 12px 0; text-align: center; }
  .release { margin-top: 6px; }
</style>
