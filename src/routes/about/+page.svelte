<script lang="ts">
  import { onMount } from "svelte";
  import {
    call,
    isTauri,
    type AboutInfo,
    type UpdateCheck,
    type UpdateInstallResult,
  } from "$lib/api";

  let info = $state<AboutInfo | null>(null);
  let failed = $state(false);
  let update = $state<UpdateCheck | null>(null);
  let updateState = $state<"idle" | "checking" | "installing" | "installed">("idle");
  let updateError = $state("");
  let installedResult = $state<UpdateInstallResult | null>(null);

  onMount(() => {
    call<AboutInfo>("about_info")
      .then((value) => (info = value))
      .catch(() => (failed = true));
  });

  async function checkForUpdates() {
    updateState = "checking";
    updateError = "";
    installedResult = null;
    try {
      update = await call<UpdateCheck>("update_check");
    } catch (error) {
      update = null;
      updateError = String(error);
    } finally {
      updateState = "idle";
    }
  }

  async function installUpdate() {
    if (!update?.latest_version) return;
    updateState = "installing";
    updateError = "";
    try {
      installedResult = await call<UpdateInstallResult>("update_install", {
        version: update.latest_version,
      });
      updateState = "installed";
    } catch (error) {
      updateError = String(error);
      updateState = "idle";
    }
  }

  async function closeWindow() {
    if (isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().close();
    } else {
      window.close();
    }
  }

  async function openRepository(e: MouseEvent) {
    e.preventDefault();
    if (!info) return;
    if (isTauri) {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(info.repository_url);
    } else {
      window.open(info.repository_url, "_blank", "noopener,noreferrer");
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") void closeWindow();
  }
</script>

<svelte:window onkeydown={onKeydown} />

<main class="about">
  <header>
    <img src="/favicon.png" alt="" />
    <div>
      <h1>Crescendo</h1>
      <p class="version">Version {info?.version ?? "…"}</p>
    </div>
  </header>

  <p class="summary">A fast music player for large local libraries.</p>

  <section class="updates" aria-live="polite">
    <h2>Updates</h2>

    {#if update?.available && update.latest_version && updateState !== "installed"}
      <div class="update-card">
        <div class="update-heading">
          <div>
            <strong>Crescendo {update.latest_version} is available</strong>
            <p>{update.package_name}</p>
          </div>
          <button class="primary" disabled={updateState === "installing"} onclick={installUpdate}>
            {updateState === "installing" ? "Updating…" : "Update"}
          </button>
        </div>

        {#if update.release_notes}
          <div class="release-notes">{update.release_notes}</div>
        {:else}
          <p class="muted">No release notes were provided.</p>
        {/if}

        {#if update.downloaded}
          <p class="cached">The update package is already downloaded and will be reused.</p>
        {/if}
        {#if updateState === "installing"}
          <p class="progress">
            Downloading and installing the update. Your system will ask for the administrator
            password when it is needed.
          </p>
        {/if}
      </div>
    {:else if updateState === "installed" && installedResult}
      <div class="update-card success">
        <strong>Crescendo {installedResult.version} was installed.</strong>
        <p>Restart Crescendo to use the new version.</p>
        {#if !installedResult.package_removed}
          <p class="muted">The installer package could not be removed from the update cache.</p>
        {/if}
      </div>
    {:else if update && !update.available}
      <p class="up-to-date">Crescendo {update.current_version} is up to date.</p>
    {:else}
      <p class="muted">Check GitHub Releases for a newer version of Crescendo.</p>
    {/if}

    {#if updateError}
      <p class="error">{updateError}</p>
    {/if}
  </section>

  {#if failed}
    <p class="error">Version details could not be loaded.</p>
  {:else if info}
    <section>
      <h2>Core dependencies</h2>
      <div class="dependencies">
        {#each info.dependencies as dependency (dependency.name)}
          <span>{dependency.name}</span>
          <code>{dependency.version}</code>
        {/each}
      </div>
    </section>

    <section>
      <h2>Project</h2>
      <a href={info.repository_url} onclick={openRepository}>{info.repository_url}</a>
    </section>
  {/if}

  <footer>
    <button disabled={updateState === "checking" || updateState === "installing"} onclick={checkForUpdates}>
      {updateState === "checking" ? "Checking…" : "Check for updates"}
    </button>
    <span></span>
    <button onclick={closeWindow}>Close</button>
  </footer>
</main>

<style>
  .about {
    height: 100vh;
    overflow-y: auto;
    padding: 24px 28px 18px;
    user-select: text;
  }
  header {
    display: flex;
    align-items: center;
    gap: 16px;
    margin-bottom: 14px;
  }
  header img {
    width: 64px;
    height: 64px;
  }
  h1 {
    font-size: 24px;
    font-weight: 400;
  }
  .version,
  .summary {
    color: var(--txt-faded);
  }
  .summary {
    line-height: 1.5;
    margin-bottom: 20px;
  }
  section {
    margin-bottom: 20px;
  }
  h2 {
    color: var(--txt-faded);
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin-bottom: 9px;
  }
  .dependencies {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 5px 18px;
    padding: 10px 12px;
    background: var(--bg-panel);
    border: 1px solid var(--sidesline);
  }
  code {
    color: var(--txt-faded);
    font-family: inherit;
    text-align: right;
  }
  a {
    color: var(--accent);
    cursor: pointer;
    text-decoration: none;
  }
  a:hover {
    text-decoration: underline;
  }
  .error {
    color: var(--search-invalid);
    line-height: 1.4;
    margin-top: 9px;
    white-space: pre-wrap;
  }
  .update-card {
    padding: 11px 12px;
    background: var(--bg-panel);
    border: 1px solid var(--sidesline);
  }
  .update-heading {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 14px;
  }
  .update-heading strong,
  .success strong {
    font-weight: 500;
  }
  .update-heading p,
  .success p,
  .updates > p,
  .update-card > p {
    margin-top: 5px;
  }
  .release-notes {
    max-height: 150px;
    overflow-y: auto;
    margin-top: 11px;
    padding: 9px 10px;
    color: var(--txt-faded);
    background: var(--bg-main);
    border: 1px solid var(--sidesline);
    line-height: 1.45;
    white-space: pre-wrap;
  }
  .muted,
  .up-to-date,
  .cached {
    color: var(--txt-faded);
    line-height: 1.4;
  }
  .cached,
  .progress {
    font-size: 12px;
  }
  .progress {
    color: var(--accent);
    line-height: 1.4;
  }
  .success {
    border-color: var(--accent);
  }
  footer {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 8px;
    padding-top: 2px;
  }
  button {
    border: 1px solid var(--border);
    padding: 5px 18px;
    user-select: none;
  }
  button:hover {
    background: var(--hover-row);
  }
  button:disabled {
    opacity: 0.55;
    pointer-events: none;
  }
  button.primary {
    color: var(--bg-main);
    background: var(--accent);
    border-color: var(--accent);
  }
  button.primary:hover {
    filter: brightness(1.08);
  }
</style>
