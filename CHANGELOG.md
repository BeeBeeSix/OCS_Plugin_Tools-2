# Changelog

All notable changes to Tools are recorded here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/) —
`0.x.y` means an initial version, still being tested.

This is `CHANGELOG.md`  which the GitHub
release notes are taken from). `scripts/release.ps1` moves the entries under
**[Unreleased]** into the new version when a release is made.

## [Unreleased]

### Added
- Shortcuts from the lisp: `01`-`09` view presets, `ZE` zoom extents, `UV` restore the UCS saved by DVT, `UW` UCS World + view twist 0.
- **DVT** (twisted view), ported from DVT.LSP: draws the pick line on `defpoints`, sets UCS World then a UCS along the line, turns the view to plan, resets snap angle and current layer, and saves the UCS as `CURRENT_VIEW`.

### Changed
- Plugin renamed from Steal to **Tools** (`OCS_Plugin_Tools`): Steal is now one tool inside it. Plugin id is now `com.beebeesix.ocs-plugin-tools`, so remove the old `com.beebeesix.ocs-plugin-steal` plugin folder before installing.
- The log file is now `%TEMP%\ocs-tools.log`.

## [0.1.1] - 2026-11-17

### Added
LAYOUTS

### Changed


## [0.1.0] - 2026-11-17

### Added

### Fixed

