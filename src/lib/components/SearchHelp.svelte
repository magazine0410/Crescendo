<script lang="ts">
  interface Props {
    open: boolean;
    onclose: () => void;
  }
  let { open, onclose }: Props = $props();

  // Every tag usable in a search, alphabetical. "Text" tags take = / != / regex
  // matchers; "Number" tags compare with < <= = != >= >. Aliases are shorter
  // names for the same tag. Keep in sync with search/sql.rs.
  const tagRows: [string, string][] = [
    ["album", "Text — album title (short: a)."],
    ["albumartist", "Text — album artist (short: aa, aart)."],
    ["albumartistsort", "Text — album-artist sort name, romaji (short: aas)."],
    ["arranger", "Text — arranger (short: arr)."],
    ["artist", "Text — track artist (short: art)."],
    ["artistsort", "Text — artist sort name, romaji (short: as, arts)."],
    ["bitdepth", "Number — bits per sample (short: bd)."],
    ["bitrate", "Number — bitrate (short: br)."],
    ["bpm", "Number — beats per minute."],
    ["catalognumber", "Text — catalogue number, e.g. DRCD-0002 (short: cat, cn; alias: catalog)."],
    ["composer", "Text — composer (short: com)."],
    ["dirname", "Text — name of the containing folder (short: dir)."],
    ["disc", "Number — disc number (alias: discnumber)."],
    ["event", "Text — release event, e.g. C83 (short: e)."],
    ["filename", "Text — file name (short: file, fn)."],
    ["format", "Text — codec / container, e.g. FLAC (short: f; alias: codec)."],
    ["genre", "Text — genre (short: g)."],
    ["grouping", "Text — grouping tag (short: gr)."],
    ["length", "Number — duration in seconds; times are written as m:ss (short: l)."],
    ["lyricist", "Text — lyricist (short: lyr)."],
    ["path", "Text — full file path."],
    ["plays", "Number — times the track was played (short: p; alias: playcount)."],
    ["rating", "Number — rating (short: r, rat)."],
    ["samplerate", "Number — sample rate in Hz (short: sr)."],
    ["title", "Text — track title (short: t)."],
    ["titlesort", "Text — title sort name, romaji (short: ts)."],
    ["track", "Number — track number (short: tr; alias: tracknumber)."],
    ["vocalist", "Text — vocalist (short: voc)."],
    ["year", "Number — release year (short: y; alias: date)."],
  ];

  const sections: { title: string; rows: [string, string][] }[] = [
    {
      title: "All searchable tags",
      rows: tagRows,
    },
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
        ["artist=parker", "Tag contains the value (case-insensitive). See the tag list above."],
        ["art=parker", "Most tags have a short name; it means exactly the same thing."],
        ["artist=\"Charlie Parker\"", "Quote a value to include spaces; it still matches as a substring (also matches “Charlie Parker Quartet”)."],
        ["artist=\"BoA\"c", "Trailing c makes the match case-sensitive."],
        ["genre!=rock", "Excludes matches (tracks without the tag count as not matching)."],
        ["album=/shadow/", "Regular expression between slashes; add c for case-sensitive."],
      ],
    },
    {
      title: "Numbers and time",
      rows: [
        ["year>=2010", "Numeric comparison: < <= = != >= >. Numeric tags: track, disc, year, length, bpm, rating, plays, bitrate, samplerate, bitdepth."],
        ["#(year >= 2010)", "The same comparison in Quod Libet's #( ) form."],
        ["1994<=year<2000", "Comparisons chain into ranges."],
        ["#(length > 5:00)", "Times are written as m:ss or h:mm:ss."],
      ],
    },
    {
      title: "Play counts",
      rows: [
        ["plays=3", "Tracks played exactly 3 times."],
        ["plays>2", "Played more than twice; use >= for “2 or more”."],
        ["plays<3", "Played once or twice. Never-played tracks stay out: no plays is not a low play count."],
        ["plays=0", "Never played. A query that asks about 0 keeps those tracks (also plays<1)."],
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
        queries turn the text red. Each part of a query is coloured on its own,
        so a bad tag reddens only itself.
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
