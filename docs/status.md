# Verified status

Prismestra is a Windows source preview. Its current product is local lighting/display orchestration, with existing cooling software left in control of cooling. The [product scope](../PRODUCT.md) and [compatibility matrix](compatibility.md) define the boundaries.

| Area | Implemented and checked | Remaining limit |
|---|---|---|
| Editor and resident engine | WebView2 editor, native fallback, persistent scenes/drafts, rendering after editor closure | A real new unattended reboot/logon remains open |
| PC lighting | Owned headless OpenRGB adapter, SDK6 ownership checks, initial busy-bus deferral, graceful drain and recovery | Mid-scan contention is Partial; kernel calls cannot be universally bounded |
| BLE, LCD and monitor lighting | Direct selected-protocol adapters and temperature inputs; one configured kit accepted previously | Support is protocol/firmware-specific, not every device with Bluetooth/USB |
| Home lighting | WLED and Home Assistant connection/setup/output paths, software coverage | Other physical devices and installations need acceptance |
| Sleep and helper ownership | Protocol2 uses active Windows time; deterministic sleep/UTC/failure tests | Actual sleep/resume remains separate from simulated tests |
| Delivery | Pinned build recipes, dependencies/component patch and source/binary package tooling | General privileged installer, clean Windows, signing and bit-identical builds are not established |

## Last native acceptance checkpoint

The installed application from commit **4c7d15d15a6c1aa78622194c95d30f178c428ccb** passed a five-minute warm-session observation and an owned-only restart with both RAM controllers receiving advancing writes and no instrumented failures. The foreign cooling process/settings and saved scene/config were preserved. Software checks for that revision passed644Rust tests, seven ignored entries, strict Clippy/fmt and five exact packaged-EXE stands. [Sanitized receipt](validation/boot-reliability-20261116.json), [push CI](https://github.com/malyarq/prismestra/actions/runs/43823058102), [PR CI](https://github.com/malyarq/prismestra/actions/runs/43823064450).

This is **not** a new reboot/logon result, an optical FPS measurement or a universal coexistence guarantee. Historical helper replacement causes remain unknown. Later documentation/tooling commits do not retroactively change that tested binary's revision. Raw workstation receipts stay local; the repository contains sanitized scope/provenance summaries.

## Current product acceptance gate

An actual Windows reboot/sign-in must start the saved scene without manually opening the program or restarting another application. Observe the expected RAM output for at least five minutes, preserve cooling settings and verify visible PC/BLE/LCD/monitor output separately. Agree the disruptive reboot/sleep timing before performing it. [Startup/recovery contract](startup-recovery.md).

Repository consolidation, green tests and replaying a scheduled task do not close this gate. See [development workflow](development.md) for authoritative files and supported tools.
