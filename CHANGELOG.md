# Changelog

## v0.1.7 (experimental)

> [!IMPORTANT]
> v0.1.7 migrates managed storage schemas 1 through 5 to the direct-source schema 6
> in place. Profiles, personal mod selections, configured original sources, and valid
> published profile instances are preserved. Obsolete immutable-base data is removed
> only after canonical containment and reparse checks.

- Record each original Among Us source by its exact canonical path, complete tree
  fingerprint, file count, byte count, and observed build.
- Keep immutable source records for every canonical path and fingerprint referenced by
  settings or a published workspace. Workspace markers bind to the exact historical
  record, so registering a newer build cannot invalidate an older playable instance.
- Build each profile's isolated direct instance from the selected original source in a
  temporary workspace. Verify the copied fingerprint, re-fingerprint the source before
  applying profile overlays, then publish the instance atomically.
- Reuse an existing valid published instance without copying the original source again.
  Keep it launchable when the source is unavailable; when the source is reachable, block
  launch only if its observed build differs from the saved source record.
- Require the exact recorded source fingerprint for every operation that constructs or
  repairs a direct instance, with bounded copies, mutation locks, crash recovery, exact
  path checks, and source/storage containment enforcement.
- Migrate every referenced schema 1 through 5 base generation independently. Hash the
  complete legacy manifest and defer obsolete-base cleanup while any global vanilla
  workspace, unmigrated marker, unavailable source, or unverified generation remains.
- Launch only validated managed workspaces across native Windows, Proton, Wine,
  CrossOver, Whisky, and Bottles; track running processes by exact workspace path so
  Steam, Epic, and other profiles can run concurrently without locking the rest of the
  app, while file-changing actions remain blocked only for the affected profile.
- Launch CrossOver bottles on macOS through CrossOver's bundled `wine` wrapper instead
  of the removed `cxrun` command; discover system, user, versioned, and `CX_ROOT`
  installations, preserve the selected bottle, and report a missing installation before
  attempting an unavailable command.
- Start CrossOver with bottle updates disabled, keep startup attached through its
  `--wait-children` wrapper, and pass the `winhttp` override through CrossOver's
  supported command-line option so BepInEx receives its loader hook.
- Watch CrossOver's wrapper and exact managed game process under one five-minute
  startup deadline, excluding both Perl and `winewrapper.exe` dispatchers from
  readiness checks. Slow cold bottles remain pending beyond 30 seconds; dispatcher
  failures and real timeouts clear pending state so repeat launch can retry. CrossOver
  Epic launches use the same watchdog, retain the starter's input pipe, and submit Enter
  after readiness or cleanup so the console helper cannot remain blocked.
- Recognize CrossOver's default `Y:` home mapping, custom drive letters, NT Unix paths,
  and macOS `winetemp` loader links without falling back to a global process-name match;
  ambiguous processes must hold the selected managed executable according to `lsof`.
- Continuously drain CrossOver output into bounded 8 KiB tails and include redacted
  wrapper status, stdout, and stderr when startup fails instead of returning an opaque
  five-minute timeout.
- Add opt-in diagnostic logging in Settings. When enabled, persist frontend command
  lifecycles, backend operation and launch stages, bounded CrossOver output, failures,
  runtime details, and process readiness while excluding command arguments and redacting
  user paths and token-shaped strings from app events.
- Add an Open logs action that refreshes `diagnostics.json`, redacted settings, the
  selected profile definition, and its BepInEx `LogOutput.log`, then opens the native log
  directory. Keep only the current and previous 5 MiB app logs; logging is off by default.
- Turn the running-state launch control into a scoped Stop button that terminates only
  that profile's process tree, and refresh status after a rejected launch so the Stop
  action appears immediately for a late-starting process.
- Remove harmless macOS `.DS_Store` metadata from the managed workspace root during
  recovery; unexpected files and links now report their exact path and removal action.
- Verify both x86 Steam and x64 Epic Town of Us workspaces against the exact release ZIP,
  including `MiraAPI.dll`, `touhats.bundle`, and `touhats.catalog`.
- Repair relocated Epic source metadata from durable folder evidence, expose an explicit
  storefront picker during setup, and fall back to the current x64 Epic architecture.
- Rebind a renamed original source only after its full manifest matches the saved record;
  preserve missing legacy build metadata by restoring it from verified migration evidence.
- Download and SHA-256-verify the pinned EpicGamesStarter helper, launch it with a usable
  interactive console, dismiss its success prompt automatically, and preserve concurrently
  running Steam profiles while the authenticated Epic workspace starts.
- Keep launch progress visible until the exact managed Epic process starts; never submit
  helper input because an unrelated Among Us session is running, and surface early helper
  exits or authentication timeouts instead of reporting a false launch success.
- Launch Windows helpers and games with conventional Win32 paths instead of canonical
  `\\?\` paths so the Town of Us BepInEx/Cpp2IL build can read IL2CPP metadata from
  custom storage roots; require the game to remain alive before reporting success.
- Give native background games explicit null standard streams instead of inheriting
  stale handles from the console-less Windows app, fixing Steam launch failures with
  `ERROR_INVALID_HANDLE`.
- Persist the exact selected Among Us instance ID in each profile and restore it on
  profile switches; never silently substitute the first global Steam or Epic source.
- Switch the selected profile through a dedicated lightweight settings command instead of
  rebuilding its already-isolated workspace; launch still validates and repairs stale workspaces.
- Reuse an already-valid direct profile instance after storefront or architecture
  metadata is corrected, persist completed validation across app restarts, skip unchanged
  config publication, and refresh the active revision after capturing runtime config.
- Render immediately from the local profile/settings/catalog snapshot, move update checks
  off the startup path, and populate multi-mod release reviews incrementally with bounded
  concurrency plus on-demand access to older releases.
- Reuse HTTP clients, deduplicate and cache release metadata, stream downloads through
  verification, and retain verified mod assets in a content-addressed cache so repeated
  installs avoid redundant network and memory work.
- Refresh mod update metadata without stale cache entries or asset-size probes,
  check profiles concurrently every 30 seconds while Perfect Sync is visible,
  refresh immediately on focus, and isolate repository failures so one broken mod
  cannot block update indicators for the rest of a profile.
- Stage full profile transactions with copy-on-write hard links, commit mod toggles
  through a crash-recoverable single-file rename and atomic manifest update, and patch
  compatible active workspaces transactionally.
- Apply profile, original-source, and mod selections optimistically with coalesced,
  stale-safe persistence; preserve same-frame pending selections when source metadata is
  refreshed in place.
- Publish source metadata before atomically committing Settings, keep current runtime state
  authoritative when a Settings write has an ambiguous durability error, and merge rescans
  without replacing concurrently selected profiles, sources, or mod state.
- Reject every containment overlap between managed storage and an original Among Us
  source before direct-instance construction, preventing recursive copies on any drive.
- Refresh fingerprint, file count, byte count, build, architecture, runtime, writable
  state, and executable identity from each exact saved source path without substituting
  a stale duplicate installation.
- Replace only Town of Us package files owned by its prior managed manifest; preserve
  user-owned plugins and configuration by rejecting conflicting package installs.
- Treat automatic update discovery as best-effort so a missing or temporarily
  unavailable release manifest cannot interrupt application startup.
- Auto-detect only valid original Among Us sources without known loader or mod artifacts;
  manually inspected sources report exact contamination and recovery guidance.
- Let first-time setup and Settings place direct profile instances and package caches in
  an empty custom folder. Relocation copies and SHA-256-verifies every file before
  switching, rejects source-path aliases across platforms, and preserves the previous
  location on failure.
- Relocation rejects linked payload files, hashes the source again after copying, and
  requires exact cleanup-token and tree-manifest evidence before each deletion. Save or
  cleanup failures disarm every reachable capability and retain both copies with explicit
  recovery paths.
- Add a Settings action that exports the selected direct profile instance's bounded,
  non-linked BepInEx `LogOutput.log` through the native save dialog.
- Add an explicit original-source check that refreshes source fingerprint and build
  metadata in place without clearing installed mods, the active profile, the selected
  instance, or pending optimistic changes.
- Refresh a changed but build-compatible clean original source automatically before mod
  installs and updates, preserving unrelated Settings fields. Continue blocking actual
  build, architecture, storefront, unavailable-source, and contaminated-source changes.
- Keep all five live operation stages aligned in one progress row.

## v0.1.6 (experimental)

> [!IMPORTANT]
> **Action required after updating:** v0.1.6 performs a one-time reset of every
> existing profile, personal mod selection, configured game source, immutable base,
> and private workspace, then opens first-time setup. Select a fresh, untouched Among
> Us installation; verify or reinstall it through Steam, Epic, or Microsoft Store first
> if necessary. The chosen large-data storage location and GitHub credential are kept.

- Treat every selected Among Us folder as a read-only source; reject known existing
  mod-loader artifacts rather than silently filtering them, then import every source
  file into an exact SHA-256-manifested immutable base under local app data.
- Version immutable bases by source identity, executable, build, and content; retain
  historical generations required by profiles, migrate compatible legacy bases, and
  garbage-collect unreferenced generations after successful use.
- Materialize every prepared profile into its own disposable private workspace with
  verified file copies, profile-specific config persistence, crash recovery, and atomic
  replacement that preserves both the immutable base and previous working instance.
- Launch only validated managed workspaces across native Windows, Proton, Wine,
  CrossOver, Whisky, and Bottles; track running processes by exact workspace path so
  Steam, Epic, and other profiles can run concurrently without locking the rest of the
  app, while file-changing actions remain blocked only for the affected profile.
- Verify both x86 Steam and x64 Epic Town of Us workspaces against the exact release ZIP,
  including `MiraAPI.dll`, `touhats.bundle`, and `touhats.catalog`.
- Repair relocated Epic source metadata from durable folder evidence, expose an explicit
  storefront picker during setup, and fall back to the current x64 Epic architecture.
- Download and SHA-256-verify the pinned EpicGamesStarter helper, launch it with a usable
  interactive console, dismiss its success prompt automatically, and preserve concurrently
  running Steam profiles while the authenticated Epic workspace starts.
- Keep launch progress visible until the exact managed Epic process starts; never submit
  helper input because an unrelated Among Us session is running, and surface early helper
  exits or authentication timeouts instead of reporting a false launch success.
- Launch Windows helpers and games with conventional Win32 paths instead of canonical
  `\\?\` paths so the Town of Us BepInEx/Cpp2IL build can read IL2CPP metadata from
  custom storage roots; require the game to remain alive before reporting success.
- Give native background games explicit null standard streams instead of inheriting
  stale handles from the console-less Windows app, fixing Steam launch failures with
  `ERROR_INVALID_HANDLE`.
- Persist the exact selected Among Us instance ID in each profile and restore it on
  profile switches; never silently substitute the first global Steam or Epic source.
- Switch the selected profile through a dedicated lightweight settings command instead of
  rebuilding its already-isolated workspace; launch still validates and repairs stale workspaces.
- Reuse the same immutable base after storefront or architecture metadata is corrected,
  persist completed validation across app restarts, skip unchanged config publication,
  and refresh the active revision after capturing runtime config so repeat launches do
  not recopy or rehash the clean game.
- Treat the selected Town of Us ZIP as authoritative when installing, updating,
  or switching storefront packages: replace and claim every managed payload file,
  including the Region Install config, instead of rejecting an unowned collision.
- Treat automatic update discovery as best-effort so a missing or temporarily
  unavailable release manifest cannot interrupt application startup.
- Auto-detect only fresh Among Us sources without known loader or mod artifacts;
  manually inspected sources still report their exact contamination warning.
- Let first-time setup and Settings place managed bases, profile workspaces, and package
  caches in an empty custom folder. Relocation copies and SHA-256-verifies every file
  before switching, rejects source-path aliases across platforms, and preserves the
  previous location on failure.
- Add a Settings action that exports the selected profile workspace's bounded,
  non-linked BepInEx `LogOutput.log` through the native save dialog.
- Prompt existing users to rerun setup until a fresh source has been selected for the
  exact immutable-base workflow.
- Keep all five live operation stages aligned in one progress row.

## v0.1.5 (experimental)

- Let catalog bundles declare the dependency packages they provide, so install review,
  batch installs, direct installs, lobby imports, and update checks reuse the bundle
  instead of downloading a second owner for the same plugin files.
- Add AleLuduMod and Mira Submerged to the community catalog, and add AUnlocker as a
  trusted catalog mod.
- Present the product as **Perfect Sync** throughout the app, installers, release
  packages, and public documentation while retaining the `Perfect-Sync` repository
  identity and existing profile storage path.

## v0.1.4 (experimental)

- Make the selected profile authoritative by detecting every unmanaged DLL that BepInEx
  would load and blocking setup or launch until each extra is resolved.
- Let users select unmanaged plugins individually, keep safe root-level DLLs in the
  profile, quarantine selected files with checksummed recovery manifests, or permanently
  delete selected files after an explicit confirmation.
- Recover a registered game instance after its folder is renamed by matching the stable
  Among Us executable identity instead of leaving profiles bound to the deleted path.
- Keep the download bar, byte counts, percentage label, and accessibility values in
  exact agreement, and show 100% only after every reported byte has arrived.

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
- Reuse enabled, version-compatible profile dependencies during install review
  instead of forcing users to select and reinstall the same library.
- Treat Town of Us - Mira as optional for Divani's add-on, Town Of Extra,
  TOU-Mira Roles Extension, and Draft Mode: warn when it is absent, but never
  select or install that main mod automatically.
- Apply folder changes from Settings to the live setup flow immediately, replace
  stale setup selections, and keep profile-bound instances on the safe **Change**
  path when an old game folder was deleted.
- Make a full Windows uninstall remove Perfect Sync profiles, settings, caches,
  logs, its saved GitHub credential, protocol registration, and per-install
  compatibility records while preserving user state during in-place updates.

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
  Silicon Perfect Sync builds can launch the Windows game through supported Proton,
  Wine, Bottles, CrossOver, or Whisky configurations.
- Detect native and Flatpak Steam, Epic, Wine/Proton, CrossOver, Whisky, and Bottles
  locations, with writable manually selected game copies as a fallback. Persist the
  selected runtime and classify compatibility paths before generic Steam paths.
- Keep profiles and their managed DLLs under host application data. Transactionally
  publish only Perfect Sync-owned plugin DLLs into a game copy, preserve unmanaged
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
- Disable Perfect Sync's owned Doorstop entry point for vanilla launch, restore it if
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
  maps while preserving maps installed outside Perfect Sync.
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
  must be copied to a normal writable folder before Perfect Sync can manage that copy.
- Mod HTTPS, size, SHA-256-when-supplied, and archive checks detect integrity failures,
  not publisher identity. Mod assets are not cryptographically signed by their
  publishers through Perfect Sync.
- Windows Authenticode signing, macOS Developer ID signing/notarization, and a signed
  automatic updater are not available in v0.1.0; updates are notifications followed by
  a manual release-page download.
