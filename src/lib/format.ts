import type { Track } from "./api";
import { prefs } from "./stores/prefs.svelte";

/** 83000 -> "1:23"; hours kick in past 60 min. Negative -> "-1:23". */
export function formatTime(ms: number): string {
  const neg = ms < 0;
  let s = Math.round(Math.abs(ms) / 1000);
  const h = Math.floor(s / 3600);
  s -= h * 3600;
  const m = Math.floor(s / 60);
  s -= m * 60;
  const core =
    h > 0
      ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
      : `${m}:${String(s).padStart(2, "0")}`;
  return neg ? `-${core}` : core;
}

/** Display label for a %duration% browse bucket (whole minutes): 0 ->
    "Less than a minute (0:00 - 0:59)", 3 -> "3 minutes (3:00 - 3:59)". */
export function durationBucketLabel(bucket: number): string {
  if (!Number.isFinite(bucket) || bucket <= 0) return "Less than a minute (0:00 - 0:59)";
  return `${bucket} minute${bucket === 1 ? "" : "s"} (${bucket}:00 - ${bucket}:59)`;
}

/** "3 days, 11 h, 48 min" style used in header stats. */
export function formatLongDuration(ms: number): string {
  const min = Math.floor(ms / 60000);
  const d = Math.floor(min / 1440);
  const h = Math.floor((min % 1440) / 60);
  const m = min % 60;
  const parts: string[] = [];
  if (d) parts.push(`${d} day${d > 1 ? "s" : ""}`);
  if (h) parts.push(`${h} h`);
  if (m || parts.length === 0) parts.push(`${m} min`);
  return parts.join(", ");
}

/** CJK detection: kana, CJK ideographs (+ ext. A), halfwidth katakana, Hangul. */
const CJK_RE = /[぀-ヿ㐀-䶿一-鿿豈-﫿ｦ-ﾟ가-힯]/;
const LATIN_RE = /\p{Script=Latin}/u;

/**
 * Prefer the romanized sort tag when the display value contains CJK text and
 * the sort tag exists. Libraries without sort tags are unaffected, and the
 * `prefer_romanized` pref turns the substitution off altogether.
 */
function romanized(value: string | null, sort: string | null): string | null {
  if (!prefs.values.prefer_romanized) return value;
  if (value && sort && CJK_RE.test(value)) return sort;
  return value;
}

export function displayTitle(t: Track): string {
  return romanized(t.title, t.titlesort) ?? fileStem(t.path);
}

/** A Latin-script titlesort that can stand in for an original CJK title. */
export function latinTitleSort(t: Track): string | null {
  if (
    prefs.values.prefer_romanized &&
    t.title &&
    t.titlesort &&
    CJK_RE.test(t.title) &&
    !CJK_RE.test(t.titlesort) &&
    LATIN_RE.test(t.titlesort)
  ) {
    return t.titlesort;
  }
  return null;
}

/** The unmodified title tag, with the usual filename fallback. */
export function displayOriginalTitle(t: Track): string {
  return t.title ?? fileStem(t.path);
}

/** "N plays" for a track that has been played, or null when it never has —
    so callers render nothing (an invisible counter) for zero-play tracks. */
export function playsText(t: Track): string | null {
  if (t.play_count <= 0) return null;
  return t.play_count === 1 ? "1 play" : `${t.play_count} plays`;
}

export function displayArtist(t: Track): string | null {
  return (
    romanized(t.artist, t.artistsort) ?? romanized(t.albumartist, t.albumartistsort)
  );
}

export function displayAlbumartist(t: Track): string | null {
  return (
    romanized(t.albumartist, t.albumartistsort) ?? romanized(t.artist, t.artistsort)
  );
}

/** Same rule for album rows (albums carry an aggregated albumartist sort). */
export function displayName(value: string | null, sort: string | null): string | null {
  return romanized(value, sort);
}

// %title%, %artist% and %albumartist% go through the display helpers, so the
// "Prefer romanized names" pref reaches format patterns too. The plain *sort
// tags stay literal — they are the raw tag, not a display choice.
const TAG_GETTERS: Record<string, (t: Track) => string | number | null> = {
  title: (t) => displayTitle(t),
  artist: (t) => displayArtist(t),
  albumartist: (t) => displayAlbumartist(t),
  titlesort: (t) => t.titlesort,
  artistsort: (t) => t.artistsort,
  albumartistsort: (t) => t.albumartistsort,
  album: (t) => t.album,
  genre: (t) => t.genre,
  grouping: (t) => t.grouping,
  tracknumber: (t) => t.track_no,
  discnumber: (t) => t.disc_no,
  year: (t) => t.year,
  date: (t) => t.year,
  length: (t) => formatTime(t.duration_ms),
  codec: (t) => t.format,
};

function fileStem(path: string): string {
  const base = path.slice(path.lastIndexOf("/") + 1);
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(0, dot) : base;
}

/**
 * Minimal foobar-style title formatting: %tag% substitution plus
 * [bracketed sections] that render only if every %tag% inside resolved.
 * Default pattern: "%albumartist% - %title%".
 */
export function formatTitle(pattern: string, track: Track | null): string {
  if (!track) return "";
  return renderSection(pattern, track).text.trim();
}

function renderSection(src: string, track: Track): { text: string; any: boolean } {
  let out = "";
  let any = false;
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === "%") {
      const end = src.indexOf("%", i + 1);
      if (end === -1) {
        out += src.slice(i);
        break;
      }
      const tag = src.slice(i + 1, end).toLowerCase();
      const val = TAG_GETTERS[tag]?.(track);
      if (val !== null && val !== undefined && val !== "") {
        out += String(val);
        any = true;
      }
      i = end + 1;
    } else if (c === "[") {
      // find matching bracket (no nesting needed beyond depth counting)
      let depth = 1;
      let j = i + 1;
      while (j < src.length && depth > 0) {
        if (src[j] === "[") depth++;
        else if (src[j] === "]") depth--;
        j++;
      }
      // A matched section ends just before `]`; an unterminated user pattern
      // runs to the end and must not silently lose its final character.
      const innerEnd = depth === 0 ? j - 1 : j;
      const inner = renderSection(src.slice(i + 1, innerEnd), track);
      if (inner.any) out += inner.text;
      i = j;
    } else {
      out += c;
      i++;
    }
  }
  return { text: out, any };
}
