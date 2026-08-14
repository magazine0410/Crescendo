export type Tab = "library" | "playlists" | "nowplaying" | "visualizer";

class UiStore {
  activeTab = $state<Tab>("playlists");
  searchQuery = $state("");
  /** how to tint the search input: plain free text, valid syntax, or a parse error */
  searchKind = $state<"plain" | "syntax" | "invalid">("plain");
  searchHelpOpen = $state(false);
  /** left time readout shows remaining (foobar-style "-3:30") vs elapsed */
  timeRemaining = $state(false);
  /** panel sizes in px, persisted via prefs later */
  bottombarH = $state(76);
  playlistSidebarW = $state(230);
  queuePanelW = $state(260);
  libraryValueListW = $state(230);
  /** Bumped once a setting that changes how lyrics resolve has finished
      saving, so the Now playing pane re-looks-them-up straight away instead
      of waiting for the next track. */
  lyricsEpoch = $state(0);
}

export const ui = new UiStore();
