<script lang="ts">
  import { onMount } from "svelte";
  import { ui } from "$lib/stores/ui.svelte";

  let message = $state("");
  let error = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;

  onMount(() => {
    const show = (event: WindowEventMap["crescendo:file-drag-notice"]) => {
      message = event.detail.message;
      error = event.detail.error;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => (message = ""), 4000);
    };
    window.addEventListener("crescendo:file-drag-notice", show);
    return () => {
      window.removeEventListener("crescendo:file-drag-notice", show);
      if (timer) clearTimeout(timer);
    };
  });
</script>

{#if message}
  <button
    class="notice"
    class:error
    style:bottom={`${ui.bottombarH + 14}px`}
    title="Dismiss"
    onclick={() => (message = "")}
  >
    {message}
  </button>
{/if}

<style>
  .notice {
    position: fixed;
    left: 50%;
    bottom: calc(var(--bottombar-h, 76px) + 14px);
    z-index: 300;
    max-width: min(520px, calc(100vw - 32px));
    transform: translateX(-50%);
    padding: 7px 13px;
    border: 1px solid var(--border);
    border-radius: 2px;
    background: var(--bg-panel);
    color: var(--txt);
    box-shadow: 0 4px 18px rgba(0, 0, 0, 0.45);
  }
  .notice.error {
    color: var(--warning, #ffba66);
  }
</style>
