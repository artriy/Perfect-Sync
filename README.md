<div align="center">

<img src="docs/assets/banner.svg" alt="Perfect Sync" width="100%">

<br>

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-9b7bff?style=flat-square)](#platform-support)
[![Version](https://img.shields.io/badge/version-0.1.6%20experimental-7a5bff?style=flat-square)](https://github.com/artriy/Perfect-Sync/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-5bc0ff?style=flat-square)](https://tauri.app)
[![License](https://img.shields.io/badge/license-MIT-5bc0ff?style=flat-square)](LICENSE)

**An experimental desktop mod manager and launcher for modded Among Us.** It installs
BepInEx, keeps Perfect Sync-managed mods in named profiles, and lets friends preview
and apply a shared lobby mod set.

</div>

> [!NOTE]
> **Disclaimer.** Perfect Sync is an unofficial, fan-made tool. It is not affiliated with,
> endorsed by, or sponsored by Innersloth LLC. Among Us is a trademark of Innersloth LLC.
> Use modded clients only in private and modded lobbies. Do not use mods to disrupt public
> or vanilla games. See the [Among Us mod policy](https://www.innersloth.com/among-us-mod-policy/).

<img src="docs/assets/divider.svg" alt="" width="100%">

## Features

<table>
  <tr>
    <td align="center" width="33%" valign="top">
      <img src="docs/assets/glyph-bepinex.svg" width="56" alt=""><br>
      <b>One-click BepInEx</b><br>
      <sub>Installs and verifies the mod loader in a writable game copy, with rollback-safe replacement.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <img src="docs/assets/glyph-profiles.svg" width="56" alt=""><br>
      <b>Mod profiles</b><br>
      <sub>Keep multiple named mod sets, toggle mods, swap versions, and resume the last one you used.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <img src="docs/assets/glyph-lobby.svg" width="56" alt=""><br>
      <b>Lobby codes</b><br>
      <sub>Export a profile as a short PERFECT- code. Friends preview its requested mod set before applying it.</sub>
    </td>
  </tr>
  <tr>
    <td align="center" width="33%" valign="top">
      <img src="docs/assets/glyph-trust.svg" width="56" alt=""><br>
      <b>Trust tiers</b><br>
      <sub>Distinguishes curated catalog metadata, community listings, and unknown sources before installation.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <img src="docs/assets/glyph-catalog.svg" width="56" alt=""><br>
      <b>Catalog and any repo</b><br>
      <sub>Add popular mods from the built-in catalog, or paste any GitHub repo and pick the exact release.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <img src="docs/assets/glyph-launch.svg" width="56" alt=""><br>
      <b>One-click launch</b><br>
      <sub>Publishes the selected managed profile, verifies BepInEx, and starts the modded game in a single click.</sub>
    </td>
  </tr>
</table>

Plus architecture detection (x86 or x64 from the game executable), a global list of
named Among Us instances with one assigned independently to each profile, and persistent
personal GitHub or local-DLL defaults merged into every applied lobby profile. Local DLLs
remain device-only and are never serialized into lobby codes.

> [!TIP]
> **The selected Among Us folder is a read-only source.** Perfect Sync never installs
> BepInEx, mods, configs, or compatibility files into it. Before the first import, the
> app rejects known existing mod-loader artifacts instead of silently omitting them.
> It then records and copies every source file into a SHA-256-manifested immutable base
> under local application data. Profiles are rebuilt into one disposable private
> workspace with verified copies, so game writes cannot mutate the base. Existing
> Steam, Epic, Microsoft Store, Game Pass, manual, Wine, Proton, CrossOver, Whisky, and
> Bottles game folders remain unchanged.

<img src="docs/assets/divider.svg" alt="" width="100%">

## How it works

1. **Choose a fresh source.** On first run the wizard auto-detects only supported
   Steam, Epic, Microsoft Store, or Game Pass locations without known mod-loader
   artifacts. You can also inspect a folder containing `Among Us.exe`, then add and
   name other read-only game sources later in Settings.
2. **Build the clean base.** Perfect Sync first rejects known BepInEx and mod-loader
   artifacts, then copies every file from the selected source and verifies an exact
   SHA-256 manifest. Bases are immutable and versioned by source executable, game
   build, architecture, store, and content. Profiles pinned to an older build retain
   its base; unreferenced generations are garbage-collected after successful use.
   The wizard also lets you keep managed data in local app data or select an empty
   custom storage folder on another drive.
3. **Add mods.** Browse the catalog or enter an exact HTTPS GitHub repository URL,
   then choose a release asset. Unknown repositories and assets require explicit
   confirmation before native code is installed.
4. **Manage.** Toggle mods, change versions, create or switch profiles, and assign a
   game source to each profile. Profile-owned DLLs and mutable BepInEx configuration
   live under application data, never in the selected source.
5. **Materialize.** Perfect Sync stages a fresh workspace from the clean base, applies
   the profile's exact loader, dependencies, mods, maps, cosmetics, and saved config,
   verifies the managed files, then atomically publishes it. Failed builds leave the
   previous workspace launchable.
6. **Share or apply.** Export a `PERFECT-` code containing every enabled shareable
   mod's exact repository identity, release version, selected asset, and exact
   LevelImposter map IDs. Local DLLs and game-build restrictions remain device-local.
7. **Launch.** Perfect Sync launches only the validated private workspace. On Linux
   and macOS it uses the selected Proton, Wine, CrossOver, Whisky, or Bottles context.
   **Set up mods** performs the same publication without launch.

<img src="docs/assets/divider.svg" alt="" width="100%">

## Mod source classifications

| Tier | Meaning |
| --- | --- |
| **Trusted** | Metadata curated in Perfect Sync's trusted catalog. This is not a publisher signature or a verification of the downloaded native code. |
| **Community** | Catalog-listed metadata that is not in the curated trusted tier. |
| **Flagged / Unverified** | An unknown direct repository, asset, or lobby mod. Explicit confirmation is required; install only if you trust the source. |

## Install and run

Download the bundle for your host from
[Releases](https://github.com/artriy/Perfect-Sync/releases). The release page is also
where the in-app update notification sends you: v0.1.0 does not download or install
application updates automatically.

| Perfect Sync host | Package and first-run restriction |
| --- | --- |
| Windows 10/11 x64 | Windows installer; unsigned, so SmartScreen can require **More info → Run anyway**. |
| Linux x86_64 | Native Linux bundle; the Windows game still runs through a supported Proton, Wine, or Bottles setup. |
| macOS Apple Silicon | Native `aarch64` bundle; ad-hoc signed but not Apple-notarized, so Gatekeeper can block the download. |
| macOS Intel | Native `x64` bundle; ad-hoc signed but not Apple-notarized, so Gatekeeper can block the download. |

On macOS, drag the app to Applications. Ad-hoc signing checks bundle consistency but
does not establish a trusted developer identity. If you trust the download and
Gatekeeper quarantines it, you may remove that quarantine:

```sh
xattr -dr com.apple.quarantine "/Applications/Perfect Sync.app"
```

Running the Windows `app.exe` itself under Wine/Proton is not supported; use the native
Perfect Sync build for the host.

## Storage locations

Perfect Sync keeps small user state separate from large managed game data:

| Data | Windows location |
| --- | --- |
| Settings, profiles, and catalog overrides | `%APPDATA%\Perfect-Sync\` |
| A profile's owned DLLs and mutable BepInEx configuration | `%APPDATA%\Perfect-Sync\profiles\<profile-id>\BepInEx\` |
| Immutable game bases | `%LOCALAPPDATA%\Perfect-Sync\managed-games\bases\<instance-id-hash>\versions\<base-id>\game\` |
| Currently selected runnable profile | `%LOCALAPPDATA%\Perfect-Sync\managed-games\workspace\current\` |
| Downloaded package caches | `%APPDATA%\Perfect-Sync\cache\` |

Only one full runnable workspace is kept active. Profiles permanently store their own
mods and configuration, not another complete copy of Among Us. Downloaded Town of Us
and BepInEx packages are shared caches. The selected Steam, Epic, Microsoft Store, or
manual folder remains outside these paths and is never changed.

The first-run wizard and Settings can relocate the large managed data to an empty
custom folder. Perfect Sync copies and SHA-256-verifies the bases, active workspace,
and package caches before switching, then removes the old copy. A custom root uses
`<selected-folder>\managed-games\` and `<selected-folder>\cache\`; profiles and
settings remain under `%APPDATA%`.

## Build from source

See [BUILD.md](BUILD.md). The Windows NSIS quick path is:

```sh
pnpm install --frozen-lockfile
pnpm run build:exe
```

Linux and macOS use the target-specific `pnpm tauri build --target ...` commands in
the build guide.

Stack: Tauri 2, React 19, TypeScript, Vite, Tailwind v4, with a Rust core crate.

## Platform support

| Perfect Sync host | Windows Among Us runtime | Status |
| --- | --- | --- |
| Windows 10/11 x64 | Native Steam, Epic, Microsoft Store / Game Pass, or manual source | **Primary supported target**; sources are imported into private managed storage and never modified |
| Linux x86_64 / Steam Deck | Steam/Flatpak Steam + Proton, Wine, or Bottles | **Experimental**; the private workspace launches through the source's compatibility context |
| macOS Intel or Apple Silicon | CrossOver, Whisky, or Wine | **Experimental**; the private workspace launches through the selected bottle or prefix |
| Windows Perfect Sync executable under Wine/Proton | Any | **Unsupported** — use the native host build |
| Android, iOS, BSD, ChromeOS, Linux ARM64, Windows ARM64 | Any | **Unsupported** |

Release CI builds Windows x64, Linux x86_64, macOS Intel, and macOS Apple Silicon
packages and runs the locked workspace tests on every build host before bundling.
Its filesystem/runtime tests and bundle checks validate build portability, host-gated
behavior, command construction, and package structure only. They do **not** establish
that a package launches and modifies the game correctly on real hardware, under a
particular store client, or through every compatibility frontend.

## Security and ownership

Mods are native code loaded inside the game. Perfect Sync requires HTTPS. Within one
install operation it checks the downloaded asset against the size and any SHA-256
digest supplied by the release metadata fetched for that operation, and ZIP extraction
enforces strict entry, path, per-file, and expanded-size limits. These controls detect
corruption, truncated/replaced assets, path traversal, and some metadata/asset
time-of-check/time-of-use races. They are **integrity checks, not authenticity**:
metadata and downloads are not publisher-signed, and a compromised source can replace
both. Only the **Trusted** catalog metadata tier is curated. Unknown direct
repositories/assets and lobby mods require explicit confirmation; trust the source and
the person sharing a lobby code.

The Epic launch helper is a special case: Perfect Sync pins the HTTPS archive's size and
SHA-256, requires an exact one-file ZIP shape, and verifies the extracted executable's
size and SHA-256 before publishing or reusing it.

An optional GitHub token is stored in the host OS credential service (Windows
Credential Manager, macOS Keychain, or Linux Secret Service), sent only to allowlisted
GitHub HTTPS hosts, and never returned to the frontend. A legacy token found in
`settings.json` is migrated to the OS keyring and scrubbed from the settings file.

During synchronization, an ownership marker limits replacement/removal to
Perfect Sync-managed plugin DLLs. Unmanaged DLLs are detected recursively before
publication and must be imported into the selected profile, moved transactionally to
`BepInEx/.perfectsync-quarantine`, or explicitly deleted after confirmation. They are
never removed silently. Name collisions fail instead of overwriting user files.
Replacement of previously owned DLLs is deliberate when changing profiles or versions.

## Credits

Built on [BepInEx](https://github.com/BepInEx/BepInEx). Type set in
[Outfit](https://github.com/Outfitio/Outfit-Fonts) and
[JetBrains Mono](https://github.com/JetBrains/JetBrainsMono). Mods are created by their
respective authors and downloaded at runtime under their own licenses. Full third-party
notices are in [NOTICE](NOTICE).

## License

Released under the [MIT License](LICENSE).
