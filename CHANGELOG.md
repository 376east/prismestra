# Changelog

This file records notable changes to Prismestra. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Public release numbers use [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

[Download for Windows](https://github.com/376east/prismestra/releases/latest) · [Device compatibility](docs/compatibility.md)

## Unreleased

## 0.2.0 - 2026-11-26

### Added

- **Alice lamps.** Sign in with Yandex ID under Settings → Lamp controls, select a lamp, and optionally save its brightness with a scene. [Scene lamp binding](https://github.com/376east/prismestra/pull/221)
- **ARGB channel preview.** Preview the LED layout for a motherboard channel before configuring its first LED count. [ARGB preview](https://github.com/376east/prismestra/pull/233)
- **English and Russian.** The app starts in English by default. Choose Russian in Settings to use it on future launches. [Language settings](https://github.com/376east/prismestra/pull/240)

### Changed

- **Scene editor.** Apply scene changes the active lighting. Save to library stores a version for later. Each action has its own confirmation. [Scene actions](https://github.com/376east/prismestra/pull/240)
- **Device status.** After a scene is applied, Devices shows delivery separately for each output. A failed write is visible even if the scene itself was accepted. [Output status](https://github.com/376east/prismestra/pull/278)
- **Connection settings.** Save pending connection edits before applying a scene. The editor points you to the unsaved settings. [Connection edits](https://github.com/376east/prismestra/pull/284)

### Fixed

- **Startup profile.** Saving the active installed profile updates the copy loaded at the next Windows sign-in. [Profile save](https://github.com/376east/prismestra/pull/255)
- **Tray controls.** After Stop outputs, turning lighting on again resumes the last applied scene. [Resume](https://github.com/376east/prismestra/pull/286)
- **Monitor lighting.** When screen capture stops, the output uses the scene’s edge colors. It returns to screen colors when capture resumes. [Screen fallback](https://github.com/376east/prismestra/pull/281)
- **LCD.** Temperature cards keep their saved position. Static images skip duplicate frame writes, and the LCD can resume when the same USB interface returns. [Card placement](https://github.com/376east/prismestra/pull/215) · [LCD recovery](https://github.com/376east/prismestra/pull/258)
- **Surplife E1.** Turning the strip off works with the default Bluetooth packet size. [BLE lights off](https://github.com/376east/prismestra/pull/257)
- **Alice lamps.** If the response to a command is lost, the app keeps the last observed lamp state and reports that the command outcome is unknown. [Command status](https://github.com/376east/prismestra/pull/279)

### Upgrading from a Preview

Run the current Windows Setup over the existing installation to switch to the stable channel. Setup keeps saved profiles. [Installation guide](docs/windows-installer.md)

## Preview 10 (0.1.0.10) · 2026-11-23

### Discovery, scenes and outputs

- Run another device search after the first scan finds nothing. If automatic setup cannot identify an OpenRGB controller, choose and configure it manually. [Empty scan](https://github.com/376east/prismestra/pull/191) · [Controller identification](https://github.com/376east/prismestra/pull/193)
- See when Bluetooth is unavailable, retry the selected LCD connection, and get guidance when a saved LCD connection is missing. [Bluetooth](https://github.com/376east/prismestra/pull/187) · [LCD retry](https://github.com/376east/prismestra/pull/166) · [Missing LCD](https://github.com/376east/prismestra/pull/185)
- Search the scene library by title or effect. Creating a scene or copying a preset opens the editor with the effect picker focused. Preview audio-reactive scenes with a test signal. [Search](https://github.com/376east/prismestra/pull/192) · [Scene editor](https://github.com/376east/prismestra/pull/181) · [Audio preview](https://github.com/376east/prismestra/pull/174)
- Use the active scene's edge colors for monitor lighting. Set up an LCD with a temperature panel alone and pick a configured temperature source from a list. [Monitor lighting](https://github.com/376east/prismestra/pull/164) · [Temperature panel](https://github.com/376east/prismestra/pull/179) · [Source picker](https://github.com/376east/prismestra/pull/194)
- Launch the portable app and configure its managed OpenRGB connection from Prismestra. The binary portable ZIP drops its legacy CMD and PowerShell scripts. [Direct launch](https://github.com/376east/prismestra/pull/165) · [Connections](https://github.com/376east/prismestra/pull/175) · [ZIP packaging](https://github.com/376east/prismestra/pull/189)
- Keep network lighting failures visible in device status when switching an output off. The status shows the output as off after the new muted frame is sent successfully. [Network output status](https://github.com/376east/prismestra/pull/186)

### Scene control and troubleshooting

- Copy a summary of the selected output's status for troubleshooting. A device-search click just before the automatic pass starts now starts a manual search. [Status summary](https://github.com/376east/prismestra/pull/195) · [Early search](https://github.com/376east/prismestra/pull/196)
- Place and move RGB devices on the scene canvas with the mouse or keyboard. Select overlapping devices individually and keep drag previews within the canvas. [Placement](https://github.com/376east/prismestra/pull/197) · [Overlap](https://github.com/376east/prismestra/pull/198) · [Canvas bounds](https://github.com/376east/prismestra/pull/199)
- Try Quiet Shore (Тихий берег), a built-in gradient-wave scene. [Quiet Shore](https://github.com/376east/prismestra/pull/200)

- Apply an ordinary saved scene from the tray after closing the editor without pending changes. [Tray scenes](https://github.com/376east/prismestra/pull/202)

## Preview 8 (0.1.0.8) · 2026-11-22

- Start a lighting scene from the library. Preview and edit remain separate actions. Failed activation links to device status.
- New profiles start with the Peach Surf effect. Existing profiles keep their saved layout and scene.
- Reopen the editor from a session started at sign-in. Recover the managed OpenRGB backend and show invalid motherboard zone sizes in device status.
- Reconnect a saved Surplife E1 strip when the Bluetooth radio appears later.
- The published Windows Setup passed offline installation, upgrade from Preview 7, uninstall, and standard-account startup in CI.

[Windows verification](https://github.com/376east/prismestra/actions/runs/45023869783)

## Preview 7 (0.1.0.7) · 2026-11-21

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

## Preview 6 (0.1.0.6) · 2026-11-19

- At sign-in, restore the saved scene and outputs. With a fresh profile, start discovery and build an initial map for supported devices when they are available.
- Run the bundled OpenRGB backend in the background. Recover registered outputs after resume and retain healthy outputs when another output fails.
- Keep an unsaved scene draft when switching scenes. Restore the editor in the existing session on repeat launch and recover it after a WebView2 browser crash.
- Give desktop navigation items distinct icons and use **Prismestra** as the window title.
- Provide one Windows x64 Setup with sign-in startup and uninstall. The published Setup passed installation, an upgrade from Preview 5, uninstall, and standard-account startup on isolated Windows. Repair passed with a separate test package. No physical devices were present on the runner.

Known issue. Opening a portable ZIP while outputs are connected can send colors before Apply. This was reproduced from the Preview 6 source tag and fixed in Preview 7.

[Windows verification](https://github.com/malyarq/prismestra/actions/runs/44595285813)
