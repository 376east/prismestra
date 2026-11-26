# Changelog

For downloads, see [GitHub Releases](https://github.com/376east/prismestra/releases). For physical device coverage, see the [compatibility table](docs/compatibility.md).

## 0.2.0

### Startup and recovery

- Restore the saved lighting profile at sign-in. Editing the installed profile keeps that same copy available at the next launch. [Profile save](https://github.com/376east/prismestra/pull/255)
- Resume the applied scene after Stop from the tray. The installed OpenRGB helper and resident use an authenticated local connection and coordinate recovery so a restart can resume the latest lighting state. [Resume](https://github.com/376east/prismestra/pull/286) · [Local backend control](https://github.com/376east/prismestra/pull/266)
- If screen capture stops providing fresh frames, monitor lighting falls back to the base scene. The app reports scene acceptance and delivery per output, so a saved scene can still show which device needs attention. [Screen fallback](https://github.com/376east/prismestra/pull/281) · [Output status](https://github.com/376east/prismestra/pull/278)

### Editor and devices

- English is the default, with Russian available in Settings. **Apply scene** updates the active scene. **Save to library** stores a version for later. [Editor and language](https://github.com/376east/prismestra/pull/240)
- Preview an ARGB channel before setting its first LED count. Save a guarded channel size for the selected controller. [ARGB preview](https://github.com/376east/prismestra/pull/233) · [Channel save](https://github.com/376east/prismestra/pull/209)
- Keep the saved LCD temperature card where you placed it. Static LCD images skip duplicate frame writes, and the LCD worker can recover when the matching HID interface returns. [Card placement](https://github.com/376east/prismestra/pull/215) · [LCD recovery](https://github.com/376east/prismestra/pull/258)
- Link an Alice lamp through Yandex ID and optionally set its brightness from a saved scene. If Yandex cannot confirm a command, Prismestra reports the uncertainty separately from lamp availability. [Scene lamp binding](https://github.com/376east/prismestra/pull/221) · [Command status](https://github.com/376east/prismestra/pull/279)

See the [device compatibility guide](docs/compatibility.md) for supported models, firmware and remaining hardware verification.

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
