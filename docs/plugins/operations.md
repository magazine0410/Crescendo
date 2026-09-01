# Installing and troubleshooting plugins

## Install and lifecycle

Open **Settings → Plugins → Install plugin** and select a
`.crescendo-plugin` file. Crescendo copies it into a private staging operation,
checks its SHA-256, ZIP structure, paths, limits, manifest, compatibility, and
content policy, then promotes a first install atomically. New plugins are always
installed disabled. Review the displayed author, license, homepage,
compatibility, package hash, contributions, and settings before enabling one.

Enabling or disabling a plugin changes the desired loaded set and shows a
restart notice. Updates and removal of a currently loaded plugin are journaled
and completed on the next start. A disabled plugin can be removed immediately.
Setting values apply live; changing them does not require a restart or replace
the current playback queue.

The official Primary Grouping package is published with Crescendo releases and
its source lives under `plugins/dev.crescendo.primary-grouping/`. Existing
installations that had the former collapse preference receive a one-time offer
to install it. Crescendo never silently installs or enables the package. Acting
on or dismissing that offer permanently completes the migration notice.

## Safe mode and recovery

Crescendo writes a startup marker before it recovers lifecycle journals and
builds the immutable loaded registry. If the previous startup did not finish,
the next launch enters plugin safe mode for one run: installed packages remain
available for inspection, but no contributions load. A clean following launch
tries the desired set again.

To force the same diagnostic mode, start Crescendo from a terminal with:

```bash
CRESCENDO_DISABLE_PLUGINS=1 crescendo
```

Remove the variable on the next run. This is an escape hatch, not a persistent
setting.

Plugin diagnostics in Settings show malformed or incompatible installed
packages without preventing the rest of Crescendo from starting. When an
interrupted update is incomplete or fails installed-package validation,
Crescendo restores the previous package and records an error. Journaled update
and uninstall transitions are idempotent across interruption; orphan staging
operations are removed after startup recovery.

## Common failures

- **Package rejected before install:** run `crescendo-plugin validate` against
  the package. Check the extension, archive/entry limits, root layout, links,
  active asset types, semantic versions, and referenced setting keys.
- **Installed but cannot enable:** inspect compatibility details and confirm no
  enabled plugin already owns the same facet target.
- **Change appears pending:** contribution-set changes intentionally require a
  restart. Setting values do not.
- **Plugin disappeared from the loaded set:** inspect Plugin diagnostics. A
  malformed package is isolated; it cannot block application startup.
- **Repeated startup trouble:** run once with `CRESCENDO_DISABLE_PLUGINS=1`,
  disable or uninstall the suspect package, then launch normally.

The About and cover windows have no plugin-management command grants. All
plugin management commands are restricted to the main window by Tauri's
generated application-command permissions and capability files.

