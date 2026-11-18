# Windows installer

The supported entry point is `Prismestra-Setup-0.1.0.6-x64.exe` on the [Preview 6 release](https://github.com/malyarq/prismestra/releases/tag/v0.1.0-preview.6). This is a Windows 10/11 x64 installer, not a portable ZIP. Device support remains model- and firmware-specific; see [compatibility](compatibility.md).

## Install and use

Run Setup from your ordinary Windows account and approve Windows elevation for installation. Setup places protected application files under Program Files and adds a Start-menu entry and Windows uninstall registration. The sign-in startup option starts one resident session; closing its editor keeps the saved scene running. The first resident launch restores a verified existing Prismestra profile, or discovers supported local devices and saves a working scene and initial map automatically. Subsequent sign-ins restore saved outputs and run a bounded discovery pass for additional supported devices. Existing device placement and disabled outputs are preserved.

Compatible existing runtimes are reused. The current build needs Visual C++ 14.51.36247.0 or newer and WebView2 105 or newer; the bundled installers can update an older installation. When Windows requests a restart, Setup saves the verified portable source location but does not launch the application or retire prior portable startup tasks. After the restart, the selected sign-in startup launches the app and can import that source; otherwise open it from the Start menu. Rerun Setup after restarting to finish verified retirement of prior portable startup tasks.

Each Windows account stores its own scenes, local bindings, assets and editor state under `%LOCALAPPDATA%/Prismestra`. An update replaces application files and the private backend while preserving that data. The installer asks a running resident session to exit normally and respects unsaved changes. Finish those changes if Setup asks you to close the app.

Uninstall through Windows Settings. It removes its own startup command, application and managed backend task. Saved user data remains available for reinstallation. Shared Visual C++, WebView2 and PawnIO installations remain available to other programs. Setup never stops FanControl or other vendor software.

An unambiguous same-user running portable Prismestra session can supply its saved profile, local bindings, media and sensor sources. The new app asks only that verified Prismestra session to exit normally and waits for its owned backend before activating the migrated scene. Unsaved edits can block this handover. Other RGB applications and their profiles are not imported or stopped. Ambiguous profiles are preserved rather than selected arbitrarily.

During the existing installation elevation, Setup can also recognize the pinned legacy Prismestra launcher in that same user's logon tasks. It passes the verified source to an ordinary-user profile preparation step, confirms that exact source was imported, and only then disables those old future triggers. The new startup preference must have been saved successfully first. Running tasks are not stopped; their definitions and access controls are retained in a protected rollback record. Unknown launchers, ambiguous sources and different administrator credentials leave previous entries unchanged and are reported in the Setup log.

## Automatic setup scope

The built-in first map uses physical Direct-capable controllers reported by the owned OpenRGB backend, positively identified TT LCD interfaces and compatible Surplife E1 controllers that confirm their actual pixel capacity. BLE discovery first requires a precise advertisement signature and then a read-only capability query; a Bluetooth name alone never enables a new protocol. A fresh LCD gets a bundled generated animation, while migration preserves selected media and sensor mappings. Available temperature providers are not invented.

The initial map preserves LED order and native controller geometry, with separate tiles for devices. Software cannot infer where a strip runs around a room: move tiles to match the installation. Scans are finite and repeated during startup to allow late devices to appear; a temporarily unavailable path does not permanently close discovery. Stop/Exit cancels the scan, and manual editing takes precedence over a pending automatic result.

Unknown serial adapters, unconfirmed LED counts, unsupported firmware and Home Assistant authentication still need explicit setup. WLED and Home Assistant retain their existing network setup paths. Automatic setup is not a promise to control every USB, BLE or network device. If Windows cannot enumerate a USB device, the app cannot repair its cable, power or firmware.

Profile and local binding updates use a paired recovery journal. Interrupted known writes can complete on restart; a conflicting journal is preserved separately without replacing subsequent user edits or preventing the editor from opening.

## Bundled components and permissions

- Prismestra's Rust application and local WebView2 editor.
- A private, pinned OpenRGB-derived headless backend, with matching sources, patch, licenses and build recipes. It is not an independently installed OpenRGB GUI and has no second tray icon.
- Microsoft's full offline x64 Visual C++ and WebView2 installers, used only when the required runtime is missing. [Microsoft documents offline WebView2 redistribution](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution#offline-deployment).
- The official signed [PawnIO](https://pawnio.eu/) installer for supported low-level PC lighting, selected by default and optional in Setup. Installing it does not guarantee a particular RAM/controller model is supported. The driver is shared; Prismestra does not remove it on uninstall.

The private backend uses loopback port 17422 rather than OpenRGB’s usual 6742, so a separate installation can keep its SDK listener. The editor discovers through the configured managed endpoint when no custom address is entered. A collision still fails safely instead of stopping another program.

The editor runs without administrator privileges. For the installing user's own split-token administrator account, Setup can register a fixed on-demand elevated backend task when PawnIO is available. The task uses protected machine policy and immutable binaries; its SDK accepts bounded lighting commands and excludes cooling, profiles, plugins and settings. The task cannot elevate arbitrary user-selected executables or configuration paths. Over-the-shoulder administrator credentials do not grant that elevated task to a different standard account. That account can still use ordinary USB/BLE/network paths; some RAM/SMBus devices require administrator-authorized access. See [managed backend](managed-openrgb.md).

This installer does not import another RGB application's scene or take ownership of all discovered hardware. Multiple writers can still conflict on the same lighting controller. Support for additional models requires compatible protocols and independent hardware validation.

## Build and verification

Maintained sources: [`scripts/package_installer.py`](../scripts/package_installer.py), [`scripts/installer`](../scripts/installer), and the protected task provisioner. Prerequisite downloads are locked by size, SHA-256, official origin and Authenticode publisher in `prerequisites.lock.json`. Build dependencies are not installed on the end user's machine.

Use a clean committed checkout, build its app/source release with `scripts/package_release.py`, verify/package the matching PC component, then stage and compile with:

```powershell
python scripts/package_installer.py --app-exe APP_EXE --app-sha256 APP_SHA256 --component-zip COMPONENT_ZIP --component-sha256 COMPONENT_SHA256 --expected-commit COMMIT --version 0.1.0.6 --output NEW_OUTPUT_DIRECTORY --release-dir APPLICATION_NOTICE_DIRECTORY --prerequisite-lock scripts/installer/prerequisites.lock.json --prerequisite-dir VERIFIED_OFFLINE_DOWNLOADS --iscc ISCC_EXE
```

`--test-namespace GUID` creates a separately named lifecycle fixture without drivers, prerequisites or elevated tasks. `test_lifecycle.ps1` distinguishes ordinary interactive startup/live-drain checks from elevated disposable-runner file/repair/upgrade/uninstall checks. Those are different gates. Published release notes record actual results; software fixtures do not certify untested hardware or a cold boot on every Windows version.
