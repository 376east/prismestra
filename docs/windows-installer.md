# Windows installer

Download and run one `Prismestra-Setup-<version>-x64.exe` for Windows 10/11 x64. This guide describes Preview 7. The published Preview 6 Setup bundles both Microsoft runtime installers. Device support depends on the model and firmware. [Compatibility](compatibility.md).

## Install and use

Run Setup from your ordinary Windows account, approve Windows elevation, and review and accept the Microsoft WebView2 Runtime terms in Setup. Setup places protected application files under Program Files and adds a Start-menu entry and Windows uninstall registration. The sign-in startup option starts one resident session; closing its editor keeps the saved scene running. The first resident launch restores a verified existing Prismestra profile, or discovers supported local devices and saves a working scene and initial map automatically. Subsequent sign-ins restore saved outputs and run a bounded discovery pass for additional supported devices. Existing device placement and disabled outputs are preserved.

Setup reuses Visual C++ 14.51.36247.0 or newer and WebView2 105 or newer. If a compatible WebView2 Runtime is missing, Setup uses the bundled, Microsoft-signed x64 Evergreen Standalone installer. If a compatible Visual C++ Runtime is missing, Setup downloads the Microsoft-signed x64 Visual C++ Redistributable directly from Microsoft and verifies its SHA-256 before running it. The build verifies the Authenticode signatures of both Microsoft prerequisite packages. The private OpenRGB backend uses the shared Visual C++ Runtime. Upgrades remove only previously shipped VC++ DLLs that match their known hashes. [Prerequisite lock](../scripts/installer/prerequisites.lock.json) · [Microsoft VC++ Runtime license](https://visualstudio.microsoft.com/license-terms/vs2026-ga-visualcpp-v14-redist-runtime/).

When Windows requests a restart, Setup saves the verified portable source location but does not launch the application or retire prior portable startup tasks. After the restart, the selected sign-in startup launches the app and can import that source; otherwise open it from the Start menu. Rerun Setup after restarting to finish verified retirement of prior portable startup tasks.

Each Windows account stores its own scenes, local bindings, assets and editor state under `%LOCALAPPDATA%/Prismestra`. An update replaces application files and the private backend while preserving that data. The installer asks a running resident session to exit normally and respects unsaved changes. Finish those changes if Setup asks you to close the app.

Uninstall through Windows Settings. It removes its own startup command, application and managed backend task. Saved user data remains available for reinstallation. Shared Visual C++, WebView2 and PawnIO installations remain available to other programs. Setup never stops FanControl or other vendor software.

An unambiguous same-user running portable Prismestra session can supply its saved profile, local bindings, media and sensor sources. The new app asks only that verified Prismestra session to exit normally and waits for its owned backend before activating the migrated scene. Unsaved edits can block this handover. Other RGB applications and their profiles are not imported or stopped. Ambiguous profiles are preserved rather than selected arbitrarily.

During the existing installation elevation, Setup can also recognize the pinned legacy Prismestra launcher in that same user's logon tasks. It passes the verified source to an ordinary-user profile preparation step, confirms that exact source was imported, and only then disables those old future triggers. The new startup preference must have been saved successfully first. Running tasks are not stopped; their definitions and access controls are retained in a protected rollback record. Unknown launchers, ambiguous sources and different administrator credentials leave previous entries unchanged and are reported in the Setup log.

## Visual C++ download errors

If Setup says it could not download or verify the required x64 Visual C++ runtime, it stops before closing Prismestra or running the downloaded file. Setup writes a log in the Windows temporary directory (`%TEMP%`). Look at the details for `vc_redist.x64.exe`. [Inno Setup logging](https://jrsoftware.org/inno-setup/documentation/topic_setupcmdline.htm#LOG).

- A connection error, such as `Error sending data`, indicates a download problem. Check your connection and rerun the same Setup file.
- `File hash is incorrect` means the download failed the SHA-256 check. Install the current x64 Visual C++ Redistributable directly from Microsoft, then rerun the same Setup file. Use this fallback if the download error persists or the target PC is offline. For an offline PC, transfer the Microsoft installer from a connected PC. [Microsoft Visual C++ downloads](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist/#latest-supported-version).

## Automatic setup scope

The built-in first map uses physical Direct-capable controllers reported by the owned OpenRGB backend, positively identified TT LCD interfaces and compatible Surplife E1 controllers that confirm their actual pixel capacity. BLE discovery first requires a precise advertisement signature and then a read-only capability query; a Bluetooth name alone never enables a new protocol. A fresh LCD gets a bundled generated animation, while migration preserves selected media and sensor mappings. Available temperature providers are not invented.

The initial map preserves LED order and native controller geometry, with separate tiles for devices. Software cannot infer where a strip runs around a room: move tiles to match the installation. Scans are finite and repeated during startup to allow late devices to appear; a temporarily unavailable path does not permanently close discovery. Stop/Exit cancels the scan, and manual editing takes precedence over a pending automatic result.

Unknown serial adapters, unconfirmed LED counts, unsupported firmware and Home Assistant authentication still need explicit setup. WLED and Home Assistant retain their existing network setup paths. Automatic setup is not a promise to control every USB, BLE or network device. If Windows cannot enumerate a USB device, the app cannot repair its cable, power or firmware.

Profile and local binding updates use a paired recovery journal. Interrupted known writes can complete on restart; a conflicting journal is preserved separately without replacing subsequent user edits or preventing the editor from opening.

## Bundled components and permissions

- Prismestra's Rust application and local WebView2 editor.
- A private, pinned OpenRGB-derived headless backend, with matching sources, patch, licenses and build recipes. It is not an independently installed OpenRGB GUI and has no second tray icon.
- Microsoft's x64 Evergreen WebView2 Standalone installer, used when WebView2 is missing or too old. [Microsoft's offline deployment instructions](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution#offline-deployment) · [WebView2 Runtime license at download](https://developer.microsoft.com/en-us/microsoft-edge/webview2/#download).
- The official signed [PawnIO](https://pawnio.eu/) installer for supported low-level PC lighting, selected by default and optional in Setup. Installing it does not guarantee a particular RAM/controller model is supported. The driver is shared; Prismestra does not remove it on uninstall.

The private backend uses loopback port 17422 rather than OpenRGB’s usual 6742, so a separate installation can keep its SDK listener. The editor discovers through the configured managed endpoint when no custom address is entered. A collision still fails safely instead of stopping another program.

The editor runs without administrator privileges. For the installing user's own split-token administrator account, Setup can register a fixed on-demand elevated backend task when PawnIO is available. The task uses protected machine policy and immutable binaries; its SDK accepts bounded lighting commands and excludes cooling, profiles, plugins and settings. The task cannot elevate arbitrary user-selected executables or configuration paths. Over-the-shoulder administrator credentials do not grant that elevated task to a different standard account. That account can still use ordinary USB/BLE/network paths; some RAM/SMBus devices require administrator-authorized access. See [managed backend](managed-openrgb.md).

This installer does not import another RGB application's scene or take ownership of all discovered hardware. Multiple writers can still conflict on the same lighting controller. Support for additional models requires compatible protocols and independent hardware validation.

## Microsoft WebView2

Prismestra uses WebView2 with Microsoft Defender SmartScreen. SmartScreen collects and sends user information to Microsoft. [Microsoft Privacy Statement](https://aka.ms/privacy) · [Microsoft Edge privacy documentation](https://learn.microsoft.com/en-us/legal/microsoft-edge/privacy#smartscreen).

Review the Microsoft WebView2 Runtime license by selecting x64 under Evergreen Standalone Installer on the Microsoft download page. Unattended Setup requires `/ACCEPTWEBVIEW2TERMS` after review and acceptance. [Microsoft license at download](https://developer.microsoft.com/en-us/microsoft-edge/webview2/#download).

## Build and verification

Maintained sources: [`scripts/package_installer.py`](../scripts/package_installer.py), [`scripts/installer`](../scripts/installer), and the protected task provisioner. Prerequisite downloads are locked by size, SHA-256, official origin and Authenticode publisher in `prerequisites.lock.json`. Build dependencies are not installed on the end user's machine.

Use a clean committed checkout, build its app/source release with `scripts/package_release.py`, verify/package the matching PC component, then stage and compile with:

```powershell
python scripts/package_installer.py --app-exe APP_EXE --app-sha256 APP_SHA256 --component-zip COMPONENT_ZIP --component-sha256 COMPONENT_SHA256 --expected-commit COMMIT --version 0.1.0.7 --output NEW_OUTPUT_DIRECTORY --release-dir APPLICATION_NOTICE_DIRECTORY --prerequisite-lock scripts/installer/prerequisites.lock.json --prerequisite-dir VERIFIED_PREREQUISITES --iscc ISCC_EXE
```

`--test-namespace GUID` creates a separately named lifecycle fixture without drivers, prerequisites or elevated tasks. `test_lifecycle.ps1` distinguishes ordinary interactive startup/live-drain checks from elevated disposable-runner file/repair/upgrade/uninstall checks. Those are different gates. Published release notes record actual results; software fixtures do not certify untested hardware or a cold boot on every Windows version.

### Windows Actions candidate

Use **Windows checks** → **Run workflow** on the intended revision and provide a four-part Windows version. Pushes to delivery CI branches and preview tags also run packaging. A preview tag such as `v0.1.0-preview.7` supplies version `0.1.0.7`. The job builds on a native x64 Windows runner with Visual Studio 2022 to stay within the required Visual C++ runtime floor. It uses pinned Rust and Inno versions, the application and component recipes, and the current OpenRGB lock and patch. Its pinned published Preview 5 source companion supplies verified dependency files and corresponding source archives. The previous backend executable is excluded. The build checks the new backend inventory and tests its parent lease with hardware detection disabled.

Update the VC Runtime CDN URL, SHA-256, size and version together in `prerequisites.lock.json`. The build verifies Microsoft's signature on the exact downloadable VC Runtime while staging only WebView2 and PawnIO as embedded prerequisites. WebView2 also uses a version-specific Microsoft CDN URL. `PRISMESTRA_WEBVIEW2_X64_URL` can supply another HTTPS location for the exact pinned bytes. Size, SHA-256 and Authenticode checks still apply.

The run stores the compiled Setup, source companion, installation manifest and checksums as an Actions artifact. Separate artifacts contain synthetic repair/upgrade inputs and lifecycle evidence. The fresh-install and fresh-account jobs run the **same production Setup SHA-256** on separate disposable runners. The fresh account checks HKCU registration and executes the installed command under a standard token. Setup itself runs under the elevated CI account. True interactive UAC, a reboot or Windows sign-in, missing runtimes, optional driver provisioning and physical devices require separate acceptance. The production-upgrade job installs published Preview 6 verified against the baseline lock, writes a synthetic per-user profile, and upgrades using the same candidate Setup SHA-256. It verifies version, installed manifest and app/backend hashes, and data preservation after update and uninstall. No app session runs during this job. The synthetic repair/upgrade cases check files and saved data through a separate AppId.

The publisher is responsible for the WebView2 license obligations that apply to distribution. Those obligations include the commitments in section 2 of the Microsoft license. [Microsoft license at download](https://developer.microsoft.com/en-us/microsoft-edge/webview2/#download).

### Visual C++ download pin

Each released Setup embeds the exact version-specific Microsoft CDN URL and SHA-256 from [the prerequisite lock](../scripts/installer/prerequisites.lock.json). Microsoft publishes a moving [latest x64 link](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist/#latest-supported-version) and makes some older versions available in Dev Essentials. Microsoft does not document a retention period for the pinned CDN URL. The build fetches that exact URL and verifies size, SHA-256 and Microsoft Authenticode before every candidate. Recheck the exact URL before releasing the candidate.

If Microsoft removes the URL or returns different bytes, Setup stops before launching the downloaded executable or closing Prismestra. An existing compatible runtime still works. A user can install the current x64 runtime from Microsoft and retry the same Setup. To recover automatic download in a subsequent release, update the version, URL, size and SHA-256 together, rebuild Setup and verify the missing-runtime path again.

The dedicated VC download job runs on a fresh GitHub-hosted VM. Before using Microsoft's uninstaller it verifies recovery media for the same installed version. It requires the shared x64 runtime DLLs to be absent before running the candidate, and checks verified download, runtime installation and Prismestra uninstall without a restart. If the candidate fails to leave a compatible runtime installed, the job reinstalls the original version from the verified recovery package. The runner image must already lack the runtime or match a version covered by the recovery pins.

Only after all required jobs pass does the run retain `prismestra-verified-candidate-<attempt>` with the tested bytes. This workflow has read-only repository permissions and does not publish a release. A newly compiled or signed EXE requires its own installation checks.
