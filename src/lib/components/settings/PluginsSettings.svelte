<script lang="ts">
  import {
    call,
    isTauri,
    type PluginInfo,
    type PluginMutationResult,
    type PluginSetting,
    type PluginSnapshot,
  } from "$lib/api";

  let snapshot = $state<PluginSnapshot | null>(null);
  let selectedId = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let confirmingUninstall = $state(false);
  let removeData = $state(false);

  const selected = $derived(
    snapshot?.plugins.find((plugin) => plugin.id === selectedId) ?? snapshot?.plugins[0] ?? null,
  );

  $effect(() => {
    refresh();
  });

  async function refresh() {
    try {
      snapshot = await call<PluginSnapshot>("plugin_list");
      if (!snapshot.plugins.some((plugin) => plugin.id === selectedId)) {
        selectedId = snapshot.plugins[0]?.id ?? null;
      }
    } catch (reason) {
      error = String(reason);
    }
  }

  async function run(action: () => Promise<PluginMutationResult>) {
    busy = true;
    error = null;
    notice = null;
    try {
      const result = await action();
      snapshot = result.snapshot;
      notice = result.message;
      if (!snapshot.plugins.some((plugin) => plugin.id === selectedId)) {
        selectedId = snapshot.plugins[0]?.id ?? null;
      }
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }

  async function install() {
    await run(() => call<PluginMutationResult>("plugin_choose_install"));
  }

  async function setEnabled(plugin: PluginInfo, enabled: boolean) {
    await run(() =>
      call<PluginMutationResult>("plugin_set_enabled", {
        pluginId: plugin.id,
        enabled,
      }),
    );
  }

  async function setSetting(plugin: PluginInfo, setting: PluginSetting, value: unknown) {
    await run(() =>
      call<PluginMutationResult>("plugin_set_setting", {
        pluginId: plugin.id,
        key: setting.key,
        value,
      }),
    );
  }

  async function uninstall(plugin: PluginInfo) {
    await run(() =>
      call<PluginMutationResult>("plugin_uninstall", {
        pluginId: plugin.id,
        removeData,
      }),
    );
    confirmingUninstall = false;
    removeData = false;
  }

  async function restart() {
    error = null;
    try {
      await call("plugin_restart_app");
      if (!isTauri) await refresh();
    } catch (reason) {
      error = String(reason);
    }
  }

  function status(plugin: PluginInfo): string {
    if (plugin.pending?.operation === "uninstall") return "Pending removal";
    if (plugin.pending?.operation === "update") return `Update ${plugin.pending.target_version}`;
    if (plugin.error) return "Error";
    if (!plugin.compatible) return "Incompatible";
    if (plugin.loaded && !plugin.desired_enabled) return "Disables after restart";
    if (!plugin.loaded && plugin.desired_enabled) return "Enables after restart";
    return plugin.loaded ? "Enabled" : "Disabled";
  }
</script>

<div class="plugin-toolbar">
  <div>
    <strong>Plugins</strong>
    <p class="hint">Install optional Crescendo features from local, declarative packages.</p>
  </div>
  <button class="btn" disabled={busy} onclick={install}>Install…</button>
</div>

{#if snapshot?.safe_mode}
  <div class="banner warning">
    <strong>Plugin safe mode</strong>
    <span>{snapshot.safe_mode_reason}</span>
  </div>
{/if}

{#if snapshot?.restart_required}
  <div class="banner restart">
    <span>Restart Crescendo to apply plugin enablement or package changes.</span>
    <button class="btn" disabled={busy} onclick={restart}>Restart Crescendo</button>
  </div>
{/if}

{#if error}<p class="message error">{error}</p>{/if}
{#if notice}<p class="message ok">{notice}</p>{/if}

{#if snapshot === null}
  <p class="empty">Loading plugins…</p>
{:else if snapshot.plugins.length === 0}
  <p class="empty">No plugins are installed.</p>
{:else}
  <div class="plugin-layout">
    <div class="plugin-list" role="listbox" aria-label="Installed plugins">
      {#each snapshot.plugins as plugin (plugin.id)}
        <button
          class="plugin-row"
          class:selected={selected?.id === plugin.id}
          role="option"
          aria-selected={selected?.id === plugin.id}
          onclick={() => {
            selectedId = plugin.id;
            confirmingUninstall = false;
            removeData = false;
          }}
        >
          <span>{plugin.name}</span>
          <small>{status(plugin)}</small>
        </button>
      {/each}
    </div>

    {#if selected}
      <section class="plugin-detail" aria-label={`${selected.name} details`}>
        <div class="title-row">
          <div>
            <h3>{selected.name}</h3>
            <code>{selected.id}</code>
          </div>
          <span class="version">{selected.version ? `v${selected.version}` : "Invalid"}</span>
        </div>

        {#if selected.description}<p>{selected.description}</p>{/if}
        <dl>
          {#if selected.authors.length}<dt>Author</dt><dd>{selected.authors.join(", ")}</dd>{/if}
          {#if selected.license}<dt>License</dt><dd>{selected.license}</dd>{/if}
          {#if selected.kind}<dt>Kind</dt><dd>{selected.kind}</dd>{/if}
          {#if selected.api_version}<dt>Plugin API</dt><dd>{selected.api_version}</dd>{/if}
          {#if selected.package_hash}<dt>SHA-256</dt><dd><code>{selected.package_hash}</code></dd>{/if}
        </dl>

        {#if selected.compatibility_error}
          <p class="message warning">{selected.compatibility_error}</p>
        {/if}
        {#if selected.error}<p class="message error">{selected.error}</p>{/if}

        <div class="actions">
          <button
            class="btn"
            disabled={busy || !selected.compatible || selected.pending?.operation === "uninstall"}
            onclick={() => setEnabled(selected, !selected.desired_enabled)}
          >
            {selected.desired_enabled ? "Disable" : "Enable"}
          </button>
          <button
            class="btn danger"
            disabled={busy || selected.pending?.operation === "uninstall"}
            onclick={() => (confirmingUninstall = true)}
          >
            Uninstall…
          </button>
        </div>

        {#if confirmingUninstall}
          <div class="confirm">
            <p>Uninstall {selected.name}?</p>
            <label class="check">
              <input type="checkbox" bind:checked={removeData} />
              Also remove this plugin's saved settings
            </label>
            <div class="actions">
              <button class="btn danger" disabled={busy} onclick={() => uninstall(selected)}>Uninstall</button>
              <button class="btn" disabled={busy} onclick={() => (confirmingUninstall = false)}>Cancel</button>
            </div>
          </div>
        {/if}

        {#if selected.settings.length}
          <h4>Settings</h4>
          <div class="plugin-settings">
            {#each selected.settings as setting (setting.key)}
              {#if setting.type === "boolean"}
                <label class="check">
                  <input
                    type="checkbox"
                    checked={setting.value}
                    disabled={busy}
                    onchange={(event) => setSetting(selected, setting, event.currentTarget.checked)}
                  />
                  {setting.label}
                </label>
              {:else if setting.type === "select"}
                <label for={`plugin-${selected.id}-${setting.key}`}>{setting.label}</label>
                <select
                  id={`plugin-${selected.id}-${setting.key}`}
                  value={setting.value}
                  disabled={busy}
                  onchange={(event) => setSetting(selected, setting, event.currentTarget.value)}
                >
                  {#each setting.options as option (option.value)}
                    <option value={option.value}>{option.label}</option>
                  {/each}
                </select>
              {:else if setting.type === "number"}
                <label for={`plugin-${selected.id}-${setting.key}`}>{setting.label}</label>
                <input
                  id={`plugin-${selected.id}-${setting.key}`}
                  type="number"
                  value={setting.value}
                  min={setting.min ?? undefined}
                  max={setting.max ?? undefined}
                  step={setting.step ?? "any"}
                  disabled={busy}
                  onchange={(event) => setSetting(selected, setting, event.currentTarget.valueAsNumber)}
                />
              {:else}
                <label for={`plugin-${selected.id}-${setting.key}`}>{setting.label}</label>
                <input
                  id={`plugin-${selected.id}-${setting.key}`}
                  type="text"
                  value={setting.value}
                  minlength={setting.min_length ?? undefined}
                  maxlength={setting.max_length ?? undefined}
                  disabled={busy}
                  onchange={(event) => setSetting(selected, setting, event.currentTarget.value)}
                />
              {/if}
              {#if setting.description}<p class="hint">{setting.description}</p>{/if}
            {/each}
          </div>
        {/if}
      </section>
    {/if}
  </div>
{/if}

{#if snapshot?.errors.length}
  <details class="diagnostics">
    <summary>Plugin diagnostics ({snapshot.errors.length})</summary>
    {#each snapshot.errors as diagnostic}<p>{diagnostic}</p>{/each}
  </details>
{/if}

<style>
  .btn {
    height: 30px;
    flex: none;
    padding: 5px 12px;
    color: var(--txt);
    border: 1px solid var(--border);
  }
  .btn:hover:not(:disabled) { background: var(--hover-row); }
  .btn:disabled { color: var(--txt-faded); cursor: default; }
  label {
    display: block;
    margin-bottom: 4px;
    color: var(--txt-faded);
    font-size: 12px;
  }
  input,
  select {
    width: 100%;
    margin-bottom: 8px;
    padding: 6px 8px;
    color: var(--txt);
    background: var(--bg-panel);
    border: 1px solid var(--border);
  }
  select { appearance: none; }
  label.check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 10px;
    color: var(--txt);
  }
  label.check input { width: auto; margin: 0; accent-color: var(--txt-full); }
  .hint { color: var(--txt-faded); font-size: 11px; line-height: 1.45; }
  .plugin-toolbar,
  .title-row,
  .actions,
  .banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  .plugin-toolbar .hint { margin: 3px 0 12px; }
  .banner {
    margin-bottom: 12px;
    padding: 9px 10px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    font-size: 11px;
  }
  .banner.warning { border-color: var(--search-invalid); }
  .banner span { line-height: 1.4; }
  .plugin-layout {
    display: grid;
    grid-template-columns: 175px minmax(0, 1fr);
    min-height: 310px;
    border: 1px solid var(--border);
  }
  .plugin-list { border-right: 1px solid var(--border); background: var(--bg-panel); }
  .plugin-row {
    width: 100%;
    padding: 9px 10px;
    text-align: left;
    color: var(--txt);
    border-bottom: 1px solid var(--hairline);
  }
  .plugin-row:hover,
  .plugin-row.selected { background: var(--hover-row); }
  .plugin-row span,
  .plugin-row small { display: block; overflow: hidden; text-overflow: ellipsis; }
  .plugin-row small { margin-top: 2px; color: var(--txt-faded); font-size: 10px; }
  .plugin-detail { min-width: 0; padding: 12px; }
  h3, h4, p { margin: 0 0 9px; }
  h3 { font-size: 14px; }
  h4 { margin-top: 18px; font-size: 12px; }
  code { color: var(--txt-faded); font-size: 10px; overflow-wrap: anywhere; }
  .version { color: var(--txt-faded); font-size: 11px; }
  dl { display: grid; grid-template-columns: 65px minmax(0, 1fr); gap: 3px 7px; font-size: 11px; }
  dt { color: var(--txt-faded); }
  dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
  .actions { justify-content: flex-start; margin-top: 12px; }
  .confirm { margin-top: 10px; padding: 10px; background: var(--bg-panel); border: 1px solid var(--border); }
  .plugin-settings { border-top: 1px solid var(--hairline); padding-top: 10px; }
  .message { padding: 7px 9px; font-size: 11px; border: 1px solid var(--border); }
  .message.error { color: var(--search-invalid); }
  .message.warning { color: var(--txt); }
  .message.ok { color: #5fbf5f; }
  .empty { color: var(--txt-faded); padding: 24px 0; text-align: center; }
  .diagnostics { margin-top: 12px; color: var(--txt-faded); font-size: 10px; }
  .diagnostics p { margin: 5px 0; overflow-wrap: anywhere; }
  .btn.danger { color: var(--search-invalid); }
</style>
