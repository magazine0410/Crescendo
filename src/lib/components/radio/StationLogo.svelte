<script lang="ts">
  import { logoUrl } from "$lib/api";
  import Icon from "../common/Icon.svelte";

  /** A station's logo, or the radio icon where it has none or it fails. */
  interface Props {
    logo: string | null;
    size?: number;
  }
  let { logo, size = 22 }: Props = $props();

  const src = $derived(logoUrl(logo));
  /** The logo that failed, so a row reused for another station retries. */
  let failedFor = $state<string | null>(null);
</script>

<span class="logo" style:width={`${size}px`} style:height={`${size}px`}>
  {#if src && failedFor !== src}
    <img {src} alt="" loading="lazy" onerror={() => (failedFor = src)} />
  {:else}
    <Icon name="radio" size={Math.round(size * 0.7)} />
  {/if}
</span>

<style>
  .logo {
    display: grid;
    place-items: center;
    flex: none;
    overflow: hidden;
    border-radius: 3px;
    background: var(--bg-main);
    color: var(--txt-faded);
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
  }
</style>
