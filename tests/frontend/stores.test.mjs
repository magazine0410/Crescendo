import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";
import ts from "typescript";
import { compileModule } from "svelte/compiler";

// Compile the real rune stores, replacing only their external services. No
// browser or new test dependency is needed to exercise async state transitions.
const root = fileURLToPath(new URL("../../", import.meta.url));
const cache = path.join(root, "node_modules/.cache");
await mkdir(cache, { recursive: true });
const output = await mkdtemp(path.join(cache, "crescendo-store-tests-"));
after(() => rm(output, { recursive: true, force: true }));
let serial = 0;

function preferences(overrides = {}) {
  return { values: {
    custom_browse_tag: "", track_sort: "", track_separators: false,
    playlist_sort: "manual", playlist_sort_locked: false, collections_placement: "above",
    ...overrides,
  } };
}

async function store(name, call, prefs = preferences()) {
  const source = await readFile(path.join(root, `src/lib/stores/${name}.svelte.ts`), "utf8");
  let js = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext },
  }).outputText.replace(/^import[^;]+;/gm, "");
  globalThis.__crescendoStoreServices = {
    call, prefs, isTauri: true, mockNowPlaying: null,
    staleProjectionRevision: () => null,
  };
  js = "const {call, prefs, isTauri, mockNowPlaying, staleProjectionRevision} = globalThis.__crescendoStoreServices;\n" + js;
  const compiled = compileModule(js, { filename: `${name}.svelte.js`, generate: "client" });
  const target = path.join(output, `${name}-${serial++}.mjs`);
  await writeFile(target, compiled.js.code);
  try {
    return (await import(pathToFileURL(target).href))[name];
  } finally {
    delete globalThis.__crescendoStoreServices;
  }
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test("a sparse context-menu selection loads only its two pages", async () => {
  const pages = [];
  const library = await store("library", async (command, args) => {
    assert.equal(command, "tracks_range");
    pages.push(args.offset);
    return Array.from({ length: args.limit }, (_, i) => ({ id: args.offset + i + 1 }));
  });
  library.trackCount = 136000;
  library.tracks = new Array(136000);
  library.selection = new Set([0, 135999]);
  assert.deepEqual((await library.collectSelectedTracks()).map(t => t.id), [1, 136000]);
  assert.deepEqual(pages, [0, 135800]);
});

test("large selections bound concurrent page requests", async () => {
  let active = 0, maximum = 0;
  const library = await store("library", async (_command, args) => {
    maximum = Math.max(maximum, ++active);
    await new Promise(resolve => setImmediate(resolve));
    active--;
    return Array.from({ length: args.limit }, (_, i) => ({ id: args.offset + i + 1 }));
  });
  library.trackCount = 4000;
  library.tracks = new Array(4000);
  library.selection = new Set(Array.from({ length: 4000 }, (_, i) => i));
  assert.equal((await library.collectSelectedTracks()).length, 4000);
  assert.ok(maximum <= 4);
});

test("a selection is discarded when its view changes during loading", async () => {
  const page = deferred();
  const library = await store("library", (command) =>
    command === "track_count" ? Promise.resolve(200) : page.promise);
  library.trackCount = 200;
  library.tracks = new Array(200);
  library.selection = new Set([0]);
  const collecting = library.collectSelectedTracks();
  await library.reloadTrackView(false);
  page.resolve([{ id: 99 }]);
  assert.deepEqual(await collecting, []);
  assert.equal(library.tracks[0], undefined);
});

test("play history refreshes counts, facets, albums and history-dependent order", async () => {
  for (const setup of [
    { track_sort: "lastplayed::desc" },
    { track_sort: "artist:lastplayed:asc" },
    { custom_browse_tag: "%playcount%" },
    { custom_browse_tag: "%lastplayed%" },
    { query: "p>0" },
  ]) {
    const commands = [];
    const library = await store("library", async command => {
      commands.push(command);
      if (command === "browse_values") return { values: [{ value: "1", count: 1 }], projection_revision: 0 };
      if (command === "browse_albums") return [];
      if (command === "track_count") return 1;
      throw new Error(`unexpected ${command}`);
    }, preferences(setup));
    if (setup.custom_browse_tag) library.mode = "custom";
    library.query = setup.query ?? null;
    library.trackCount = 2;
    library.tracks = [{ id: 1, play_count: 0 }, { id: 2, play_count: 0 }];
    library.selection = new Set([0]);
    const refreshed = library.bumpPlayCount(2, 1);
    assert.equal(library.interactionBlocked, true);
    await refreshed;
    assert.equal(library.interactionBlocked, false);
    assert.equal(library.trackCount, 1);
    assert.equal(library.trackEpoch, 1);
    assert.equal(library.selection.size, 0);
    assert.equal(library.tracks[0], undefined);
    assert.deepEqual(commands.sort(), ["browse_albums", "browse_values", "track_count"]);
  }
});

test("a play during a history refresh causes a second refresh", async () => {
  const first = deferred();
  let counts = 0;
  const library = await store("library", async command => {
    if (command === "browse_values") return { values: [], projection_revision: 0 };
    if (command === "browse_albums") return [];
    if (command === "track_count") return ++counts === 1 ? first.promise : 2;
    throw new Error(command);
  }, preferences({ track_sort: "lastplayed::desc" }));
  const one = library.bumpPlayCount(1, 1);
  while (counts === 0) await new Promise(resolve => setImmediate(resolve));
  const two = library.bumpPlayCount(2, 1);
  first.resolve(1);
  await Promise.all([one, two]);
  assert.equal(counts, 2);
  assert.equal(library.trackCount, 2);
});

test("ordinary library views patch play counts without querying the library", async () => {
  const library = await store("library", () => { throw new Error("unexpected refresh"); });
  library.tracks = [{ id: 1, play_count: 0 }];
  await library.bumpPlayCount(1, 1);
  assert.equal(library.tracks[0].play_count, 1);
  assert.equal(library.trackEpoch, 0);
});

test("a history refresh replaces an album load that it invalidated", async () => {
  const oldAlbum = deferred();
  let albumCalls = 0;
  const library = await store("library", async command => {
    if (command === "browse_values") return { values: [], projection_revision: 0 };
    if (command === "browse_albums") return [];
    if (command === "track_count") return 1;
    if (command === "album_tracks") {
      return ++albumCalls === 1 ? oldAlbum.promise : [{ id: 42, play_count: 1 }];
    }
    throw new Error(command);
  }, preferences({ track_sort: "lastplayed::desc" }));
  const selecting = library.selectAlbum({ id: 7 });
  await library.bumpPlayCount(42, 1);
  oldAlbum.resolve([{ id: 42, play_count: 0 }]);
  await selecting;
  assert.equal(albumCalls, 2);
  assert.equal(library.albumTracks[0].play_count, 1);
  assert.equal(library.selectedAlbum.id, 7);
});

async function playlistsWithRows(call) {
  const playlists = await store("playlists", call);
  playlists.list = [1, 2, 3].map(id => ({ id, name: String(id), track_count: 1, collection_id: null }));
  playlists.selectedIds = [1];
  playlists.groups = [{ playlist_id: 1, tracks: [{ id: 101 }] }];
  return playlists;
}

test("playlist actions keep the rendered identity until replacement rows arrive", async () => {
  const incoming = deferred();
  const playlists = await playlistsWithRows(() => incoming.promise);
  const selecting = playlists.select(2);
  assert.deepEqual([...playlists.selectedIds], [2]);
  assert.deepEqual([...playlists.renderedIds], [1]);
  assert.equal(playlists.renderedId, 1);
  assert.equal(playlists.tracks[0].id, 101);
  incoming.resolve([{ playlist_id: 2, tracks: [{ id: 202 }] }]);
  await selecting;
  assert.deepEqual([...playlists.renderedIds], [2]);
  assert.equal(playlists.renderedId, 2);
  assert.equal(playlists.tracks[0].id, 202);
});

test("failed playlist loads restore the sidebar selection and expose an error", async () => {
  const incoming = deferred();
  const playlists = await playlistsWithRows(() => incoming.promise);
  const selecting = playlists.select(2);
  incoming.reject(new Error("fixture failure"));
  await selecting;
  assert.deepEqual([...playlists.selectedIds], [1]);
  assert.deepEqual([...playlists.renderedIds], [1]);
  assert.match(playlists.selectionError, /fixture failure/);
});

test("an older playlist response cannot replace a newer selection", async () => {
  const older = deferred(), newer = deferred();
  const playlists = await playlistsWithRows((_command, args) => args.ids[0] === 2 ? older.promise : newer.promise);
  const a = playlists.select(2), b = playlists.select(3);
  newer.resolve([{ playlist_id: 3, tracks: [{ id: 303 }] }]);
  await b;
  older.resolve([{ playlist_id: 2, tracks: [{ id: 202 }] }]);
  await a;
  assert.deepEqual([...playlists.renderedIds], [3]);
  assert.equal(playlists.tracks[0].id, 303);
});

test("row playback sends the clicked track identity over IPC", async () => {
  const calls = [];
  const player = await store("player", async (...args) => calls.push(args));
  const ctx = { type: "library_view", projection_revision: 0, query: null, sort: null };
  await player.playContext(ctx, 0, 42);
  assert.deepEqual(calls, [["player_play_context", { ctx, startIndex: 0, startTrackId: 42 }]]);
});
