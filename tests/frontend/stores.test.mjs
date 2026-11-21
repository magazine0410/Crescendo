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

async function store(name, call, prefs = preferences(), services = {}) {
  const source = await readFile(path.join(root, `src/lib/stores/${name}.svelte.ts`), "utf8");
  let js = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext },
  }).outputText.replace(/^import[^;]+;/gm, "");
  globalThis.__crescendoStoreServices = {
    call, prefs, isTauri: true, mockNowPlaying: null,
    staleProjectionRevision: () => null,
    ...services,
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
    if (setup.custom_browse_tag) {
      library.facet("artist").expanded = false;
      library.facet("custom").expanded = true;
    }
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

test("scrollbar jumps skip obsolete pages and load the latest viewport next", async () => {
  const requests = [];
  const library = await store("library", (_command, args) => {
    const reply = deferred();
    requests.push({ offset: args.offset, reply });
    return reply.promise;
  });
  library.trackCount = 160000;
  library.tracks = new Array(160000);
  library.ensureTrackRange(190, 220); // spans two pages
  library.ensureTrackRange(50000, 50040);
  library.ensureTrackRange(100000, 100040);
  library.ensureTrackRange(150000, 150040);
  assert.deepEqual(requests.map(r => r.offset), [0]);
  requests[0].reply.resolve([{ id: 1 }]);
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(requests.map(r => r.offset), [0, 150000]);
  requests[1].reply.resolve([{ id: 150001 }]);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(library.tracks[150000].id, 150001);
  library.ensureTrackRange(150000, 150040);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(requests.length, 2, "revisiting a loaded page uses the cache");
});

test("album separators fetch only visible pages within huge groups", async () => {
  const offsets = [];
  const library = await store("library", async (command, args) => {
    if (command === "track_count") return 160000;
    if (command === "track_groups") return [{ album_id: 1, count: 160000 }];
    assert.equal(command, "tracks_range");
    offsets.push(args.offset);
    return [];
  });
  await library.reloadTrackView(true);
  library.ensureRowRange(150001, 150040);
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(offsets, [150000]);
});

test("old page failures cannot evict a new view's cached page", async () => {
  const old = deferred();
  let requests = 0;
  const library = await store("library", async command => {
    if (command === "track_count") return 200;
    return ++requests === 1 ? old.promise : [{ id: 42 }];
  });
  library.trackCount = 200;
  const pending = library.ensureTrackRangeLoaded(0, 10);
  await library.reloadTrackView(false);
  await library.ensureTrackRangeLoaded(0, 10);
  old.reject(new Error("obsolete request"));
  await pending;
  await library.ensureTrackRangeLoaded(0, 10);
  assert.equal(requests, 2);
  assert.equal(library.tracks[0].id, 42);
});

const settle = async () => { for (let i = 0; i < 20; i++) await new Promise(resolve => setImmediate(resolve)); };

/** Answers every library query; `overrides` replaces single commands. */
function libraryCalls(overrides = {}, log = []) {
  return async (command, args) => {
    log.push({ command, args });
    if (overrides[command]) return overrides[command](args);
    if (command === "browse_values") return { values: [], projection_revision: args?.revision ?? 0 };
    if (command === "browse_albums") return [];
    if (command === "track_count") return 400;
    if (command === "tracks_range") return Array.from({ length: args.limit }, (_, i) => ({ id: args.offset + i + 1 }));
    throw new Error(`unexpected ${command}`);
  };
}

const staleError = { stale: 5 };
const staleServices = { staleProjectionRevision: e => (e === staleError ? 5 : null) };

test("a stale recovery waits for a projection change and the gate stays shut for both", async () => {
  const write = deferred(), recoveryValues = deferred();
  let values = 0;
  const library = await store("library", libraryCalls({
    browse_values: () => (++values === 2 ? recoveryValues.promise : { values: [], projection_revision: 3 }),
  }), preferences(), staleServices);

  const changing = library.changeProjection(() => write.promise);
  const recovering = library.recoverStaleProjection(staleError);
  await settle();
  assert.equal(values, 0, "the recovery must not start beside the projection change");
  write.resolve(3);
  await changing;
  await settle();
  assert.equal(values, 2, "the recovery has started");
  assert.equal(library.interactionBlocked, true, "the first to finish must not open the gate");
  assert.equal(library.projectionRevision, 5);
  recoveryValues.resolve({ values: [], projection_revision: 5 });
  assert.equal(await recovering, true);
  assert.equal(library.interactionBlocked, false);
});

test("a rejected projection write still rebuilds what it invalidated", async () => {
  const log = [];
  const library = await store("library", libraryCalls({}, log));
  await library.reloadTrackView(false);
  await library.ensureTrackRangeLoaded(0, 10);
  assert.equal(library.tracks[0].id, 1);

  log.length = 0;
  await assert.rejects(library.changeProjection(async () => { throw new Error("refused"); }), /refused/);
  assert.equal(library.interactionBlocked, false);
  assert.ok(log.some(c => c.command === "track_count"), "the view reloaded");
  await library.ensureTrackRangeLoaded(0, 10);
  assert.equal(library.tracks[0].id, 1, "its pages load again");
});

test("a rebuild that hit a newer revision recovers instead of leaving the view empty", async () => {
  let first = true, values = 0;
  const library = await store("library", libraryCalls({
    browse_values: () => ({ values: [], projection_revision: ++values === 1 ? 3 : 5 }),
    track_count: () => { if (first) { first = false; throw staleError; } return 7; },
  }), preferences(), staleServices);
  await library.changeProjection(async () => 3);
  await settle();
  assert.equal(library.interactionBlocked, false);
  assert.equal(library.projectionRevision, 5);
  assert.equal(library.trackCount, 7);
});

test("a failed reload keeps the old list working, with an in-flight page included", async () => {
  const inflight = deferred(), log = [];
  const library = await store("library", libraryCalls({
    tracks_range: args => (args.offset === 0 && !inflight.done ? (inflight.done = true, inflight.promise)
      : [{ id: args.offset + 1, query: args.query }]),
    track_count: args => { if (args.query === "broken(") throw new Error("invalid query"); return 400; },
  }, log));
  library.query = "good";
  await library.reloadTrackView(false);
  const page = library.ensureTrackRangeLoaded(0, 10);
  await settle();

  library.query = "broken(";
  await library.reloadTrackView(false);
  assert.equal(library.trackEpoch, 1, "nothing was replaced");
  inflight.resolve([{ id: 1, query: "good" }]);
  await page;
  assert.equal(library.tracks[0].id, 1, "that page still belongs to the list on screen");

  await library.ensureTrackRangeLoaded(200, 210);
  assert.equal(library.tracks[200].query, "good", "later pages match the rows on screen");
});

test("a page requested during a reset fetches the rows of the list on screen", async () => {
  const count = deferred(), pages = [];
  const library = await store("library", libraryCalls({
    track_count: args => (args.query === "new" ? count.promise : 400),
    tracks_range: args => { pages.push(args.query); return [{ id: args.offset + 1, query: args.query }]; },
  }));
  library.query = "old";
  await library.reloadTrackView(false);
  library.query = "new";
  const reloading = library.reloadTrackView(false);
  await library.ensureTrackRangeLoaded(0, 10);
  assert.deepEqual(pages, ["old"]);
  assert.equal(library.tracks[0].query, "old");

  count.resolve(50);
  await reloading;
  await library.ensureTrackRangeLoaded(0, 10);
  assert.deepEqual(pages, ["old", "new"]);
  assert.equal(library.tracks[0].query, "new");
  assert.equal(library.tracks.length, 50);
});

test("a view change made during a rebuild applies when it ends, and only then", async () => {
  const write = deferred(), counts = [];
  const prefs = preferences();
  const library = await store("library", libraryCalls({
    track_count: args => { counts.push(args.query); return 1; },
  }), prefs);
  const changing = library.changeProjection(() => write.promise);
  library.requestTrackView(false);
  library.query = "typed meanwhile";
  library.requestTrackView(false);
  assert.deepEqual(counts, []);
  write.resolve(1);
  await changing;
  await settle();
  // The rebuild read the query when it ran, so nothing is left to apply.
  assert.deepEqual(counts, ["typed meanwhile"]);

  const second = deferred();
  const again = library.changeProjection(() => second.promise);
  library.requestTrackView(false);
  second.resolve(2);
  await settle();
  await again;
  library.query = "after";
  assert.equal(counts.length, 2, "an unchanged view does not reload twice");
});

test("a view change made after a rebuild has read its inputs is not lost", async () => {
  const count = deferred(), counts = [];
  const library = await store("library", libraryCalls({
    track_count: args => { counts.push(args.query); return counts.length === 1 ? count.promise : 1; },
  }));
  const changing = library.changeProjection(async () => 1);
  await settle();
  assert.deepEqual(counts, [null], "the rebuild is waiting on its count");
  library.query = "typed meanwhile";
  library.requestTrackView(false);
  count.resolve(1);
  await changing;
  await settle();
  assert.deepEqual(counts, [null, "typed meanwhile"]);
});

test("a failed or superseded value load does not prune the selection", async () => {
  const library = await store("library", libraryCalls({
    browse_values: () => { throw new Error("invalid query"); },
  }));
  const artist = library.facet("artist");
  artist.values = [{ value: "a", count: 1 }];
  artist.selectedValues = ["a", "gone"];
  await library.setQuery("broken(");
  assert.deepEqual([...artist.selectedValues], ["a", "gone"]);
});

const openFacets = library => library.facets.filter(facet => facet.expanded).map(facet => facet.mode);

test("opening a category closes the others and keeps every selection", async () => {
  for (const layout of ["tabs", "list"]) {
    const library = await store("library", libraryCalls(), preferences({ filter_panel_layout: layout }));
    const artist = library.facet("artist");
    const album = library.facet("album");
    artist.selectedValues = ["a"];
    artist.valueFilter = "narrowing";
    await library.openFacet(album);
    assert.deepEqual(openFacets(library), ["album"], layout);
    assert.equal(artist.valueFilter, "");
    assert.deepEqual([...artist.selectedValues], ["a"]);
    await library.openFacet(album);
    assert.equal(album.expanded, true, "clicking the open tab leaves it open");
  }
});

test("a vertical header toggles its own list, one open at most", async () => {
  const library = await store("library", libraryCalls(), preferences({ filter_panel_layout: "list" }));
  await library.toggleFacet(library.facet("album"));
  assert.deepEqual(openFacets(library), ["album"]);
  await library.toggleFacet(library.facet("genre"));
  assert.deepEqual(openFacets(library), ["genre"]);
  await library.toggleFacet(library.facet("genre"));
  assert.deepEqual(openFacets(library), [], "clicking the open header collapses it");
});

test("the tabs layout opens Artist when every category was collapsed", async () => {
  const library = await store("library", libraryCalls(), preferences({ filter_panel_layout: "tabs" }));
  library.facet("album").selectedValues = ["x"];
  for (const facet of library.facets) facet.expanded = false;
  await library.showOneFacet();
  assert.deepEqual(openFacets(library), ["artist"]);
  assert.deepEqual([...library.facet("album").selectedValues], ["x"]);

  await library.openFacet(library.facet("genre"));
  await library.showOneFacet();
  assert.deepEqual(openFacets(library), ["genre"], "an open category stays");
});

test("clearing filters empties every category and reloads the albums once", async () => {
  const albums = [];
  const library = await store("library", libraryCalls({
    browse_albums: args => { albums.push(args.filters); return []; },
  }));
  library.facet("artist").selectedValues = ["a"];
  library.facet("album").selectedValues = ["x", "y"];
  library.facet("artist").valueFilter = "kept";
  await library.clearFilters();
  assert.deepEqual(library.activeFilters, []);
  assert.deepEqual(albums, [[]]);
  assert.equal(library.facet("artist").valueFilter, "kept");
  await library.clearFilters();
  assert.equal(albums.length, 1, "nothing to clear, nothing reloaded");
});

const progress = (fields = {}) => ({ scanned: 1, added: 0, updated: 0, removed: 0, errors: 0, current_path: "", done: false, cancelled: false, ...fields });

test("a scan moves through its phases from one state value", async () => {
  const start = deferred();
  const library = await store("library", async c => (c === "scan_start" ? start.promise : undefined));
  assert.equal(library.scan.phase, "idle");
  const starting = library.startScan(false);
  assert.equal(library.scan.phase, "starting");
  assert.equal(library.scanning, true);
  start.resolve();
  await starting;
  library.onScanProgress(progress());
  assert.equal(library.scan.phase, "running");
  await library.cancelScan();
  assert.equal(library.scan.phase, "cancelling");
  library.onScanProgress(progress({ scanned: 2 }));
  assert.equal(library.scan.phase, "cancelling", "a late progress event must not undo it");
  assert.equal(library.scan.progress.scanned, 2);
  library.onScanProgress(progress({ done: true, cancelled: true }));
  assert.equal(library.scan.phase, "finished");
  assert.equal(library.scanning, false);
});

test("dismissing a scan failure leaves nothing behind", async () => {
  const library = await store("library", async () => undefined);
  await library.startScan(true);
  library.onScanProgress(progress());
  library.onScanFailed("disk went away");
  assert.deepEqual({ ...library.scan }, { phase: "failed", message: "disk went away" });
  assert.equal(library.scanning, false);
  library.dismissScanFailure();
  assert.equal(library.scan.phase, "idle");
});

test("a finished scan's timer cannot clear the next scan", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const library = await store("library", async () => undefined);
  library.onScanProgress(progress({ done: true }));
  // The scanner's own final progress and scan:done both report the end.
  library.onScanProgress(progress({ done: true }));
  t.mock.timers.tick(3000);
  await library.startScan(false);
  library.onScanProgress(progress());
  t.mock.timers.tick(4000);
  assert.equal(library.scan.phase, "running");
  library.onScanProgress(progress({ done: true }));
  t.mock.timers.tick(4000);
  assert.equal(library.scan.phase, "idle");
});

test("a scan refused at the start reports why", async () => {
  const library = await store("library", async () => { throw "Finish the current tag edit first"; });
  await library.startScan(false);
  assert.equal(library.scan.phase, "failed");
  assert.match(library.scan.message, /tag edit/);
});

test("a reload finds a scan that is still running", async () => {
  const library = await store("library", libraryCalls({
    library_stats: () => ({ tracks: 0, albums: 0, total_duration_ms: 0 }),
    scan_running: () => true,
  }));
  await library.init();
  assert.equal(library.scan.phase, "running");
  assert.equal(library.scanning, true);
});
