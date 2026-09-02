# Plugin security audit

The review Stage 6 requires before the Wasm runtime ships. Each section states
what the boundary is, how it is enforced, and the test that keeps it enforced.
Where a check was missing, the finding and its fix are recorded here rather
than quietly patched.

Threat model: a package the user installed deliberately but does not fully
trust. It may be hostile, buggy, or a good plugin that was updated into a bad
one. It must not read or change anything the user did not approve, must not be
able to stop Crescendo from opening or playing, and must not learn about the
library or the machine through the errors it is given.

## Capability enforcement

Every host interface except `crescendo:host/host` is gated. `host` itself
carries only this plugin's own live settings and its own bounded log, so it
needs no grant.

- The permission set is copied into the store data when the instance is
  created, from the manifest of the package the loader validated. A guest call
  cannot reach the manifest, the grant table, or another plugin's store.
- Each gated function calls `require` (or `require_playlists`, or `control`
  for transport) first and maps a refusal onto that interface's own `denied`
  variant. A denial is a value, not a trap: a plugin can handle it.
- `library.paths` is a second gate *inside* `library.tracks`: without it the
  `path` column is selected as `NULL` in SQL, so the field is not fetched and
  then dropped — it is never read.
- `network.http` additionally checks the approved domain list on every hop.

Covered by `capability_calls_are_denied_without_the_matching_permission`,
`player_capabilities_are_separate_and_need_an_installed_player`,
`playlist_reads_and_writes_are_separately_gated`, and
`network_requests_are_denied_blocked_or_allowed_by_host_not_by_the_guest`.

**Finding (fixed): denial ordering in `player.enqueue`.** Enqueue is the one
transport call that carries plugin-supplied data, and the added track-id
validation ran before the capability check. A plugin with no player permission
at all could tell an existing track id from a missing one by the difference
between `invalid-request` and `denied`. The check now comes first, and the
test asserts both id shapes answer `denied` without the grant.

## Package replacement and permission upgrades

- An update must carry the same plugin id and a strictly higher version. There
  is no downgrade or same-version replacement path.
- A grant is recorded against the plugin id, the package version, **and** the
  package SHA-256, and `approves` compares all three plus the exact permission
  and domain sets. Any change to the package — including one that only adds a
  permission — leaves no grant that matches, so the package cannot be enabled
  until the user approves it again.
- Replacement never happens under a running registry. Updates and the removal
  of a loaded plugin are journalled and applied at the next start, so the
  module backing a live instance cannot change beneath it.
- The old directory is retained in the operation's own staging area until the
  new registry is built, and restored if it is not.

Covered by `grants_are_pinned_to_the_exact_package_version_and_hash`,
`updates_reject_same_or_lower_versions`,
`startup_resumes_an_update_interrupted_after_backing_up_the_old_copy`, and
`startup_rejects_an_update_that_no_longer_matches_its_journal`.

## Resource exhaustion

Every guest-controlled quantity has a ceiling, and each ceiling is charged
against something the host owns:

| Resource | Bound |
| --- | --- |
| Linear memory | 64 MiB per instance, through the store limiter |
| Table growth | charged against the same 64 MiB budget |
| CPU per call | fuel, plus a 1 ms-tick epoch deadline |
| Pending host requests | bounded queue; a full queue drops, never blocks |
| Command input/output | 1 MiB each; a guest error string 4 KiB |
| Log and diagnostics | bounded ring, coarsened under repeat |
| Storage | 256-byte keys, 256 KiB values, 5 MiB quota, byte-accounted |
| Library reads | 2000 rows per page, windowed in SQL |
| Playlist reads | windowed in SQL, ids only |
| HTTP | HTTPS only, response size cap, one deadline across all redirects |

A trapped or timed-out command fails alone. Three failures in a session mark
the plugin unhealthy; a second startup-initialization failure disables it and
asks for one safe-mode run. None of this reaches the player: the runtime is a
separate thread and holds no player state.

Covered by `guest_work_is_bounded_by_fuel`,
`guest_work_is_bounded_by_the_epoch_deadline_and_failures_mark_it_unhealthy`,
`table_growth_is_charged_against_the_instance_budget`,
`a_broken_module_fails_isolated_and_auto_disables_after_repeated_startups`,
and `storage_is_namespaced_per_plugin_and_bounded_by_the_quota`.

## Confused deputy

The host must never do something on a plugin's behalf that the plugin could
not do itself.

- **Identity is never an argument.** No host import takes a plugin id; it comes
  from the store the call arrived on. A plugin cannot name another plugin's
  storage namespace, settings, or grants.
- **Ids the plugin supplies are validated by the host.** Playlist writes check
  the playlist and every track id in the same transaction that inserts them;
  `player.enqueue` checks every id before the queue takes it. `playlist_tracks`
  has no foreign keys, so this is the only thing standing between a plugin and
  rows pointing at nothing.
- **Dispatch reaches only declared commands.** `plugin_invoke` refuses a
  command the manifest does not declare, and a `run_command` menu action is
  validated at package time against the same list, so adding a menu item
  cannot widen what is reachable. The action carries no arguments.
- **The network guard resolves once.** Policy and connection share a single DNS
  lookup through a guarded resolver, so an approved name cannot answer public
  for the check and private for the connect, and no private address is ever
  handed to the connector.
- **Errors do not describe the host.** A blocked or unresolvable host is one
  answer, so the error cannot be used to map the user's internal network.

Covered by `storage_is_namespaced_per_plugin_and_bounded_by_the_quota`,
`playlist_writes_refuse_ids_that_do_not_exist`,
`a_run_command_menu_control_reaches_the_snapshot_and_dispatches`,
`the_resolver_refuses_an_approved_name_that_points_at_localhost`, and the
enqueue assertions above.

## Window ACL

Plugin management stays on the main window. `src-tauri/build.rs` asserts at
build time that no plugin permission appears in the About or shared
cover-window capability, and the handler/`COMMANDS` parity check fails the
build when the two lists diverge.

## What is deliberately not defended

- **Local packages are unsigned.** v1 installs a file the user chose. There is
  no signature, publisher identity, or revocation. Installation says so.
- **A granted capability is granted.** A plugin with `playlists.write` can
  write nonsense playlists, and one with `network.http` can talk to its
  approved domain as much as its rate limits allow. Capabilities bound *what*,
  not *how much*.
- **Event delivery is not gated per plugin.** Every loaded plugin receives the
  five notifications. They carry no data beyond an id, and a plugin without
  the matching capability cannot act on them; gating would only hide that
  something happened, which the plugin can already observe by polling.
