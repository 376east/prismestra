# Verified status

Prismestra **0.2.0** is available as `Prismestra-Setup-0.2.0.0-x64.exe`. It coordinates lighting and displays on Windows while existing cooling software stays in control of cooling. [Download 0.2.0](https://github.com/376east/prismestra/releases/tag/v0.2.0) · [Device compatibility](compatibility.md)

## Release verification

Release CI built and published Setup from `2c1675c2`. Quick and Full software checks passed, along with four Windows lifecycle jobs on disposable runners covering offline Setup, standard-account startup, upgrade from the pinned Preview 7 installer, and synthetic repair and upgrade. [Source commit](https://github.com/376east/prismestra/commit/2c1675c2f9d4165c80012414eef5ae1b16f822b3) · [Release CI #45747426619](https://github.com/376east/prismestra/actions/runs/45747426619)

The downloaded Setup matched the published SHA-256 `0818ed7e00983a40c9616257d4dcaf440a6cdf207941404e50463828f7625482`. Windows Authenticode inspection of that EXE returned `NotSigned`. Physical output on real devices remains unverified for this exact release, including after Windows sign-in. [Release and checksum](https://github.com/376east/prismestra/releases/tag/v0.2.0)

## Implemented behavior

Installed startup now restores saved local outputs after a verified prior Prismestra session and its backend have drained. A bounded background search constructs and saves an initial map for supported OpenRGB, Surplife E1 and TT LCD devices, without requiring Add/Apply actions. Existing layout, disabled devices and manual edits take precedence. Paired profile/binding writes recover interrupted commits and preserve conflicting edits. [Startup coordinator](../src/ui/windows/first_run.rs), [discovery and persistence](../src/app/first_run.rs), [migration](../src/app/installed_migration.rs).

Software fixtures cover map geometry, actual generated frames through the engine, save/reload, recovery conflicts and migration provenance. They do not certify a particular device's passive advertisements or physical output. The ordinary-account installer harness additionally requires the real resident startup coordinator to complete a discovery pass and drain normally. Published release notes identify which exact installer ran that harness. [Installer scope](windows-installer.md).

| Area | Implemented and checked | Remaining limit |
|---|---|---|
| Editor and resident engine | WebView2 editor, native fallback, persistent scenes/drafts, rendering after editor closure. Autostart observed on an earlier configured-kit build. | Physical output at sign-in still needs acceptance on 0.2.0 |
| PC lighting | Owned headless OpenRGB adapter, SDK6 ownership checks, initial busy-bus deferral, graceful drain and recovery | Mid-scan contention is Partial; kernel calls cannot be universally bounded |
| BLE, LCD and monitor lighting | Direct adapters for selected protocols and temperature inputs. See the historical checks below. | Coverage depends on model, protocol and firmware |
| Home lighting | WLED and Home Assistant setup and output paths with software coverage, plus the Alice pilot | Physical acceptance depends on the device and installation |
| Sleep and helper ownership | Protocol2 uses active Windows time; deterministic sleep/UTC/failure tests | Actual sleep/resume remains separate from simulated tests |
| Delivery | Single-file offline Inno Setup installer tooling, per-user bootstrap, protected own-user backend policy, repair/upgrade/uninstall harness; [installer contract](windows-installer.md) | See release lifecycle evidence for actual install scope. Cold boot on fresh Windows 10/11, broader hardware validation, signing and bit-identical builds have separate checks. |

## Historical acceptance checkpoints

Preview 10 Setup (0.1.0.10, `75d4396f`) passed software and four Windows lifecycle checks on disposable runners. A separate clean Windows guest on the earlier candidate `7f15be0` covered actual sign-in, empty discovery and tray open and exit. Physical output after sign-in on Preview 10 remained open. [Preview 10 source](https://github.com/376east/prismestra/commit/75d4396f20c7986280ccd794e3b2663ab40ad166) · [Preview 10 CI](https://github.com/376east/prismestra/actions/runs/45138153893) · [Earlier candidate CI](https://github.com/376east/prismestra/actions/runs/45087638746)

### Warm-session observation

The installed application from commit **4c7d15d15a6c1aa78622194c95d30f178c428ccb** passed a five-minute warm-session observation and an owned-only restart with both RAM controllers receiving advancing writes and no instrumented failures. The foreign cooling process/settings and saved scene/config were preserved. Software checks for that revision passed 644 Rust tests, seven ignored entries, strict Clippy/fmt and five exact packaged-EXE stands. [Sanitized receipt](validation/boot-reliability-20261116.json), [push CI](https://github.com/malyarq/prismestra/actions/runs/43823058102), [PR CI](https://github.com/malyarq/prismestra/actions/runs/43823064450).

This checkpoint covers runtime writes and coexistence on one warm-session kit. Reboot and optical FPS were outside its scope. The causes of earlier helper replacement remain unknown. Raw workstation receipts stay local. The repository records sanitized scope and provenance.

### Real reboot on 16 November 2026

On 16 November 2026 the user performed a real reboot with the installed **4c7d15d** binary. Automatic startup and a subsequent 301.074-second observation passed for both RAM controllers, with advancing write counters, no instrumented write failures and unchanged FanControl process/settings during observation. Active scene, bindings, sensor sources and backend configuration matched their pre-boot hashes. The editor-only draft differed and was explicitly recorded without overwriting it. The user reported the other outputs working except LCD. [Sanitized real-boot receipt](validation/realboot-20261116.json).

The LCD's saved USB device was absent; its previous port matched a failed USB descriptor request/code43 node. One explicitly authorized targeted node restart did not restore enumeration. The firmware/power/cable cause is unknown. The real-boot result was **partial**. Bound-interface arrival recovery applies once Windows enumerates the device. Physical LCD return and reconnect remain unverified.

## Physical acceptance after sign-in

An actual Windows reboot/sign-in must start the saved scene without manually opening the program or restarting another application. Observe the expected RAM output for at least five minutes, preserve cooling settings and verify visible PC/BLE/LCD/monitor output separately. Agree the disruptive reboot/sleep timing before performing it. [Startup/recovery contract](startup-recovery.md).

Repository consolidation, green tests and replaying a scheduled task do not close this gate. See [development workflow](development.md) for authoritative files and supported tools.
