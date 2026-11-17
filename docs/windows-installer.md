# Windows installer

The supported entry point is `Prismestra-Setup-0.1.0.5-x64.exe` on the [Preview 5 release](https://github.com/malyarq/prismestra/releases/tag/v0.1.0-preview.5). This is a Windows 10/11 x64 installer, not a portable ZIP. Device support remains model- and firmware-specific; see [compatibility](compatibility.md).

## Install and use

Run Setup from your ordinary Windows account and approve Windows elevation for installation. Setup places protected application files under Program Files and adds a Start-menu entry and Windows uninstall registration. The sign-in startup option starts one resident session; closing its editor keeps the saved scene running. A fresh profile is empty: discover supported devices, configure connections and save a scene. Subsequent sign-ins restore that saved configuration.

Each Windows account stores its own scenes, local bindings, assets and editor state under `%LOCALAPPDATA%/Prismestra`. An update replaces application files and the private backend while preserving that data. The installer asks a running resident session to exit normally and respects unsaved changes. Finish those changes if Setup asks you to close the app.

Uninstall through Windows Settings. It removes its own startup command, application and managed backend task. Saved user data remains available for reinstallation. Shared Visual C++, WebView2 and PawnIO installations remain available to other programs. Setup never stops FanControl or other vendor software.

## Bundled components and permissions

- Prismestra's Rust application and local WebView2 editor.
- A private, pinned OpenRGB-derived headless backend, with matching sources, patch, licenses and build recipes. It is not an independently installed OpenRGB GUI and has no second tray icon.
- Microsoft's full offline x64 Visual C++ and WebView2 installers, used only when the required runtime is missing. [Microsoft documents offline WebView2 redistribution](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution#offline-deployment).
- The optional official signed [PawnIO](https://pawnio.eu/) installer for supported low-level PC lighting. Installing it does not guarantee a particular RAM/controller model is supported. The driver is shared; Prismestra does not remove it on uninstall.

The private backend uses loopback port 17422 rather than OpenRGB’s usual 6742, so a separate installation can keep its SDK listener. The editor discovers through the configured managed endpoint when no custom address is entered. A collision still fails safely instead of stopping another program.

The editor runs without administrator privileges. For the installing user's own split-token administrator account, Setup can register a fixed on-demand elevated backend task when PawnIO is available. The task uses protected machine policy and immutable binaries; its SDK accepts bounded lighting commands and excludes cooling, profiles, plugins and settings. The task cannot elevate arbitrary user-selected executables or configuration paths. Over-the-shoulder administrator credentials do not grant that elevated task to a different standard account. That account can still use ordinary USB/BLE/network paths; some RAM/SMBus devices require administrator-authorized access. See [managed backend](managed-openrgb.md).

This installer does not import another RGB application's scene or take ownership of all discovered hardware. Multiple writers can still conflict on the same lighting controller. Support for additional models requires compatible protocols and independent hardware validation.

## Build and verification

Maintained sources: [`scripts/package_installer.py`](../scripts/package_installer.py), [`scripts/installer`](../scripts/installer), and the protected task provisioner. Prerequisite downloads are locked by size, SHA-256, official origin and Authenticode publisher in `prerequisites.lock.json`. Build dependencies are not installed on the end user's machine.

Use a clean committed checkout, build its app/source release with `scripts/package_release.py`, verify/package the matching PC component, then stage and compile with:

```powershell
python scripts/package_installer.py --app-exe APP_EXE --app-sha256 APP_SHA256 --component-zip COMPONENT_ZIP --component-sha256 COMPONENT_SHA256 --expected-commit COMMIT --version 0.1.0.5 --output NEW_OUTPUT_DIRECTORY --release-dir APPLICATION_NOTICE_DIRECTORY --prerequisite-lock scripts/installer/prerequisites.lock.json --prerequisite-dir VERIFIED_OFFLINE_DOWNLOADS --iscc ISCC_EXE
```

`--test-namespace GUID` creates a separately named lifecycle fixture without drivers, prerequisites or elevated tasks. `test_lifecycle.ps1` distinguishes ordinary interactive startup/live-drain checks from elevated disposable-runner file/repair/upgrade/uninstall checks. Those are different gates. Published release notes record actual results; software fixtures do not certify untested hardware or a cold boot on every Windows version.
