# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Crescendo is a Tauri 2 + Svelte 5 music player for Linux that replicates the foobar2000 **Eole** theme (dark mode). It is built for a ~136k-track, FLAC-heavy library on an HDD — every design decision below follows from "never make the UI wait on 136k rows".

The visual source of truth is the original theme in `reference/Ottodix-Eole-foobar-theme-ea6744c/` — colors and layout constants were extracted from `themes/eole/js/JStheme_common.js` into `src/lib/styles/tokens.css`. When adjusting the look, check the reference theme (and its `preview.png`) before inventing values. Priority order for appearance decisions: theme files > user's foobar screenshots > written specs.

**Icons are the exception**: they are inline **Lucide** SVGs (see *Frontend > icons* below), not theme sprites — the Eole PNG glyphs were removed for licensing reasons, so don't reach for `/eole/controls/*.png`. Colors and layout still follow the theme.

## Commands

```bash
npm run tauri dev        # run the real desktop app (rebuilds Rust on change)
npm run dev              # frontend only at :1420 — runs against src/lib/mock.ts, no Rust needed
npm run check            # svelte-check (type-check the frontend)
npm run tauri build      # release bundles → target/release/bundle/{rpm,deb}

cargo test -p crescendo-core                          # all core tests
cargo test -p crescendo-core --test search_test       # one test file
cargo test -p crescendo-core --test search_test free_text_japanese  # one test

# Headless harnesses (run against real files / the real DB):
cargo run --release -p crescendo-core --example smoke -- <music-dir> <db-path>   # scan + search + art
cargo run -p crescendo --example play_test -- <file>...                          # GStreamer playback + gapless
cargo run --release -p crescendo-core --example m3u_art_test -- <db> <m3u> <cache-dir>
```

Runtime data: DB at `~/.local/share/crescendo/library.db` (SQLite WAL — safe to open with a second connection while the app runs), thumbnails at `~/.cache/crescendo/thumbs/`.

## Architecture

**Two Rust crates, deliberately split.** `crates/core` (crescendo-core) holds everything with no UI dependency: DB/schema/queries, scanner, search compiler, play-queue logic, m3u, cover-art resolution, Last.fm client. It builds and tests without webkit2gtk/GStreamer headers. `src-tauri` is a thin shell: the GStreamer player thread, `#[tauri::command]` wrappers (in `src/commands/`, no logic of their own), the `crescendo-art://` protocol handler, and window management. Put new logic in core unless it needs tauri or gstreamer types.

**The frontend never receives the whole library.** Views get counts and fetch windowed slices (`tracks_range`), or bounded sets (albums for a selection, one playlist). Playback starts by sending a **context descriptor** (`PlayContext`: album / playlist / library_view / value_tracks) plus a start index; Rust re-resolves it to an ordered id list (`Db::resolve_context`) using the same SQL the view used. Track-id lists never cross IPC.

**Player** (`src-tauri/src/player/mod.rs`): one thread owns a `playbin3`; commands arrive via mpsc `Cmd`, state flows out as `player:state` / `player:position` (250 ms tick) Tauri events, mirrored into a shared `Mutex<PlayerState>` for `player_get_state`. Gapless works by setting the next uri inside GStreamer's `about-to-finish` callback — which calls `PlayQueue::advance()` *early*, so the queue is already advanced when `StreamStart` fires. Ordering semantics live in `crescendo_core::playqueue` (pure, unit-tested): ad-hoc queue always preempts the context (foobar behavior), shuffle is a permuted walk order that keeps the current track, repeat cycles off→all→one.

**Scanner** (`crates/core/src/scanner/`): walker thread (extension whitelist) → mtime/size diff against an in-memory map of known files (unchanged files never get their tags read — this is why rescans take seconds) → 4 lofty reader threads (HDD-tuned, `scan_threads` pref) → single writer batching 1000/txn. Unreadable tags fall back to filename-only rows (relaxed lofty parsing first). After every scan `albums.rs::rebuild` regenerates the albums table in one GROUP BY pass, upserting on a normalized `(albumartist-or-artist, album)` key so album ids — and therefore the thumbnail cache — stay stable.

**Search** (`crates/core/src/search/`): hand-rolled recursive-descent parser for Quod Libet syntax → AST → SQL WHERE fragment with *anonymous* `?` params only (queries.rs appends numbered params after it — keep that invariant). SQLite's LIKE/lower() are ASCII-only, so matching goes through Rust UDFs (`ucontains`, `ueq`, `regexp`) registered per connection; free text hits `search_blob`, which is Rust-lowercased at insert time. The library is heavily Japanese-tagged — test any matching change against non-ASCII (see `tests/search_test.rs`).

**Cover art**: no image blobs in the DB. `albums.cover_source` caches the resolution (`folder:<path>` or `embedded:<track_id>`); thumbnails generate lazily on first request through the `crescendo-art://` protocol (semaphore-bounded), so the virtualized album wall only ever generates what's visible.

**Romanized display**: the library carries `titlesort`/`artistsort`/`albumartistsort` tags with romaji for Japanese names. Wherever a title or artist renders, use the `display*` helpers in `src/lib/format.ts` (never raw `t.title`/`t.artist`): they substitute the sort tag when the original contains CJK and the sort tag exists, and are a no-op otherwise. This is deliberate automatic behavior with no settings toggle. Artist browsing also sorts by `artistsort` (`browse_values` returns `[value, count, sort]` triples); Last.fm queries keep the raw names.

**Frontend** (`src/lib/`): Svelte 5 runes stores in `stores/*.svelte.ts`; `api.ts` routes `call()` to real IPC or to `mock.ts` when not in Tauri — keep mock.ts in sync when adding commands so `npm run dev` stays useful. `events.ts` bridges Tauri events into stores. Virtualization is hand-rolled (`virtual/VirtualList.svelte` with prefix-sum offsets for mixed-height rows, `VirtualGrid.svelte` on top of it); mixed-height lists like the grouped playlist view pass an `itemHeight(index)` function. Panel sizes persist via `prefs.setLayout` into the backend `prefs` table (the only settings store — no tauri-plugin-store).

**Icons** (`src/lib/components/common/Icon.svelte` + `src/lib/icons.ts`): UI glyphs are inline **Lucide** SVGs (ISC-licensed), not the old Eole PNG sprites. `icons.ts` is **hand-maintained** (no generator) and holds two maps: `iconMap` (Eole glyph name → icon name, e.g. `"prev" → "skip-back"`) and `lucideIcons` (icon name → inner SVG markup, a 24×24 viewBox with no `<svg>` wrapper). `Icon.svelte` takes an Eole *or* Lucide name and renders it at `stroke-width 1.5` with `stroke="currentColor"`, so **color and hover come from CSS `color`, not opacity** (`SpriteButton` brightens on hover; the shuffle toggle stays full-opacity and signals state by *shape*, while repeat still dims when off). *To add/change an icon*: paste the target Lucide icon's inner paths into `lucideIcons` and point the Eole name at it in `iconMap` — that's it. A few icons are **custom composites** built from Lucide primitives: `rotate-cw-1` (repeat-one = rotate-cw ring + Lucide's repeat-1 "1"), `two-arrows-right` (shuffle-off = arrow-right-left with the bottom arrow mirrored right), and `volume-1..4` — a 4-wave fan (speaker path + N concentric arcs, center x≈10.5, half-angle 37°, radii 4/7/10/13 for 1→4 waves) that **overrides official Lucide `volume-1`/`-2`** so all four levels share geometry. Only two Eole PNGs remain in `static/eole/` — the `no_cover` and `nothing_played` placeholders (kept deliberately); everything else there was deleted.

## Gotchas

- **Tauri capabilities**: webview-side APIs (dialogs, `window.close()`, …) silently no-op unless allowed in `src-tauri/capabilities/default.json`, and each window label must be listed there. If a frontend API "does nothing", check this file first — it has bitten twice (m3u file picker, cover-window Esc).
- **NVIDIA blank window**: `WEBKIT_DISABLE_DMABUF_RENDERER=1` is set in `src-tauri/src/main.rs`; removing it makes the window render black on this machine.
- Tauri command args: snake_case parameter names in Rust, camelCase keys from JS (`album_id` ↔ `albumId`) — Tauri converts automatically.
- AppImage bundling fails at linuxdeploy's GStreamer step; bundle targets are intentionally restricted to `deb` + `rpm` in `tauri.conf.json`.
- `reference/` is excluded from Vite's watcher (see `vite.config.js`) and is not part of the build — it is documentation. It is also **gitignored**: the bundled Eole theme carries no license, so never commit or redistribute it. The built `.deb`/`.rpm` still embed the two Eole placeholder PNGs (`no_cover`, `nothing_played`) — swap those before any *public* release.
