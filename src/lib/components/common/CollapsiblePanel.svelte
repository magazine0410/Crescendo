<script lang="ts">
  import type { Snippet } from "svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import Splitter from "./Splitter.svelte";

  interface Props {
    side: "left" | "right";
    size: number;
    min: number;
    max: number;
    persistKey: string;
    collapsePersistKey: string;
    collapsed: boolean;
    panelId: string;
    label: string;
    transitionsReady?: boolean;
    onresize: (px: number) => void;
    oncollapsedchange: (collapsed: boolean) => void;
    children: Snippet;
  }

  let {
    side,
    size,
    min,
    max,
    persistKey,
    collapsePersistKey,
    collapsed,
    panelId,
    label,
    transitionsReady = true,
    onresize,
    oncollapsedchange,
    children,
  }: Props = $props();

  let resizing = $state(false);

  function toggle() {
    const next = !collapsed;
    oncollapsedchange(next);
    void prefs.setLayout(collapsePersistKey, next ? 1 : 0);
  }
</script>

{#snippet panel()}
  <div
    id={panelId}
    class="panel {side}"
    class:animate={transitionsReady && !resizing}
    style:width={`${collapsed ? 0 : size}px`}
    style:--expanded-width={`${size}px`}
    role="region"
    aria-label={label}
    aria-hidden={collapsed}
    inert={collapsed}
  >
    <div class="inner">
      {@render children()}
    </div>
  </div>
{/snippet}

{#snippet divider()}
  <Splitter
    {size}
    {min}
    {max}
    invert={side === "right"}
    {persistKey}
    {collapsed}
    {panelId}
    panelLabel={label}
    animate={transitionsReady && !resizing}
    {onresize}
    ontoggle={toggle}
    ondragchange={(active) => (resizing = active)}
  />
{/snippet}

{#if side === "left"}
  {@render panel()}
  {@render divider()}
{:else}
  {@render divider()}
  {@render panel()}
{/if}

<style>
  .panel {
    position: relative;
    flex: none;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }
  .panel.animate {
    transition: width 180ms cubic-bezier(0.2, 0, 0, 1);
  }
  .inner {
    position: absolute;
    inset-block: 0;
    width: var(--expanded-width);
    min-height: 0;
  }
  .left .inner {
    left: 0;
  }
  .right .inner {
    right: 0;
  }

  @media (prefers-reduced-motion: reduce) {
    .panel.animate {
      transition-duration: 0s;
    }
  }
</style>
