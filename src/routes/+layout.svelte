<script lang="ts">
  import "$lib/styles/tokens.css";
  import "$lib/styles/fonts.css";
  import "$lib/styles/base.css";
  import "$lib/styles/scrollbars.css";

  let { children } = $props();
</script>

{@render children()}
