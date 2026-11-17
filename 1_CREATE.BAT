@echo off
setlocal

echo Creating directory structure for BeeBeeSix/OCS_Plugin_Steal...
if not exist "src" mkdir "src"
if not exist ".github\workflows" mkdir ".github\workflows"

echo Writing Cargo.toml...
(
echo [package]
echo name = "ocs-plugin-Steal"
echo version = "0.1.0"
echo edition = "2021"
echo description = "Block Attribute Management for Open CAD Studio."
echo license = "GPL-3.0-only"
echo.
echo [workspace]
echo.
echo [lib]
echo crate-type = ["cdylib", "rlib"]
echo.
echo [dependencies]
echo ocs_plugin_api = { git = "https://github.com", tag = "v2026.46", features = ["host"] }
echo serde_json = "1"
echo.
echo # Uncomment below if Steal opens a separate eframe GUI window:
echo # eframe = { version = "0.37", default-features = false, features = ["default_fonts", "glow", "x11", "wayland"] }
echo # winit = "0.30"
echo.
echo # [target.'cfg(windows^)'.dependencies]
echo # rfd = { version = "0.15", default-features = false }
) > Cargo.toml

echo Writing plugin.toml...
(
echo [plugin]
echo id = "com.beebeesix.ocs-plugin-Steal"
echo name = "Steal (Block Attribute Manager)"
echo version = "0.1.0"
echo description = "Block Attribute Management"
echo author = "BeeBeeSix"
echo license = "GPL-3.0-only"
echo repository = "https://github.com"
echo.
echo [opencad]
echo # Must match your Open CAD Studio version (v2026.46 = API 23^)
echo api_version = 23
echo ribbon_order = 120
echo command_prefixes = ["STEAL"]
echo xdata_apps = []
) > plugin.toml

echo Writing .github\workflows\release.yml...
(
echo name: Release Plugin
echo.
echo on:
echo   push:
echo     tags:
echo       - 'v*'
echo.
echo permissions:
echo   contents: write
echo.
echo jobs:
echo   build:
echo     name: Build (${{ matrix.os }}^)
echo     runs-on: ${{ matrix.os }}
echo     strategy:
echo       fail-fast: false
echo       matrix:
echo         include:
echo           - os: ubuntu-latest
echo             lib: libocs_plugin_Steal.so
echo             asset: com.beebeesix.ocs-plugin-Steal-linux-x86_64.so
echo           - os: windows-latest
echo             lib: ocs_plugin_Steal.dll
echo             asset: com.beebeesix.ocs-plugin-Steal-windows-x86_64.dll
echo           - os: macos-latest
echo             lib: libocs_plugin_Steal.dylib
echo             asset: com.beebeesix.ocs-plugin-Steal-macos-aarch64.dylib
echo.
echo     steps:
echo       - name: Checkout repository
echo         uses: actions/checkout@v4
echo.
echo       - name: Install Linux dependencies
echo         if: runner.os == 'Linux'
echo         run: ^|
echo           sudo apt-get update
echo           sudo apt-get install -y \
echo             libgl1-mesa-dev \
echo             libx11-dev \
echo             libxcursor-dev \
echo             libxi-dev \
echo             libxrandr-dev \
echo             libxkbcommon-dev \
echo             libwayland-dev \
echo             libfontconfig1-dev \
echo             libfreetype6-dev
echo.
echo       - name: Install Rust toolchain
echo         uses: dtolnay/rust-toolchain@stable
echo.
echo       - name: Build release binary
echo         run: cargo build --release
echo.
echo       - name: Stage release asset
echo         shell: bash
echo         run: ^|
echo           mkdir -p dist
echo           cp "target/release/${{ matrix.lib }}" "dist/${{ matrix.asset }}"
echo.
echo       - name: Upload artifact
echo         uses: actions/upload-artifact@v4
echo         with:
echo           name: ${{ matrix.asset }}
echo           path: dist/${{ matrix.asset }}
echo.
echo   release:
echo     name: Create GitHub Release
echo     needs: build
echo     runs-on: ubuntu-latest
echo     steps:
echo       - name: Checkout repository
echo         uses: actions/checkout@v4
echo.
echo       - name: Download all artifacts
echo         uses: actions/download-artifact@v4
echo         with:
echo           path: artifacts
echo           merge-multiple: true
echo.
echo       - name: Publish GitHub Release
echo         uses: softprops/action-gh-release@v2
echo         with:
echo           generate_release_notes: true
echo           files: ^|
echo             plugin.toml
echo             artifacts/*
) > .github\workflows\release.yml

echo Writing src\lib.rs...
(
echo use ocs_plugin_api::{
echo     ocs_plugin, CommandDef, CommandResult, HostContext, Plugin, PluginManifest, RibbonItem,
echo };
echo.
echo #[derive(Default^)]
echo pub struct StealPlugin;
echo.
echo impl Plugin for StealPlugin {
echo     fn manifest(&self^) -^> PluginManifest {
echo         PluginManifest {
echo             id: "com.beebeesix.ocs-plugin-Steal".into(^),
echo             name: "Steal (Block Attribute Manager^)".into(^),
echo             version: env!("CARGO_PKG_VERSION"^).into(^),
echo             description: "Block Attribute Management".into(^),
echo             author: "BeeBeeSix".into(^),
echo         }
echo     }
echo.
echo     fn commands(&self^) -^> Vec^<CommandDef^> {
echo         vec![CommandDef {
echo             name: "Steal".into(^),
echo             aliases: vec!["BATTMAN".into(^)],
echo             description: "Open the Block Attribute Manager".into(^),
echo         }]
echo     }
echo.
echo     fn ribbon_items(&self^) -^> Vec^<RibbonItem^> {
echo         vec![RibbonItem {
echo             tab: "Plugins".into(^),
echo             panel: "Block Attributes".into(^),
echo             label: "Steal".into(^),
echo             command: "Steal".into(^),
echo             tooltip: "Manage block attributes in the active drawing".into(^),
echo         }]
echo     }
echo.
echo     fn execute_command(&mut self, command: &str, host: &mut HostContext^) -^> CommandResult {
echo         match command {
echo             "Steal" =^> {
echo                 host.print("Steal: Block Attribute Manager initialized."^);
echo                 CommandResult::Completed
echo             }
echo             _ =^> CommandResult::UnknownCommand,
echo         }
echo     }
echo }
echo.
echo ocs_plugin!(StealPlugin^);
) > src\lib.rs

echo.
echo Done! Generated files for BeeBeeSix/OCS_Plugin_Steal:
echo   - Cargo.toml              (lib: ocs_plugin_Steal^)
echo   - plugin.toml             (id:  com.beebeesix.ocs-plugin-Steal^)
echo   - .github\workflows\release.yml
echo   - src\lib.rs
echo.
echo Next steps:
echo   1. Delete setup_Steal.bat (or keep it out of git^)
echo   2. Run: cargo check
echo   3. git add . ^&^& git commit -m "Initial Steal plugin scaffold" ^&^& git push
echo   4. git tag v0.1.0 ^&^& git push origin v0.1.0
pause
