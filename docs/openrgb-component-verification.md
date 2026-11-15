# PC backend component

Status: evaluation candidate, 2026-11-15. The installed public ZIP does not yet include this component. This work supplies a pinned source patch, build recipe and bounded verification tools; it does not establish a general installer or clean-Windows acceptance.

## Source and selected features

The standalone engine is [Nexus openrgb-headless at 6bf7d9f5](https://github.com/hello-nexus/openrgb-headless/tree/6bf7d9f5348be41dcf6a45dc7ad57f00b0b69b43), derived from OpenRGB. No Nexus AGPL workspace/service code is imported. Our patch and exact hash are in `components/openrgb/source-lock.json`. It is a modified component, not an official OpenRGB binary.

Build with `CONFIG+=prismestra_pc_backend`. This excludes the entire Philips Hue controller, hueplusplus sources and three mbedTLS link inputs. The vendored [hueplusplus license is LGPLv3](https://github.com/hello-nexus/openrgb-headless/blob/6bf7d9f5348be41dcf6a45dc7ad57f00b0b69b43/dependencies/hueplusplus-1.2.0/LICENSE), [mbedTLS 3.2.1 is Apache-2.0](https://github.com/Mbed-TLS/mbedtls/blob/v3.2.1/LICENSE), and the engine includes GPL-2.0-only controller files. Do not treat that full combination as automatically GPLv2-compatible. The PC variant keeps home-lighting integration in the application's separate adapters. This boundary check is not a complete dependency redistribution audit.

The variant also removes the unconditional detached Windows timer-resolution worker. It does not request a permanent 0.5 ms timer period on the application's behalf. Cooling extensions from the donor are not used by the Prismestra client.

## Busy shared bus

Before creating I2C buses, Windows discovery attempts the existing shared SMBus mutex for at most 100 ms. If busy, USB/HID discovery continues and the existing discovery thread retries the unopened bus phase after 1/2/5/15/30/60 seconds, capped at 60. No extra timeout thread, foreign-process restart or unowned mutex release is used.

Actual PawnIO transfers use owned guards. Failed/timed-out acquisition never reaches the driver call. Discovery shares a 100 ms aggregate mutex-wait budget and stops attempting further bus transfers after contention/cancellation. Normal output uses a separate 100 ms per-transfer limit; this is not a whole-frame latency guarantee. Legacy `shared_smbus_access=false` is explicitly ignored with a warning: this variant always coordinates shared bus access.

If contention begins after discovery has already created buses/controllers, the pass becomes **Partial**. It is not automatically rerun, because the upstream registration API has no deduplication guarantee. An explicit rescan performs cleanup on the discovery owner before rebuilding the inventory. Synchronous kernel/driver calls remain outside the mutex wait bound. A hung IOCTL is not solved by this patch.

## Build and inspect

Use an isolated checkout. After checking out the pinned revision, run `git apply --check` and `git apply --intent-to-add` with `components/openrgb/prismestra-pc.patch`. Preserve the source lock beside that patch. Do not apply it to a separately installed user's OpenRGB.

`scripts/components/build_openrgb.ps1` accepts `-SourceRoot`, `-BuildRoot`, `-QtRoot` (directory containing `bin/qmake.exe`), `-Jom`, `-VsDevCmd` and optional `-Jobs`. It creates temporary short paths, takes an exclusive build-directory lock and compiles the PC variant. It never installs or runs the component. Qt/qmake is a build tool; the produced component must have no Qt imports.

The recipe disables qmake's dependency scan and forces all objects with jom `/A`. This avoids the observed Qt 6.8.3 dependency-scanner failure without permitting stale objects after a header change. Keep stdout/stderr logs for the actual final build.

The PC hardware path keeps a balanced1ms `timeBeginPeriod` request until device cleanup finishes. It does not start the donor's repeating0.5ms worker. Windows builds before19041 are skipped because their request has global scope; `--nodetect` also skips it. The process's prior power-throttling policy is saved, only the timer-resolution-ignore bit is disabled, and the policy is restored afterward. API failures are logged and leave default waits. This preserves existing drivers' short protocol delays; see [cadence evidence and limitations](output-cadence.md).

The release directory needs the EXE, three driver/user-mode DLLs, six PawnIO modules and any imported MSVC runtime files. Do not install a kernel driver as part of these verification scripts. Runtime staging, licenses and corresponding dependency source are separate gates.

The selected HIDAPI and PawnIOLib DLLs are our source builds, pinned separately in `components/openrgb/dependency-lock.json`. `build_openrgb_dependencies.ps1` builds only those user-mode libraries from the caller's verified extracted source archives. Its PawnIO recipe bypasses the repository's WDK/driver targets. Preserve the BSD-style HIDAPI notices, LGPL2.1 PawnIOLib text, exact source archives and recipe with any redistributed component. A rebuild can have a different binary hash and must pass fresh import/ABI/SDK checks; byte-for-byte reproducibility and Authenticode signing are not claimed.

```text
python scripts/components/verify_openrgb_component.py --source-root SOURCE --release-dir RELEASE --source-lock components/openrgb/source-lock.json --dependency-lock components/openrgb/dependency-lock.json --output audit.json
python scripts/components/test_verify_openrgb_component.py
```

The static gate checks the exact source revision/exported patch, fixed file allowlist, source-copy hashes, AMD64 PE imports, missing non-OS dependencies and Qt rejection. An explicit source lock acknowledges an exact derivative; it does not prove independent review, source-to-binary correspondence, signing, license approval or hardware behavior.

## Actual SDK smoke without hardware

### Optional cooperative shutdown

Managed configuration accepts `graceful_shutdown: true` only for a pinned component implementing `PRISMESTRA_PARENT_PIPE=1`. It defaults to false for existing external backends. The manager supplies a private anonymous stdin pipe and clears inherited opt-in for legacy launches. Closing its write end requests normal server stop and cleanup; a retained child gets up to three seconds before the existing forced-termination fallback. Job containment still handles owner crashes. No public SDK shutdown command is added.

The Windows PC component preserves that pipe instead of attaching/reopening console input, and detects EOF without a blocking observer thread. Upstream optional service-shutdown profiles remain honored; enablement can therefore write an exit lighting state. Device initialization, cleanup or synchronous kernel IOCTLs may exceed the grace period. Forced termination can still abandon shared mutex ownership. Do not claim that cooperative shutdown cures a mutex already held by another living process.

`test_parent_pipe.py --exe COMPONENT_EXE --sha256 COMPONENT_SHA --artifacts NEW_DIRECTORY` checks actual EOF after readiness, early EOF, legacy behavior and invalid input with detection disabled, isolated config and owned-child cleanup. It complements the Rust real-child EOF/fallback/owner-crash tests; it does not establish hardware cleanup, clean Windows or optical acceptance.

First build the Rust library test executable with `cargo test --locked --lib --no-run`. Use its actual emitted filename and SHA256, and the component's SHA256:

```text
python scripts/components/test_actual_openrgb.py --exe COMPONENT_EXE --sha256 COMPONENT_SHA --client-test-exe RUST_TEST_EXE --client-test-sha256 TEST_SHA --artifacts NEW_DIRECTORY
```

This starts only a hash-pinned child with `--nodetect`, `--noautoconnect`, isolated APPDATA/config and a private loopback port. Windows listener ownership is checked before the exact prebuilt Rust test runs. A zero-test result fails the gate. The harness closes its own server; it does not leave a Cargo/compiler process tree behind. Windows process/socket observation must be available to the harness.

The actual client negotiates protocol 6, reads empty inventory twice per session and reconnects three times. Empty inventory is intentional. Direct frames, device-list notifications, real discovery and hardware recovery need separate acceptance. The `DeferredI2CPolicyTest.cpp` stand uses the real guard/admission/phase helpers and private mutexes; its simulated independent-work counter is not evidence of actual HID or DetectionManager scheduling.

The historical 140-second physical startup failure remains valid evidence. Neither helper tests nor a no-hardware SDK run erase it. Preserve FanControl and the user's lighting profile, bindings, temperatures and unsaved scene draft during any later installation test.
