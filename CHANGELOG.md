# Changelog

## v0.1.0 (experimental)

This is an early, experimental release. Nothing here is official or stable yet:
expect rough edges, breaking changes, and behavior that may differ across Among Us
versions and stores. Use it at your own risk, and please report what breaks.

### What's in it
- Detect Among Us across native and Flatpak Steam, Epic, itch, manually selected
  folders, Wine, Proton, CrossOver, Whisky, and Bottles.
- Persist the selected host runtime and classify bottle paths before generic Steam
  paths, preventing CrossOver/Wine installs from being mistaken for Proton.
- Synchronize and validate the selected game folder without requiring Wine/Proton
  to be installed or running; runtime-prefix guidance is reported separately.
- Install and verify the required `winhttp` override, with actionable first-launch
  errors when Steam has not created a Proton prefix yet.
- Launch through native Steam, Flatpak Steam, Wine, CrossOver, Whisky, and Bottles.
- One-click BepInEx setup that auto-refreshes to the latest build and keeps the
  working loader when offline.
- Mod catalog plus add-any-GitHub-repo, named profiles, lobby/share codes, trust
  badges, and profile synchronization before launch.

### Known limitations
- Windows builds are unsigned, so SmartScreen warns on first run.
- macOS artifacts are free ad-hoc signed but not notarized; Gatekeeper may still
  require removing the download quarantine attribute.
- Linux/macOS game launch integrations remain experimental while their CI builds
  and host-runtime filesystem tests gain broader real-machine coverage.
- Microsoft Store / Game Pass copies live in the protected WindowsApps folder and
  must be copied to a normal folder first.
- Downloaded mods are not integrity-checked yet.
