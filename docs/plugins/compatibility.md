# Plugin compatibility and versioning

Crescendo uses three independent versions in `manifest.json`.

`schema_version` identifies the manifest shape. This release accepts exactly
version 1. A different value, an unknown field, or an unknown contribution is a
validation error; Crescendo leaves the package disabled and reports the error.

`api_version` identifies contribution semantics. The host currently implements
`1.0.0`. A plugin is compatible when its API major matches the host and its
version is not newer than the host API. For example, an API `1.0.0` package is
accepted by host `1.1.0`, while `1.1.0` is not accepted by host `1.0.0`, and
API `2.x` requires an explicit new compatibility contract.

`min_crescendo_version` is the oldest application version the author supports.
It must not be newer than the running Crescendo version. The package can remain
installed when incompatible, but it cannot be enabled.

The package `version` is the plugin's own semantic version. Updating an
installed plugin requires the same plugin ID and a strictly higher package
version. Use patch releases for compatible fixes, minor releases for compatible
new contributions or settings, and a major release when plugin behavior or
stored-data expectations change incompatibly.

Package metadata in `manifest.json` is authoritative. Crescendo stores the
desired enabled state, pending lifecycle journal, package SHA-256, and
namespaced setting values separately. Uninstalling may keep settings for a
future reinstall or remove them at the user's choice.

The declarative contribution set is immutable for a running process. First
install is immediately listed but disabled; install, update, enable, disable,
and uninstall changes are reflected after the required restart. Values for
declared settings use a guarded write-then-publish path and apply live. Views
and newly created play contexts carry a projection revision so one response
cannot combine old and new projection semantics.

