<script lang="ts">
  interface Props {
    open: boolean;
    onclose: () => void;
  }
  let { open, onclose }: Props = $props();

  const sections: { title: string; rows: [string, string][] }[] = [
    {
      title: "Free text",
      rows: [
        ["nausicaa wind", "Words match anywhere in title, artist, album or genre; multiple words must all match (AND)."],
        ["\"exact phrase\"", "Quoted text matches as one phrase."],
      ],
    },
    {
      title: "Tag searches",
      rows: [
        ["artist=parker", "Tag contains the value (case-insensitive). Tags: title, artist, album, albumartist, titlesort, artistsort, albumartistsort, genre, grouping, format, dirname, filename, path."],
        ["artist=\"Charlie Parker\"", "Quoted value matches the whole tag exactly."],
        ["artist=\"BoA\"c", "Trailing c makes the match case-sensitive."],
        ["genre!=rock", "Excludes matches (tracks without the tag count as not matching)."],
        ["album=/shadow/", "Regular expression between slashes; add c for case-sensitive."],
      ],
    },
    {
      title: "Numbers and time",
      rows: [
        ["#(year >= 2010)", "Numeric comparison: < <= = != >= >. Numeric tags: track, disc, year, length, rating, bitrate, samplerate, bitdepth."],
        ["#(1994 <= year < 2000)", "Comparisons chain into ranges."],
        ["#(length > 5:00)", "Times are written as m:ss or h:mm:ss."],
      ],
    },
    {
      title: "Combining",
      rows: [
        ["&(electro, !ambient)", "AND of several conditions."],
        ["|(mangum, neutral milk)", "OR of several conditions."],
        ["!genre=pop", "NOT — negates any condition."],
        ["genre=|(jazz, soul)", "Value list: the tag may match any of the values."],
        ["&(artist=zun, #(length > 4:00))", "Everything nests freely."],
      ],
    },
  ];

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
  }
</script>

<svelte:window onkeydown={open ? onKeydown : undefined} />

{#if open}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
    <div class="dialog" role="dialog" aria-label="Search syntax help">
      <header>
        <span>Searching the library</span>
        <button class="x" onclick={onclose}>✕</button>
      </header>
      <p class="intro">
        The search bar uses Quod Libet-style queries. Plain words are enough for
        most searches; the operators below unlock precise filters. Invalid
        queries turn the text red.
      </p>
      {#each sections as s (s.title)}
        <section>
          <h3>{s.title}</h3>
          <table>
            <tbody>
              {#each s.rows as [q, desc] (q)}
                <tr>
                  <td><code>{q}</code></td>
                  <td>{desc}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </section>
      {/each}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 300;
  }
  .dialog {
    width: 640px;
    max-height: 82vh;
    overflow-y: auto;
    background: var(--bg-main);
    border: 1px solid var(--popup-border);
    box-shadow: 0 8px 40px rgba(0, 0, 0, 0.7);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 18px;
    border-bottom: 1px solid var(--hairline);
    font-weight: 600;
    position: sticky;
    top: 0;
    background: var(--bg-main);
  }
  .x {
    color: var(--txt-faded);
  }
  .x:hover {
    color: var(--txt);
  }
  .intro {
    padding: 12px 18px 4px;
    color: var(--txt-faded);
    line-height: 1.55;
  }
  section {
    padding: 10px 18px;
  }
  h3 {
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--txt-faded);
    margin-bottom: 8px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  td {
    padding: 5px 10px 5px 0;
    vertical-align: top;
    line-height: 1.5;
    border-bottom: 1px solid var(--sidesline);
  }
  td:first-child {
    white-space: nowrap;
    width: 220px;
  }
  code {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 11px;
    color: var(--accent);
    user-select: text;
  }
  td:last-child {
    color: var(--txt);
  }
</style>
