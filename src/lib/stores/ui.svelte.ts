export type Tab = "library" | "playlists" | "nowplaying" | "visualizer";

class UiStore {
  activeTab = $state<Tab>("playlists");
  searchQuery = $state("");
  searchInvalid = $state(false);
  searchHelpOpen = $state(false);
  /** left time readout shows remaining (foobar-style "-3:30") vs elapsed */
  timeRemaining = $state(false);
  /** panel sizes in px, persisted via prefs later */
  bottombarH = $state(76);
  playlistSidebarW = $state(230);
  queuePanelW = $state(260);
  libraryValueListW = $state(230);
}

export const ui = new UiStore();
