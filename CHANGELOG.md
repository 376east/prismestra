# Changelog

This file records notable changes to Prismestra. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Public release numbers use [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

[Download for Windows](https://376east.github.io/prismestra/?download=1) · [Device compatibility](docs/compatibility.md)

## Unreleased

### Changed

- **Download and device lookup.** Download Setup from the product website, README or device catalog. The catalog separates models listed by OpenRGB from models where live-scene support has been checked.
- **Updates.** The tray check uses the app language. If there is no newer Preview, it can offer a newer Stable release. Setup blocks installation over a newer version.

### Fixed

- **First-run discovery.** Find devices remains available after Prismestra automatically connects the first device. The search can be repeated after saving edits to the same profile.
- **Scene edges and RGB status.** Effects that use audio or other inputs can start when monitor lighting is the only scene output. After turning a group back on, its RGB status waits for a new frame submission.
- **Wake from sleep.** RGB and LCD output can resume while a screen capture or temperature reader finishes. Monitor lights can use the scene colors while capture reconnects.
- **LCD status.** After a quick power change, Devices waits for the LCD to acknowledge the current frame or blanking command.

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

## 0.1.0-preview.10 - 2026-11-23

### Added

- **Scene layout.** Place and move RGB devices on the canvas with the mouse or arrow keys. Select overlapping devices individually. [Placement](https://github.com/376east/prismestra/pull/197) · [Overlapping devices](https://github.com/376east/prismestra/pull/198)
- **Scene library.** Search by scene name or effect, try the Quiet Shore gradient wave, and preview audio-reactive effects with a test signal. [Search](https://github.com/376east/prismestra/pull/192) · [Quiet Shore](https://github.com/376east/prismestra/pull/200) · [Audio preview](https://github.com/376east/prismestra/pull/174)
- **Tray scenes.** Switch to a saved scene from the tray after closing the editor without pending changes. [Tray scenes](https://github.com/376east/prismestra/pull/202)
- **Monitor lighting.** Use the active scene’s edge colors for monitor lighting. [Scene edges](https://github.com/376east/prismestra/pull/164)
- **LCD setup.** Use a temperature panel without a background image and choose an existing temperature source from a list. [Temperature panel](https://github.com/376east/prismestra/pull/179) · [Source picker](https://github.com/376east/prismestra/pull/194)
- **Troubleshooting.** Copy a status summary for a selected output or retry a selected LCD connection. [Status summary](https://github.com/376east/prismestra/pull/195) · [LCD retry](https://github.com/376east/prismestra/pull/166)

### Changed

- **Scene editor.** Creating a scene or copying a preset opens the editor with the effect picker focused. [Editor focus](https://github.com/376east/prismestra/pull/181)
- **Device discovery.** Run another search after an empty result, see when Bluetooth is unavailable, and configure an OpenRGB controller manually when the app cannot identify it. [Search retry](https://github.com/376east/prismestra/pull/191) · [Bluetooth](https://github.com/376east/prismestra/pull/187) · [Controller setup](https://github.com/376east/prismestra/pull/193)
- **Portable app.** Launch the EXE directly and configure its managed OpenRGB connection in the app. [Direct launch](https://github.com/376east/prismestra/pull/165) · [Connections](https://github.com/376east/prismestra/pull/175)

### Fixed

- **Network lights.** A failed off command stays visible as an error. The output shows as off after the app successfully sends the muted frame. [Network output status](https://github.com/376east/prismestra/pull/186)
- **Device discovery.** Selecting Search just before the automatic pass starts now starts a manual search. [Early search](https://github.com/376east/prismestra/pull/196)
- **Scene layout.** Drag previews stay within the canvas. [Canvas bounds](https://github.com/376east/prismestra/pull/199)

### Removed

- **Portable download.** The binary ZIP no longer includes the CMD and PowerShell launch scripts. [ZIP packaging](https://github.com/376east/prismestra/pull/189)

## 0.1.0-preview.8 - 2026-11-22

### Changed

- **Scene library.** Start a scene directly from its card. Preview and editing have separate actions. If the scene fails to start, the library links to device status.
- **New profiles.** Start with Peach Surf. Existing profiles keep their saved layout and scene.
- **Motherboard lighting.** Device status shows invalid motherboard zone sizes so you can identify the channel that needs attention.

### Fixed

- **Windows startup.** Reopen the editor from the tray after automatic sign-in startup.
- **Output recovery.** The managed OpenRGB connection can recover in the background. A saved Surplife E1 strip can reconnect when Bluetooth becomes available later. A failed light or display connection leaves independent outputs running.

[Windows verification](https://github.com/376east/prismestra/actions/runs/45023869783)

## 0.1.0-preview.7 - 2026-11-21

### Added

- **Scene files.** Open and save portable scene files. Preview a selected output’s draft LED map before applying changes, including while that output is offline. [Scene and map editor](https://github.com/376east/prismestra/pull/39)
- **Palettes.** Insert and reorder colors and use the built-in Plum Dawn preset. [Palette editing](https://github.com/376east/prismestra/pull/39) · [Plum Dawn](https://github.com/376east/prismestra/pull/51)

### Changed

- **Settings.** LCD and screen pages appear when those outputs are configured. Scheduling and diagnostics move to Settings. [Navigation](https://github.com/376east/prismestra/pull/57)
- **Device status.** Failed BLE writes and LED-map length mismatches appear in diagnostics. Screen fallback reports the capture color space. [Device support](https://github.com/376east/prismestra/pull/40) · [Diagnostics](https://github.com/376east/prismestra/pull/57)

### Fixed

- **Profile import.** Opening a portable ZIP previews the scene. The app waits for Apply before sending its colors to connected lights. [Portable import](https://github.com/376east/prismestra/pull/62)
- **Device selection.** Discovery updates preserve the selected device and unfinished input. Focus returns to the button that opened a settings page. [Device selection](https://github.com/376east/prismestra/pull/57) · [Keyboard focus](https://github.com/376east/prismestra/pull/59)
- **OpenRGB discovery.** An invalid LED layout on one controller leaves valid controllers available. The first scan retries an interrupted read, and controllers with ambiguous identity stay out of Direct mode. [Device support](https://github.com/376east/prismestra/pull/40) · [Discovery recovery](https://github.com/376east/prismestra/pull/63)
- **Surplife E1.** Keep all 153 color positions and send a fresh full frame after an interrupted Bluetooth write. [Device support](https://github.com/376east/prismestra/pull/40) · [Frame recovery](https://github.com/376east/prismestra/pull/38)
- **LCD and RGB recovery.** Ignore stale LCD connection notifications. Working outputs keep playing during supported RGB configuration changes, and effects keep their timing. [Device support](https://github.com/376east/prismestra/pull/40) · [Runtime recovery](https://github.com/376east/prismestra/pull/63)
- **Network lights.** WLED discovery keeps distinct addresses. Home Assistant lights get the current scene color after reconnecting, and authorization failures tell you when to intervene. [Runtime recovery](https://github.com/376east/prismestra/pull/63)
- **Tray and accessibility.** Open the editor from the tray by keyboard. Closing the app ignores further tray activation, and screen readers announce changes to the selected device’s status. [Keyboard access](https://github.com/376east/prismestra/pull/43) · [Exit](https://github.com/376east/prismestra/pull/60) · [Status announcements](https://github.com/376east/prismestra/pull/57)

## 0.1.0-preview.6 - 2026-11-19

### Added

- **First run.** With a fresh profile, the app searches for supported devices and builds an initial lighting map when it finds them.

### Changed

- **Desktop window.** Navigation pages have distinct icons, and the window title is Prismestra.

### Fixed

- **Sign-in and resume.** At Windows sign-in, the app restores the saved scene and outputs. Resume can recover registered outputs while leaving healthy ones running.
- **Scene drafts.** Switching scenes preserves unsaved drafts.
- **Editor recovery.** Launching Prismestra again opens the editor in the running session. The editor can recover after a WebView2 browser crash.

### Known issue

Opening a portable ZIP profile with connected outputs could send its colors before Apply. Preview 7 fixed this. [Portable import fix](https://github.com/376east/prismestra/pull/62)

## 0.1.0-preview.5 - 2026-11-18

### Added

- **Windows Setup.** One x64 installer includes Prismestra, the private OpenRGB backend, and offline Visual C++ and WebView2 runtimes. Setup also offers PawnIO for supported RAM and motherboard lighting.
- **Windows integration.** Choose whether Prismestra starts at sign-in and remove it through Windows Settings. Updates preserve scenes and local device settings. Uninstall leaves user scenes and shared Windows components available.

### Changed

- **PC lighting setup.** The installed app starts its own OpenRGB backend in the background. There is no separate OpenRGB window or tray icon to manage.

[Windows verification](https://github.com/376east/prismestra/actions/runs/44179270592)

## 0.1.0-preview.4 - 2026-11-17

### Added

- **Desktop editor.** Use the WebView2 editor with a native fallback. Save scenes and keep one active while the editor is closed.
- **Device connections.** Drive 153 individual positions on supported Surplife E1 strips and use supported Skydimo hardware for monitor lighting. Configure WLED and Home Assistant connections in the app.

### Changed

- **PC lighting.** Prismestra can manage a headless OpenRGB connection and wait when the hardware bus is busy. An external compatible SDK6 server remains an option.

### Fixed

- **LCD reconnection.** A saved LCD can reconnect after Windows detects its USB interface late or after the display returns.

[Source revision](https://github.com/376east/prismestra/commit/a1579f858d326484a3e81341365f574b65dcb06c)

## 0.1.0-preview.3 - 2026-11-10

### Fixed

- **Temperatures.** Retry brief interruptions while FanControl recreates its temperature-data connection. Previously, a temporary gap could fail the temperature read immediately. [Temperature connection](https://github.com/376east/prismestra/pull/6)

## 0.1.0-preview.2 - 2026-11-09

### Fixed

- **Tray.** Reopening the editor shows the current session status and any device errors. Previously, it could still say the editor was closed. [Source revision](https://github.com/376east/prismestra/commit/7f0ddad1194af8df50f0c9a9b7d4f75b8288e15b)

## 0.1.0-preview.1 - 2026-11-09

### Added

- **Windows portable app.** Open the native editor, close it while the lighting keeps running, and reopen it or stop the scene from the tray.
- **Lighting and displays.** Combine PC lighting through a separately configured OpenRGB server with supported Bluetooth lights and Thermaltake LCD output. Use desktop colors for ambient lighting, or play a GIF with a temperature overlay on the LCD.
- **Network lighting.** Configure WLED and Home Assistant connections. This preview included protocol tests for these integrations. [First preview source revision](https://github.com/376east/prismestra/commit/76e91606b9927efe2f839ff7973c65471f7eee47)
