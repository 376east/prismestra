# Verified status

Prismestra is a Windows preview with a published x64 installer. Its current product is local lighting/display orchestration, with existing cooling software left in control of cooling. The [product scope](../PRODUCT.md) and [compatibility matrix](compatibility.md) define the boundaries.

The published Preview 10 Setup (0.1.0.10) was built from `75d4396f`. Its release CI passed software checks and four Windows lifecycle scenarios: offline Setup, standard-account startup, upgrade from the pinned Preview 7 installer, and synthetic repair and upgrade fixtures. The scenarios cover installation and removal on disposable Windows runners. A separate clean Windows guest check covered actual sign-in, empty device discovery and tray open and exit on the earlier candidate `7f15be0`. A full physical-output check after Windows sign-in on Preview 10 is still open. [Preview 10 source tag](https://github.com/376east/prismestra/tree/v0.1.0-preview.10) · [Release CI](https://github.com/376east/prismestra/actions/runs/45138153893) · [Earlier candidate CI](https://github.com/376east/prismestra/actions/runs/45087638746)

Installed startup now restores saved local outputs after a verified prior Prismestra session and its backend have drained. A bounded background search constructs and saves an initial map for supported OpenRGB, Surplife E1 and TT LCD devices, without requiring Add/Apply actions. Existing layout, disabled devices and manual edits take precedence. Paired profile/binding writes recover interrupted commits and preserve conflicting edits. [Startup coordinator](../src/ui/windows/first_run.rs), [discovery and persistence](../src/app/first_run.rs), [migration](../src/app/installed_migration.rs).

Software fixtures cover map geometry, actual generated frames through the engine, save/reload, recovery conflicts and migration provenance. They do not certify a particular device's passive advertisements or physical output. The ordinary-account installer harness additionally requires the real resident startup coordinator to complete a discovery pass and drain normally. Published release notes identify which exact installer ran that harness. [Installer scope](windows-installer.md).

| Area | Implemented and checked | Remaining limit |
|---|---|---|
| Editor and resident engine | WebView2 editor, native fallback, persistent scenes/drafts, rendering after editor closure; configured-kit autostart observed after a real reboot | A full physical-output check after Windows sign-in on Preview 10 is open |
| PC lighting | Owned headless OpenRGB adapter, SDK6 ownership checks, initial busy-bus deferral, graceful drain and recovery | Mid-scan contention is Partial; kernel calls cannot be universally bounded |
| BLE, LCD and monitor lighting | Direct selected-protocol adapters and temperature inputs; one configured kit accepted previously | Support is protocol/firmware-specific, not every device with Bluetooth/USB |
| Home lighting | WLED and Home Assistant connection/setup/output paths, software coverage | Other physical devices and installations need acceptance |
| Sleep and helper ownership | Protocol2 uses active Windows time; deterministic sleep/UTC/failure tests | Actual sleep/resume remains separate from simulated tests |
| Delivery | Single-file offline Inno Setup installer tooling, per-user bootstrap, protected own-user backend policy, repair/upgrade/uninstall harness; [installer contract](windows-installer.md) | See release lifecycle evidence for actual install scope. Cold boot on fresh Windows 10/11, all hardware, product signing and bit-identical builds remain separate gates |

## Last native acceptance checkpoint

The installed application from commit **4c7d15d15a6c1aa78622194c95d30f178c428ccb** passed a five-minute warm-session observation and an owned-only restart with both RAM controllers receiving advancing writes and no instrumented failures. The foreign cooling process/settings and saved scene/config were preserved. Software checks for that revision passed 644 Rust tests, seven ignored entries, strict Clippy/fmt and five exact packaged-EXE stands. [Sanitized receipt](validation/boot-reliability-20261116.json), [push CI](https://github.com/malyarq/prismestra/actions/runs/43823058102), [PR CI](https://github.com/malyarq/prismestra/actions/runs/43823064450).

This is **not** a new reboot/logon result, an optical FPS measurement or a universal coexistence guarantee. Historical helper replacement causes remain unknown. Later documentation/tooling commits do not retroactively change that tested binary's revision. Raw workstation receipts stay local; the repository contains sanitized scope/provenance summaries.

## Current product acceptance gate

On 16 November 2026 the user performed a real reboot with the installed **4c7d15d** binary. Automatic startup and a subsequent 301.074-second observation passed for both RAM controllers, with advancing write counters, no instrumented write failures and unchanged FanControl process/settings during observation. Active scene, bindings, sensor sources and backend configuration matched their pre-boot hashes. The editor-only draft differed and was explicitly recorded without overwriting it. The user reported the other outputs working except LCD. [Sanitized real-boot receipt](validation/realboot-20261116.json).

The LCD's saved USB device was absent; its previous port matched a failed USB descriptor request/code43 node. One explicitly authorized targeted node restart did not restore enumeration. The firmware/power/cable cause is unknown. This is a **partial** real-boot result, not successful full-scene acceptance. The source adds bound-interface arrival recovery, but physical return/reconnect of the LCD remains unverified; it cannot repair a missing Windows USB device.

An actual Windows reboot/sign-in must start the saved scene without manually opening the program or restarting another application. Observe the expected RAM output for at least five minutes, preserve cooling settings and verify visible PC/BLE/LCD/monitor output separately. Agree the disruptive reboot/sleep timing before performing it. [Startup/recovery contract](startup-recovery.md).

Repository consolidation, green tests and replaying a scheduled task do not close this gate. See [development workflow](development.md) for authoritative files and supported tools.
