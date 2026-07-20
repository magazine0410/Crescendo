<script lang="ts" module>
  export interface MenuItem {
    label: string;
    action?: () => void;
    separator?: boolean;
    disabled?: boolean;
  }

  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  export function openContextMenu(e: MouseEvent, items: MenuItem[]) {
    e.preventDefault();
    e.stopPropagation();
    menu = { x: e.clientX, y: e.clientY, items };
  }

  export function closeContextMenu() {
    menu = null;
  }
</script>

<script lang="ts">
  function pick(item: MenuItem) {
    if (item.disabled || item.separator) return;
    closeContextMenu();
    item.action?.();
  }

  function onWindowDown(e: PointerEvent) {
    if (menu && !(e.target as HTMLElement).closest(".ctx-menu")) closeContextMenu();
  }
</script>

<svelte:window onpointerdown={onWindowDown} onblur={closeContextMenu} />

{#if menu}
  <div
    class="ctx-menu"
    style:left={`${Math.min(menu.x, window.innerWidth - 200)}px`}
    style:top={`${Math.min(menu.y, window.innerHeight - menu.items.length * 26 - 12)}px`}
  >
    {#each menu.items as item, i (i)}
      {#if item.separator}
        <div class="sep"></div>
      {:else}
        <button class="item" class:disabled={item.disabled} onclick={() => pick(item)}>
          {item.label}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .ctx-menu {
    position: fixed;
    z-index: 500;
    background: var(--bg-popup);
    border: 1px solid var(--popup-border);
    padding: 4px 0;
    min-width: 180px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.6);
  }
  .item {
    display: block;
    width: 100%;
    text-align: left;
    padding: 5px 14px;
    color: var(--txt);
  }
  .item:hover:not(.disabled) {
    background: var(--hover-row);
  }
  .item.disabled {
    color: var(--txt-faded);
    cursor: default;
  }
  .sep {
    height: 1px;
    margin: 4px 8px;
    background: var(--hairline);
  }
</style>
