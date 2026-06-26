# Changelog

## v0.1.0 (experimental)

This is an early, experimental release. Nothing here is official or stable yet:
expect rough edges, breaking changes, and behavior that may differ across Among Us
versions and stores. Use it at your own risk, and please report what breaks.

### What's in it
- Detect Among Us across Steam, Epic, itch, and a manually selected folder, with
  Wine/Proton/CrossOver support off Windows.
- One-click BepInEx setup that auto-refreshes to the latest build, and keeps the
  working loader when offline.
- Mod catalog plus add-any-GitHub-repo, named profiles, lobby/share codes, and
  trust badges.
- One-click launch that syncs the active profile into the game folder.

### Known limitations
- Builds are unsigned, so Windows SmartScreen and macOS Gatekeeper warn on first run.
- Microsoft Store / Game Pass copies live in the protected WindowsApps folder and
  must be copied to a normal folder first.
- Downloaded mods are not integrity-checked yet.
