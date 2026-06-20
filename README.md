# Perfect-Sync

A desktop mod manager for Among Us that installs BepInEx, manages mod profiles, and syncs a lobby's mod set between players via shareable codes.

> **Disclaimer**
>
> Perfect-Sync is an unofficial, fan-made tool. It is not affiliated with, endorsed by, or sponsored by Innersloth LLC. Among Us is a trademark of Innersloth LLC.
>
> Use modded clients only in private/modded lobbies. Do not use mods to disrupt public or vanilla games. See the [Among Us mod policy](https://www.innersloth.com/among-us-mod-policy/).

## Install and run

Windows is the supported platform.

1. Download the installer (`Perfect-Sync_<version>_x64-setup.exe`) or the portable `app.exe`.
2. The build is unsigned, so Windows SmartScreen shows a warning on first run. Click **More info**, then **Run anyway**.

Linux (via Steam Proton) and macOS (via CrossOver/Wine) support exists in the code but is **experimental** and has not yet been built or tested on those platforms. To try them, build from source.

## Build from source

See [BUILD.md](BUILD.md). In short:

```
pnpm install
pnpm run build:exe
```

## Mod trust levels

The app labels every mod with one of three trust levels:

- **Trusted**: curated, known-good mods from the catalog.
- **Community dev**: community-made mods that are listed in the catalog but are not first-party curated.
- **Flagged / Unverified**: anything not in the catalog. Install at your own risk, and only apply lobby codes from people you trust.

## Security note

Applying a lobby code installs the mod DLLs that the code lists. Only Trusted and Community mods are vetted; Flagged mods are not. Only apply codes from people you trust.

## License

Released under the [MIT License](LICENSE).

Third-party components are credited in [NOTICE](NOTICE).
