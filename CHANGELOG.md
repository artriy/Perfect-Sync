# Changelog

## v0.1.0 (experimental)

This is an early, experimental release. Nothing here is official or stable yet:
expect rough edges, breaking changes, and behavior that may differ across Among Us
versions, stores, and compatibility runtimes.

### What's in it

- Windows 10/11 x64 is the primary target. Native Linux x86_64 and macOS Intel/Apple
  Silicon Perfect-Sync builds can launch the Windows game through supported Proton,
  Wine, Bottles, CrossOver, or Whisky configurations.
- Detect native and Flatpak Steam, Epic, Wine/Proton, CrossOver, Whisky, and Bottles
  locations, with writable manually selected game copies as a fallback. Persist the
  selected runtime and classify compatibility paths before generic Steam paths.
- Keep profiles and their managed DLLs under host application data. Transactionally
  publish only Perfect-Sync-owned plugin DLLs into a game copy, preserve unmanaged
  files, reject unmanaged name collisions, and deliberately replace stale owned DLLs
  when a profile or version changes.
- Atomically persist settings, profiles, catalogs, plugin files, compatibility registry
  changes, loader caches, and game publications. Mutations stage replacements and
  restore prior state on failure, retaining an explicit recovery backup if rollback
  itself cannot complete.
- Validate loader archives and paths under strict bounds, install BepInEx with rollback,
  auto-refresh its cached build when online, and keep a previously verified loader
  available when refresh fails.
- Resolve and reconcile catalog dependency graphs for installs, updates, removals,
  personal mods, and lobby application rather than leaving partial dependency state.
- Disable Perfect-Sync's owned Doorstop entry point for vanilla launch, restore it if
  that launch fails, and restore mod loading before the next managed setup or launch.
- Provide named profiles, per-profile game-instance assignment, multiple role mods,
  release selection, lobby/share codes, per-mod diffs, and explicit confirmation for
  unknown direct assets or lobby mods.
- Require HTTPS for network resources; verify release asset size and any same-operation
  SHA-256 metadata; enforce bounded ZIP/path extraction; and pin both the archive and
  extracted shape, size, and SHA-256 for the Epic launch helper. Mod integrity checks
  are not publisher signatures or proof of authenticity.
- Store an optional GitHub token in the OS keyring, migrate and scrub a legacy
  `settings.json` token, and limit credential forwarding to GitHub HTTPS hosts.
- Harden desktop boundaries with a restrictive production CSP, validated canonical
  update links, registered `perfectsync:` lobby deep links, and a notifier that opens
  the manual GitHub release download rather than auto-updating.
- Make dialogs, controls, focus management, reduced motion, status announcements, and
  async UI sessions more reliable and accessible, including stale-result suppression
  when a dialog or profile session changes.
- Build Windows x64, Linux x86_64, macOS x64, and macOS arm64 packages in release CI,
  with locked workspace tests on every build host and production npm and RustSec
  dependency audits. A release remains unpublished until all required builds, bundle
  validations, exact installer-asset checks, and immutable tag checks complete, then is
  published as an experimental prerelease.

### Known limitations

- Windows packages are unsigned, so SmartScreen can warn or block first run.
- macOS packages are ad-hoc signed, not Developer ID signed or notarized; Gatekeeper
  can require an explicit user override or quarantine removal.
- CI validates compilation, packaging, unit/integration behavior, filesystem/runtime
  logic, and constructed launch commands. It is not real-machine game validation.
  Linux/macOS launch integrations remain experimental until representative hardware,
  store clients, game versions, and compatibility runtimes are exercised end to end.
- Native Microsoft Store / Game Pass files live under protected `WindowsApps` ACLs and
  must be copied to a normal writable folder before Perfect-Sync can manage that copy.
- Mod HTTPS, size, SHA-256-when-supplied, and archive checks detect integrity failures,
  not publisher identity. Mod assets are not cryptographically signed by their
  publishers through Perfect-Sync.
- Windows Authenticode signing, macOS Developer ID signing/notarization, and a signed
  automatic updater are not available in v0.1.0; updates are notifications followed by
  a manual release-page download.
