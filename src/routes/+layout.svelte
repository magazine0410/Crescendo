<script lang="ts">
  import "$lib/styles/tokens.css";
  import "$lib/styles/fonts.css";
  import "$lib/styles/base.css";
  import "$lib/styles/scrollbars.css";
  import { APPEARANCE_CACHE_KEY, restoreAppearance } from "$lib/appearance";

  let { children } = $props();

  // Before the first render: the main window would otherwise start in Eole
  // colors until prefs load, and the other windows cannot read prefs at all.
  restoreAppearance();
</script>

<!-- An open About window follows appearance changes made in Settings. -->
<svelte:window onstorage={(e) => e.key === APPEARANCE_CACHE_KEY && restoreAppearance()} />

{@render children()}
