<script lang="ts">
  import { iconMap, lucideIcons } from "$lib/icons";

  interface Props {
    /** Eole glyph name (e.g. "prev") or a Lucide name directly (e.g. "skip-back"). */
    name: string;
    /** Rendered square size in px. */
    size?: number;
  }
  let { name, size = 24 }: Props = $props();

  // Accept either an Eole glyph name (mapped) or a raw Lucide name.
  const inner = $derived(lucideIcons[iconMap[name] ?? name] ?? "");
</script>

{#if inner}
  <svg
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    width={size}
    height={size}
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true">{@html inner}</svg>
{/if}

<style>
  svg {
    display: block;
    flex: none;
  }
</style>
