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
