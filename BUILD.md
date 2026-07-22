# Building Perfect-Sync into a real app

Development mode runs the UI with hot reload. A release build produces a native
Tauri desktop app; end users do not need Node or Rust.

## One-time prerequisites (build machine only)

- **All hosts:** stable Rust, Node 20+, pnpm, then `pnpm install`.
- **Windows:** Microsoft C++ Build Tools and WebView2 (included with current Windows).
- **Linux:** WebKitGTK 4.1 and the other packages installed by
  `.github/workflows/release.yml`.
- **macOS:** Xcode command-line tools.

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

Pushing a tag like `v0.1.0` or manually dispatching `.github/workflows/release.yml`
builds Windows x64, Linux x86_64, macOS Apple Silicon, and macOS Intel artifacts.
Every matrix job first runs the TypeScript check and default Rust workspace test
suite, including the host-runtime filesystem smoke test, before Tauri packages and
uploads the release artifacts.

The local `pnpm run build:exe` command remains the Windows-only quick path.

## Code signing & updates

- **Windows:** builds are unsigned, so SmartScreen prompts on first run. Trusted
  Authenticode signing requires a certificate.
- **macOS:** `tauri.conf.json` sets `bundle.macOS.signingIdentity` to `-`, producing
  a free ad-hoc signature on both architectures. This verifies bundle consistency,
  but it is not a Developer ID signature and cannot be notarized. Downloaded builds
  can still require:

  ```sh
  xattr -dr com.apple.quarantine "/Applications/Perfect-Sync.app"
  ```

  A paid Developer ID certificate plus Apple notarization is still required for a
  warning-free public install.
- The app's update notifier checks GitHub Releases. A signed automatic updater is
  future work.
