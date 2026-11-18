# Tools - an Open CAD Studio plugin

A collection of tools for Open CAD Studio.

| Command | Tool |
|---|---|
| `STEAL` | Import layers, linetypes, text styles, dimension styles and blocks from another drawing |
| `01`-`09` | View presets: 01 SW iso, 02 front, 03 SE iso, 04 left, 05 top, 06 right, 07 NW iso, 08 back, 09 NE iso |
| `ZE` | Zoom extents |
| `UW` | UCS World, then view twist 0 (`DVIEW TWIST 0`) |
| `UV` | Restore the UCS saved by DVT (`CURRENT_VIEW`) |
| `DVT` | Twisted view: pick two points for the desired horizontal; UCS and plan view turn to that line |

## Steal

Import **layers, linetypes, text styles, dimension styles and blocks** from another
drawing (DXF or DWG) into the drawing you have open. 

Idea Based on https://www.lee-mac.com/steal.html

## Open CAD Studio version: Release tag `v2026.46` (plugin API 23) 

<img width="900" height="500" alt="image" src="https://github.com/user-attachments/assets/cf2e4c34-7a85-46a3-9101-0578711376a3" />

* **Type list (left)** - pick Layers, Linetypes, Text styles, Dimension styles or Blocks.
  The number in (round brackets) is how many the source has; **[n]** is how many you have
  ticked. Ticks are kept while you switch type.
* **Filter** works as you type. Comma separated; plain text matches anywhere in the name,
  an entry with `*` or `?` is a wildcard for the whole name (`WALL*`, `DOOR?`).
* **Select all / Select none / Invert** act on the items the filter currently shows
  (everything, if the filter is empty). Items outside the filter are never touched.
* Items marked *(exists)* are already in your drawing: skipped unless *Overwrite* is ticked
  (never for blocks). **Hide items already in the drawing** removes them from the list.
* **Import** runs in the background and is **one Undo step**. The window lists what was
  created, overwritten, skipped or failed.
* Closing the window only hides it; `STEAL` brings it back with your source still loaded.

Dependencies come along when "Also import what ticked items need" is on: a layer's
linetype, the layers/linetypes/text styles a block's contents use, nested blocks, a
dimension style's text style.

## Troubleshooting

Every step is logged to `%TEMP%\ocs-tools.log`. A healthy run: `--- STEAL started`,
`window shown`, then `reading ...`, `import started`, `import finished`.

* The window does not appear: the log says why (`the Steal window could not start`).
* *no drawing attached*: run `STEAL` from the drawing tab you want to import into.
* The window is not available on macOS yet.

## Limits

* Complex linetypes (with embedded shapes or text) cannot be created through the host API
  and are reported as failed; simple dash/dot/gap linetypes work.
* A block is rebuilt from its member entities with its origin at 0,0,0 (a non-zero source
  base point is not carried); members the host refuses are counted in the report.
  Anonymous blocks, xrefs and layout blocks are not offered.
* Dimension styles: fields the host refuses are dropped one at a time and listed in the report.
* Per the Open CAD Studio notes, a DXF *save* drops a text style's backward/upside-down
  flags and a dimension style's text-style name; DWG keeps both.

## Development

`STEAL_TEST_SRC=path/to/source.dxf cargo test -- --nocapture` runs the engine against a
mock host and drives the real window headlessly (synthetic clicks and typing).

## Build and install

Needs Rust (https://rustup.rs) and, on Windows, the Visual Studio "Desktop development
with C++" build tools. The first build is long (the UI toolkit is compiled too).

    Windows (PowerShell):   .\install.ps1        (close Open CAD Studio first)
    Linux:                  ./install.sh

That builds the plugin and copies `plugin.toml` plus the library to
`%APPDATA%\OpenCADStudio\plugins\com.beebeesix.ocs-plugin-tools\`. Start Open CAD Studio and run `STEAL`.

## It must match your Open CAD Studio version

`Cargo.toml` pins `ocs_plugin_api` to the release tag `v2026.46` (plugin API 23) and
`plugin.toml` says `api_version = 23`. For another release change **both** (the API number
is `API_VERSION` in `crates/ocs_plugin_api/src/manifest.rs` at that tag).
