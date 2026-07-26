# Changelog

## v0.1.3 (experimental)

- Install Town of Us - Mira as one target-specific release package, including its
  matching BepInEx, dependencies, configs, cosmetics, and fixed UnityDoorstop.
- Keep pre-existing MiraAPI, Reactor, and Mini.RegionInstall profile copies dormant
  while Town of Us is active, then restore them automatically if Town of Us is
  disabled or removed.
- Offer the separate UnityDoorstop 4.5.1 compatibility fix only for BepInEx-only
  setup and repair flows, with the option disabled by default.
- Let full Town of Us release downloads run beyond the metadata request deadline
  while retaining bounded connection and inactivity timeouts.
- Allow slow release downloads up to ten minutes of inactivity and sixty seconds
  to connect before reporting a network timeout.
- Reuse an already-installed, complete Town of Us package when adding one of its
  extensions instead of resolving and downloading the same release again.
- Put The Other Roles, Town of Us - Mira, Town of Host - Enhanced, Town Of Extra,
  and LevelImposter first in the default catalog order, with dependency packages
  at the bottom.
- Remove the Health & maintenance control and panel from the app.
- Show the same live, stage-aware download progress during first-time BepInEx
  and Town of Us setup that later mod installs use.
- Keep setup's local settings and profile state synchronized after a partial
  install failure, and make **Skip setup** dismiss without resubmitting stale
  game-instance data.
- Accept pinned BepInEx build identifiers containing SemVer `+` metadata in the
  validated loader cache path.
- Warn before installs, Town of Us setup, or shared-lobby application would
  combine known main mods, and require explicit incompatibility acknowledgment
  before continuing. Final Suspect is included in this safeguard.

## v0.1.2 (experimental)

This release includes the complete v0.1.1 and v0.1.0 change sets documented
below, plus:

- Adopt the Twin Relay logo across the app, browser favicon, documentation,
  platform icon bundles, executable, and Windows installers.
- Brand the NSIS installer with an explicit Twin Relay executable icon, custom
  welcome/sidebar and header artwork, and timeless publisher text.
- Embed the selected logo directly in the standard versioned setup executable;
  the v0.1.2 filename also avoids stale Windows Explorer cache entries from
  earlier installer builds.

## v0.1.1 (experimental)

- Launch EpicGamesStarter with interactive input and an isolated Legendary token
  store on Windows, Linux, and macOS, preventing Among Us's incompatible
  `EGSAuth.json` from crashing first-time Epic login while automatically closing
  the helper's final success prompt after the game starts.
- Keep a modal, stage-aware progress surface visible while applying lobby codes,
  installing mod batches, changing releases, and downloading LevelImposter maps.
- Create and synchronize managed maps at
  `BepInEx\plugins\LevelImposter`, including the directory when absent.
- List profile-installed LevelImposter maps in the map browser and allow each
  managed map to be removed and synchronized from the app.
- Automatically include and show Region Install with Town of Us - Mira alongside
  MiraAPI and Reactor, overriding stale cached dependency policy.

- Default Town of Us - Mira to `TownOfUsMira.dll`, ignore stale cached ZIP rules,
  and expose every direct DLL from the chosen release in the install review.
- Provision and verify `touhats.bundle` and `touhats.catalog` from the exact
  selected Town of Us - Mira release pack before launch, replacing them when
  the selected mod version changes and using a conventional Win32 working path
  so Epic's Addressables runtime resolves the `touhats` catalog key.
- Retry LevelImposter banners through a bounded native image proxy when WebView loading
  fails, and keep Windows verbatim path prefixes out of every user-facing label.
- Display the application version in the title bar so corrected builds are identifiable.
- Accept current signed LevelImposter `.lim` map assets from the official storage
  bucket while retaining strict host, bucket, map ID, and size checks.
- Allow exact release changes for installed managed dependencies and reset release
  picker state after each completed install so another version can be chosen immediately.
- Close Add Mod, Settings, and release dialogs by clicking their shaded backdrop.
- Resolve GitHub releases through API-free release pages, Atom feeds, and expanded
  asset metadata, eliminating normal catalog use of the 60-request REST quota.
- Install catalog-selected ZIP packages by extracting exactly one declared plugin
  DLL with traversal, duplicate, symlink, entry-count, and expanded-size checks.
- Enforce semver requirements for shared dependencies while leaving top-level mod
  combinations under user control, selecting the newest release that satisfies every root.
- Prioritize the catalog-recommended DLL for installed-mod updates while keeping
  exact repository, release, and asset confirmation plus advanced version selection.
- Save or discard instance, token, and lobby-default mod changes as one Settings
  draft, including personal-mod asset selection through the shared release picker.
- Reflow the desktop shell at the supported minimum size and high zoom, consolidate
  lobby joining into one entry point, and quiet glass, glow, and metadata hierarchy.
- Detect Microsoft Store and Game Pass installations, create bounded transactional
  writable x64 managed copies, and launch those copies through the store-aware path.
- Expand the built-in catalog to 30 maintained mods with verified repository identities
  and explicit release-asset rules where upstream projects publish stable DLL names.
- Add Health & Maintenance diagnostics, redacted support bundles, and transactional
  Innersloth save-data backups with safety backups before restore.
- Review all available mod updates as one batch and apply the confirmed set in one
  profile transaction.
- Persist local DLL defaults alongside personal GitHub defaults, merge both into new
  lobby profiles, and keep every local file out of shared lobby codes.
- Let users disable or remove automatically added dependencies. Explicit installs add
  dependencies once; lobby application follows only the exact contents of its code.
- Preserve exact mod versions, selected release assets, and LevelImposter map IDs in
  lobby codes while keeping local DLLs and game-build restrictions device-local.
- Remove pointer-triggered persistent focus rings while preserving keyboard focus
  visibility, and make add-mod selection state explicit with a stable checkmark.

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
- Select multiple catalog mods, review the latest version and default DLL for each,
  choose any other direct DLL asset from that release, override mod and dependency
  versions, include or exclude each automatic dependency, then install the confirmed
  set as one profile mutation.
- Restrict mod releases to direct `.dll` assets, rejecting mod archives at the command
  boundary. Import a local computer DLL into one profile and keep it out of lobby codes.
- Trust LevelImposter v0.21.2-beta as a catalog map loader. Open its map browser from
  either the catalog or the installed mod, search the live community index, view map
  banners, download multiple arbitrary `.lim2` maps, and synchronize profile-owned
  maps while preserving maps installed outside Perfect-Sync.
- Include each profile's exact LevelImposter map selection in lobby codes, show the map
  count during preview, and download the shared selection when applying the code.
- Adopt byte-identical unmanaged game DLLs during profile synchronization; retain and
  explain conflicts when an unmanaged DLL differs from the selected profile version.
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
