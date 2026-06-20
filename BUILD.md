# Building Perfect-Sync into a real app

Two ways to run it. Dev mode is for hacking; the build produces a normal Windows
app you can install and double-click — no Node/Rust needed by the end user.

## One-time prerequisites (build machine only)

- **Rust** (stable): https://rustup.rs
- **Node + pnpm**: `npm i -g pnpm`
- **Microsoft C++ Build Tools** (MSVC) and **WebView2** (preinstalled on Win10/11)
- Install JS deps once: `pnpm install`

End users need **none** of this. WebView2 ships with Windows; everything else is
bundled into the app.

## Produce a testable exe (no dev server)

```powershell
pnpm run build:exe        # = tauri build --bundles nsis
# or:  ./scripts/build.ps1
```

This compiles the Rust core + UI in release mode and bundles an installer.

### Outputs (under `target/release/`)

| File | What it is |
|---|---|
| `target/release/app.exe` | **Portable** build (~12 MB). Double-click to run, no install. Fastest way to test. |
| `target/release/bundle/nsis/Perfect-Sync_0.1.0_x64-setup.exe` | **Installer** (~3 MB). What you'd share with people. |

So to test without `tauri dev`: build once, then run `target/release/app.exe`.

## Dev mode (hot reload, for development)

```powershell
pnpm tauri dev            # native window + Vite hot reload
pnpm dev                  # browser-only UI demo (mock data) at http://localhost:1420
```

## Notes

- The build is **unsigned**, so Windows SmartScreen shows a "protected your PC"
  prompt on first run: click **More info -> Run anyway**. Code signing (a paid
  cert) removes this; intentionally skipped for now.
- First launch of a modded profile downloads the BepInEx loader pack (~31 MB,
  from GitHub) once, then caches it under `%APPDATA%/Perfect-Sync`.
- The mod catalog is fetched from GitHub at startup and cached, with a bundled
  copy as offline fallback.

## Cross-platform builds (CI)

Pushing a tag like `v0.1.0` triggers `.github/workflows/release.yml`, which builds
Windows, Linux, and macOS (Apple Silicon + Intel) via `tauri-apps/tauri-action` and
creates a **draft prerelease** on GitHub with the bundled artifacts attached. You can
also run it manually from the Actions tab (workflow_dispatch). The local
`pnpm run build:exe` remains the Windows-only path for quick testing.

## Code signing & updates

- **Windows** builds are currently **unsigned**, so SmartScreen shows a prompt on
  first run (click **Run anyway**). To sign, add an Authenticode certificate and wire
  it into CI through `tauri-action`/environment variables.
- **macOS** artifacts need an Apple Developer ID plus notarization (set the `APPLE_*`
  secrets) before they are distributable; until then they are unsigned and Gatekeeper
  will block them.
- The app already ships an in-app "update available" notifier that checks GitHub
  Releases. A full signed auto-installer (`tauri-plugin-updater` with a signing
  keypair) is future work.
