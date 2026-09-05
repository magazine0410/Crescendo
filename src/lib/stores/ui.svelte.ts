export type Tab = "library" | "playlists" | "nowplaying" | "visualizer";

export type SearchSpan = { start: number; end: number; kind: "plain" | "syntax" | "invalid" };

class UiStore {
  activeTab = $state<Tab>("playlists");
  /** Tab to return to when an album was opened through cross-tab navigation. */
  albumReturnTab = $state<Tab | null>(null);
  searchQuery = $state("");
  /** How to tint the search input, one span per top-level term: plain free
      text, valid syntax, or a parse error. Byte-free character offsets into
      the query, so the text between two spans stays uncoloured. */
  searchSpans = $state<SearchSpan[]>([]);
  searchHelpOpen = $state(false);
  /** left time readout shows remaining (foobar-style "-3:30") vs elapsed */
  timeRemaining = $state(false);
  /** panel sizes in px, persisted via prefs later */
  bottombarH = $state(76);
  playlistSidebarW = $state(230);
  queuePanelW = $state(260);
  libraryValueListW = $state(230);
  /** Side-panel visibility is persisted separately from the last open width. */
  librarySidebarCollapsed = $state(false);
  playlistSidebarCollapsed = $state(false);
  /** The same queue panel is shown in both Library and Playlists. */
  queuePanelCollapsed = $state(false);
  /** Prevent persisted sizes/states from animating in after the first paint. */
  layoutReady = $state(false);
  /** Bumped once a setting that changes how lyrics resolve has finished
      saving, so the Now playing pane re-looks-them-up straight away instead
      of waiting for the next track. */
  lyricsEpoch = $state(0);
}

export const ui = new UiStore();
