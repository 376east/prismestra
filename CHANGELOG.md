# Changelog

For downloads, see [GitHub Releases](https://github.com/malyarq/prismestra/releases). For physical device coverage, see the [compatibility table](docs/compatibility.md).

## Unreleased

These changes are in `main` and are pending a Windows release.

### Scenes and editor

- Open and save portable scene files. Preview a draft LED map for a selected output before applying it, including while the output is offline. [Editor changes](https://github.com/malyarq/prismestra/pull/39)
- Insert and reorder palette colors, and start from the built-in Plum Dawn preset. [Palette editing](https://github.com/malyarq/prismestra/pull/39) · [Plum Dawn](https://github.com/malyarq/prismestra/pull/51)
- Keep device selection and unfinished input tied to the device ID across discovery updates. Show LCD and screen pages when their outputs are configured. Move scheduling and diagnostics into Settings. [Navigation and device status](https://github.com/malyarq/prismestra/pull/57)
- Open a portable ZIP profile in preview without sending its colors to existing local outputs. Hardware activation requires an explicit Apply. [Portable import fix](https://github.com/malyarq/prismestra/pull/62)

### Devices and recovery

- Keep valid OpenRGB controllers available when another controller has invalid geometry. Refuse Direct mode if the physical identity is ambiguous. Retry one interrupted inventory read during first run. [Device support](https://github.com/malyarq/prismestra/pull/40) · [Startup and recovery](https://github.com/malyarq/prismestra/pull/63)
- Preserve all 153 Surplife E1 color positions, send a fresh full frame after interrupted transmission, and show the failed write or mismatched map length in diagnostics. [Device support](https://github.com/malyarq/prismestra/pull/40) · [Frame recovery](https://github.com/malyarq/prismestra/pull/38) · [Repair guidance](https://github.com/malyarq/prismestra/pull/57)
- Discard stale LCD arrival notifications and show the actual DXGI color space when screen capture falls back. [Device support](https://github.com/malyarq/prismestra/pull/40)
- Keep unaffected RGB and LCD outputs running during supported RGB configuration changes. Preserve effect timing while applying a new output plan. [Runtime recovery](https://github.com/malyarq/prismestra/pull/63)
- After Home Assistant reconnects, send the latest scene color. Show authorization errors as requiring user action. [Runtime recovery](https://github.com/malyarq/prismestra/pull/63)

### Windows desktop

- Open the editor from the tray by keyboard. Respect Windows menu alignment and ignore tray activation after exit begins. [Tray access](https://github.com/malyarq/prismestra/pull/43) · [Exit fix](https://github.com/malyarq/prismestra/pull/60)
- Announce changes to the selected device status and restore keyboard focus to the button that opened a settings page. [Device status](https://github.com/malyarq/prismestra/pull/57) · [Settings focus](https://github.com/malyarq/prismestra/pull/59)

[All changes since Preview 6](https://github.com/malyarq/prismestra/compare/v0.1.0-preview.6...main)

## Preview 6 (0.1.0.6) · 2026-11-19

- At sign-in, restore the saved scene and outputs. With a fresh profile, start discovery and build an initial map for supported devices when they are available.
- Run the bundled OpenRGB backend in the background. Recover registered outputs after resume and retain healthy outputs when another output fails.
- Keep an unsaved scene draft when switching scenes. Restore the editor in the existing session on repeat launch and recover it after a WebView2 browser crash.
- Give desktop navigation items distinct icons and use **Prismestra** as the window title.
- Provide one Windows x64 Setup with sign-in startup and uninstall. The published Setup passed installation, an upgrade from Preview 5, uninstall, and standard-account startup on isolated Windows. Repair passed with a separate test package. No physical devices were present on the runner.

Known issue. Opening a portable ZIP while outputs are connected can send colors before Apply. This was reproduced from the Preview 6 source tag and fixed in the unreleased changes above.

[Download Preview 6](https://github.com/malyarq/prismestra/releases/tag/v0.1.0-preview.6) · [Windows verification](https://github.com/malyarq/prismestra/actions/runs/44595285813) · [All changes since Preview 5](https://github.com/malyarq/prismestra/compare/v0.1.0-preview.5...v0.1.0-preview.6)
