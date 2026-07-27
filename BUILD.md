# Building and releasing Perfect Sync

Development mode uses Vite hot reload. A packaged build embeds the frontend in a
native Tauri application; end users do not need Node.js or Rust.

## Build prerequisites

- **All hosts:** Rust 1.88.0 (the workspace MSRV and release toolchain), Node.js
  20.19+ or 22.12+, and pnpm 9.15.9.
- **Windows:** Microsoft C++ Build Tools and WebView2 (included with current
  Windows).
- **Linux:** `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, and
  `patchelf`; RPM packaging also needs `rpm`. The exact Ubuntu packages are in
  `.github/workflows/release.yml`.
- **macOS:** Xcode command-line tools.

Install the locked frontend dependencies:

```sh
pnpm install --frozen-lockfile
```

## Development

```sh
pnpm tauri dev    # native window with Vite hot reload
pnpm dev          # browser-only mock-data UI at http://localhost:1420
```

## Local packaged builds

The Windows-only quick path builds the frontend and Rust application, creates an
NSIS installer, and leaves the directly runnable executable beside it:

```powershell
pnpm run build:exe
# equivalent repository helper:
./scripts/build.ps1
```

| Local Windows output | Purpose |
| --- | --- |
| `target/release/app.exe` | Uninstalled local smoke-test executable; release automation does not upload this loose file |
| `target/release/bundle/nsis/Perfect Sync_<version>_x64-setup.exe` | NSIS installer |

For a package native to the current host, use Tauri's build command. The release
workflow makes target selection explicit:

```sh
# Linux x64
pnpm tauri build --target x86_64-unknown-linux-gnu

# macOS Intel
pnpm tauri build --target x86_64-apple-darwin

# macOS Apple Silicon
pnpm tauri build --target aarch64-apple-darwin
```

Targeted outputs live under `target/<target>/release/bundle/`. Production binaries
use Tauri's `custom-protocol` feature and embedded `dist` assets; they do not require
`localhost:1420`. Use a Tauri build so `beforeBuildCommand` regenerates the frontend.
Plain `cargo build --release` can embed an existing, stale `dist` directory and is not
the packaging command.

## Release packages and support

| Release job | Uploaded packages | Game launch expectation |
| --- | --- | --- |
| Windows x64 | MSI and NSIS installers | Primary target; launches the Windows game natively |
| Linux x64 | DEB, RPM, and AppImage | Experimental; native Perfect Sync app launches the Windows game through Proton, Wine, or Bottles |
| macOS x64 | DMG | Experimental; native Perfect Sync app launches the Windows game through CrossOver, Whisky, or Wine |
| macOS arm64 | DMG | Experimental; native Perfect Sync app launches the Windows game through CrossOver, Whisky, or Wine |

These packages compiling, passing tests, and having valid container/file structure in
CI is **build validation**, not proof of correct behavior on real hardware or with a
particular game version, store client, prefix, bottle, or compatibility runtime.
Linux/macOS game integration still requires representative real-host end-to-end
validation. A protected native Microsoft Store / Game Pass copy under `WindowsApps`
must be copied to a normal writable folder before Perfect Sync can manage that copy.

## Release workflow

`.github/workflows/release.yml` accepts either:

- a pushed existing tag matching `v*`; or
- a manual dispatch whose required `tag` input names an existing tag.

Validation fetches the tag with full history, requires it to be exactly
`v${package.json.version}`, and verifies that the Tauri version and every Cargo
workspace package version match. Cargo metadata, Clippy, and workspace tests all use
the checked-in lockfile. The validation job resolves the tag's peeled commit once and
all four build jobs check out that immutable SHA. It also performs a frozen frontend
dependency install, production npm and RustSec advisory audits, frontend type-check and
production build, Rust formatting and Clippy checks, and the full all-features workspace
test suite using Rust 1.88.0.

Only after validation succeeds does the workflow create a **draft prerelease**. An
existing release may be reused only while it is still a draft; all of its old assets
are deleted before any build starts. Four build jobs package Windows x64, Linux x64,
macOS x64, and macOS arm64. Every build host runs the locked all-features workspace
tests before Tauri bundles with locked Cargo resolution. The jobs validate the expected
installers/bundles (including package/container shape, nonempty content, hashes, and the
macOS bundle's ad-hoc signature) before uploading only those packages to the draft.

The publish job runs only after every matrix build succeeds. It requires exactly seven
assets—one MSI, one NSIS setup EXE, one DEB, one RPM, one AppImage, and two DMGs—then
force-fetches the tag and confirms it still peels to the validated commit immediately
before making the draft visible as a GitHub **prerelease**. Push and manual runs for the
same effective tag share one concurrency group. A validation, build, asset-shape, or
stale/moved-tag failure leaves the release non-public.

## Signing, runtime downloads, and updates

- **Windows:** packages are unsigned. SmartScreen may show **Windows protected your
  PC**; after independently confirming the download, use **More info → Run anyway**.
  Warning-free distribution requires Authenticode signing.
- **macOS:** `bundle.macOS.signingIdentity` is `-`, so packages are ad-hoc signed.
  CI verifies bundle consistency, but there is no Developer ID identity or Apple
  notarization. Gatekeeper can still block the download. After independently confirming
  it, a user may need:

  ```sh
  xattr -dr com.apple.quarantine "/Applications/Perfect Sync.app"
  ```

- **BepInEx:** setup downloads a bounded loader package at runtime, caches it per build
  and game architecture under the application-data directory, and retains a previously
  verified working cache if an online refresh fails.
- **Application updates:** v0.1.0 only checks the canonical GitHub Releases page and
  notifies the user. The user opens that page and manually downloads/installs the new
  package. There is no signed automatic updater.
