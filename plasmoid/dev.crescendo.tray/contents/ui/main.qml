/*
    Crescendo system-tray widget: the playing song and transport controls
    without raising the main window.

    Everything shown comes from Crescendo's MPRIS interface through Plasma's
    own MPRIS model, the same one the stock Media Player widget uses. The one
    thing MPRIS has no word for, "Stop after this song", is read and written
    through the dev.crescendo.Player interface Crescendo serves on the same
    D-Bus object.
*/
import QtQuick
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.private.mpris as Mpris
import org.kde.plasma.workspace.dbus as DBus

PlasmoidItem {
    id: root

    // Crescendo's player container, or null while it is not on the bus.
    property var player: null
    readonly property bool hasTrack: player !== null && (player.track ?? "").length > 0
    readonly property bool isPlaying: player !== null && player.playbackStatus === Mpris.PlaybackStatus.Playing
    readonly property bool stopAfterCurrent: Boolean(crescendo.properties.StopAfterCurrent)

    Plasmoid.icon: "crescendo"
    Plasmoid.status: player !== null ? PlasmaCore.Types.ActiveStatus : PlasmaCore.Types.PassiveStatus
    toolTipMainText: hasTrack ? player.track : "Crescendo"
    toolTipSubText: hasTrack ? (player.artist ?? "") : ""
    fullRepresentation: FullRepresentation {}

    Mpris.Mpris2Model {
        id: mpris2Model
    }

    // The model lists every MPRIS player on the bus; pick Crescendo by the
    // desktop entry it reports, and look again whenever players come or go.
    function refreshPlayer() {
        player = mpris2Model.playerForLauncherUrl("applications:crescendo.desktop", 0);
    }

    Connections {
        target: mpris2Model
        function onRowsInserted() { root.refreshPlayer(); }
        function onRowsAboutToBeRemoved() { root.player = null; }
        function onRowsRemoved() { root.refreshPlayer(); }
    }

    // A player row can appear before its properties have been fetched, so
    // keep looking while Crescendo is on the bus but not yet matched.
    Timer {
        interval: 1000
        repeat: true
        running: root.player === null
        onTriggered: root.refreshPlayer()
    }

    Component.onCompleted: refreshPlayer()

    DBus.Properties {
        id: crescendo
        busType: DBus.BusType.Session
        service: "org.mpris.MediaPlayer2.crescendo"
        path: "/org/mpris/MediaPlayer2"
        iface: "dev.crescendo.Player"
    }

    // Plasma's D-Bus module cannot build the variant a property write
    // needs, so Crescendo also offers the write as a plain method.
    function setStopAfterCurrent(on) {
        DBus.SessionBus.asyncCall({
            "service": crescendo.service,
            "path": crescendo.path,
            "iface": crescendo.iface,
            "member": "SetStopAfterCurrent",
            "arguments": [new DBus.bool(on)]
        }, () => {}, (reply) => {
            console.warn("Crescendo widget: could not set StopAfterCurrent:", reply.error.message);
        });
    }
}
