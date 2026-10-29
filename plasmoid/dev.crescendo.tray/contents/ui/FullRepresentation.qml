/*
    The popup: cover art filling the view, darkened toward the bottom, with
    the album artist, title, a seek bar and the transport controls on the
    dark part. Plasma draws the frame, the heading and the pin around it.
*/
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC3

Item {
    id: full

    readonly property var player: root.player
    // Microseconds, like MPRIS; the slider is the single source of truth
    // between position updates from the player.
    readonly property double length: player?.length ?? 0

    Layout.preferredWidth: Kirigami.Units.gridUnit * 22
    Layout.preferredHeight: Kirigami.Units.gridUnit * 26
    Layout.minimumWidth: Kirigami.Units.gridUnit * 16
    Layout.minimumHeight: Kirigami.Units.gridUnit * 18

    // Text and controls sit on a dark overlay whatever the desktop theme is.
    Kirigami.Theme.inherit: false
    Kirigami.Theme.colorSet: Kirigami.Theme.Complementary

    function formatTime(us) {
        const seconds = Math.max(0, Math.floor(us / 1000000));
        const minutes = Math.floor(seconds / 60);
        return minutes + ":" + String(seconds % 60).padStart(2, "0");
    }

    function syncPosition() {
        if (player !== null) {
            seek.value = player.position;
        }
    }

    Rectangle {
        anchors.fill: parent
        color: "#1b1e20"
    }

    Image {
        id: art
        anchors.fill: parent
        source: full.player?.artUrl ?? ""
        fillMode: Image.PreserveAspectCrop
        asynchronous: true
        cache: false
        smooth: true
        visible: status === Image.Ready
    }

    Kirigami.Icon {
        anchors.centerIn: parent
        anchors.verticalCenterOffset: -Kirigami.Units.gridUnit * 3
        width: Kirigami.Units.iconSizes.enormous
        height: width
        source: "crescendo"
        opacity: 0.35
        visible: !art.visible
    }

    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.00; color: "transparent" }
            GradientStop { position: 0.28; color: "#1a000000" }
            GradientStop { position: 0.55; color: "#80000000" }
            GradientStop { position: 0.78; color: "#c2000000" }
            GradientStop { position: 1.00; color: "#d9000000" }
        }
    }

    ColumnLayout {
        anchors {
            left: parent.left
            right: parent.right
            bottom: parent.bottom
            margins: Kirigami.Units.largeSpacing
        }
        spacing: Kirigami.Units.smallSpacing

        PC3.Label {
            Layout.fillWidth: true
            text: full.player?.artist ?? ""
            opacity: 0.72
            elide: Text.ElideRight
            visible: text.length > 0
        }

        Kirigami.Heading {
            Layout.fillWidth: true
            level: 3
            text: root.hasTrack ? full.player.track : i18n("Nothing playing")
            elide: Text.ElideRight
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing

            PC3.Label {
                text: full.formatTime(seek.value)
                opacity: 0.72
                font: Kirigami.Theme.smallFont
            }

            PC3.Slider {
                id: seek
                Layout.fillWidth: true
                from: 0
                to: Math.max(full.length, 1)
                stepSize: 1000000
                enabled: full.player?.canSeek ?? false
                onMoved: queuedSeek.restart()
            }

            PC3.Label {
                text: full.formatTime(full.length)
                opacity: 0.72
                font: Kirigami.Theme.smallFont
            }
        }

        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            spacing: Kirigami.Units.largeSpacing

            PC3.ToolButton {
                icon.name: "media-skip-backward"
                display: PC3.AbstractButton.IconOnly
                enabled: full.player?.canGoPrevious ?? false
                onClicked: full.player.Previous()
                PC3.ToolTip.text: i18n("Previous")
                PC3.ToolTip.visible: hovered
            }

            PC3.ToolButton {
                id: stopButton
                icon.name: "media-playback-stop"
                display: PC3.AbstractButton.IconOnly
                enabled: full.player?.canControl ?? false
                onClicked: full.player.Stop()
                PC3.ToolTip.text: root.stopAfterCurrent ? i18n("Stop (stopping after this song)") : i18n("Stop")
                PC3.ToolTip.visible: hovered

                // Right-click offers the one-shot stop; left clicks fall
                // through to the button.
                MouseArea {
                    anchors.fill: parent
                    acceptedButtons: Qt.RightButton
                    onClicked: stopMenu.popup()
                }

                Rectangle {
                    anchors.top: parent.top
                    anchors.right: parent.right
                    width: Kirigami.Units.smallSpacing * 2
                    height: width
                    radius: width / 2
                    color: Kirigami.Theme.highlightColor
                    visible: root.stopAfterCurrent
                }
            }

            PC3.ToolButton {
                icon.name: root.isPlaying ? "media-playback-pause" : "media-playback-start"
                icon.width: Kirigami.Units.iconSizes.medium
                icon.height: Kirigami.Units.iconSizes.medium
                display: PC3.AbstractButton.IconOnly
                enabled: full.player?.canControl ?? false
                onClicked: full.player.PlayPause()
                PC3.ToolTip.text: root.isPlaying ? i18n("Pause") : i18n("Play")
                PC3.ToolTip.visible: hovered
            }

            PC3.ToolButton {
                icon.name: "media-skip-forward"
                display: PC3.AbstractButton.IconOnly
                enabled: full.player?.canGoNext ?? false
                onClicked: full.player.Next()
                PC3.ToolTip.text: i18n("Next")
                PC3.ToolTip.visible: hovered
            }
        }

        PC3.Label {
            Layout.alignment: Qt.AlignHCenter
            text: i18n("Stopping after this song")
            color: Kirigami.Theme.highlightColor
            font: Kirigami.Theme.smallFont
            visible: root.stopAfterCurrent
        }
    }

    PC3.Menu {
        id: stopMenu
        PC3.MenuItem {
            checkable: true
            checked: root.stopAfterCurrent
            text: i18n("Stop after this song")
            onTriggered: root.setStopAfterCurrent(!root.stopAfterCurrent)
        }
    }

    // Advance the bar once a second while playing; the player only reports
    // its position on seeks and track changes.
    Timer {
        interval: 1000
        repeat: true
        running: root.isPlaying && !seek.pressed
        onTriggered: seek.value = Math.min(seek.value + 1000000, full.length)
    }

    // Send the dragged position after the hand stops, not on every pixel.
    Timer {
        id: queuedSeek
        interval: 100
        onTriggered: {
            if (full.player !== null) {
                full.player.position = seek.value;
            }
        }
    }

    Connections {
        target: full.player
        function onPositionChanged() { full.syncPosition(); }
        function onPlaybackStatusChanged() { full.player.updatePosition(); }
        function onTrackChanged() { full.player.updatePosition(); }
    }

    onPlayerChanged: syncPosition()
    Component.onCompleted: syncPosition()
}
