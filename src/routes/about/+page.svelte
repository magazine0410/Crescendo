<script lang="ts">
  import { onMount } from "svelte";
  import { call, isTauri, type AboutInfo } from "$lib/api";

  let info = $state<AboutInfo | null>(null);
  let failed = $state(false);

  onMount(() => {
    call<AboutInfo>("about_info")
      .then((value) => (info = value))
      .catch(() => (failed = true));
  });

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

  <p class="summary">A fast, Eole-inspired music player for large local libraries.</p>

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
  }
  footer {
    display: flex;
    justify-content: flex-end;
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
</style>
