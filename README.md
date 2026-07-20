# Crescendo

A desktop music player for Linux that replicates the look of the foobar2000
**Eole** theme (dark mode). Built with Tauri 2, Svelte 5, and GStreamer for a
136k+ track FLAC library.

## Stack

- **Shell**: Tauri 2 (WebKitGTK) + SvelteKit (static adapter, SPA)
- **Playback**: GStreamer `playbin3` on a dedicated Rust thread — FLAC, MP3,
  Opus, Vorbis, WAV, AAC; gapless via `about-to-finish`
- **Library**: SQLite (WAL) at `~/.local/share/crescendo/library.db`;
  incremental scans are stat-only (mtime/size diff), tags read with lofty
- **Search**: Quod Libet-style query syntax compiled to SQL
  (`artist=parker`, `#(1994 <= year < 2000)`, `&(a, b)`, `album=/regex/`, …)
- **Art**: no blobs in the DB; `folder.jpg`/embedded art resolved lazily,
  256px thumbnails cached in `~/.cache/crescendo/thumbs/`, served over the
  `crescendo-art://` protocol
- **Now Playing**: Last.fm artist/album info (API key set in Settings, 30-day
  cache) with local-tag fallback

## Workspace layout

```
crates/core/   # crescendo-core: db, scanner, search, playlists, m3u, art, lastfm
               # (no UI deps — builds and tests standalone)
src-tauri/     # tauri shell: player engine, commands, art protocol
src/           # SvelteKit frontend (runs against a mock backend in plain vite dev)
reference/     # the original Eole foobar2000 theme (visual source of truth)
static/eole/   # PNG sprites copied from the theme
static/fonts/  # Selawik (SIL OFL) — metric-compatible Segoe UI substitute
```

## Building

One-time system setup (Fedora):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
sudo dnf install -y webkit2gtk4.1-devel openssl-devel file libappindicator-gtk3-devel \
  librsvg2-devel gcc-c++ gstreamer1-devel gstreamer1-plugins-base-devel
npm install
```

Run in development / build a release bundle:

```bash
npm run tauri dev
npm run tauri build
```

Frontend-only preview (mock library, no Rust needed): `npm run dev` → http://localhost:1420

Core tests and the scanner smoke test:

```bash
cargo test -p crescendo-core
cargo run --release -p crescendo-core --example smoke -- <music-dir> <db-path>
```

## Keyboard shortcuts

Space play/pause · ←/→ seek ±5s · `/` focus search · scroll wheel over the
volume icon adjusts volume · Esc closes the cover window
