# Delivery and coexistence contract

Status: proposed product delivery contract, 2026-11-15. This describes the intended release experience, not the capabilities of the current ZIP. The current package does not bundle OpenRGB or provision a general privileged helper. A physical run waited on a foreign SMBus owner beyond a 140-second acceptance limit; a later follow-up observed four recovered SDK output connections. This does not establish general coexistence or consistent recovery latency.

## The ordinary user's path

1. Install or extract Prismestra. A fresh supported Windows machine does not require a separately installed OpenRGB application.
2. Discover supported devices and show each device's state. Enable only the selected lighting/display outputs. Keep cooling under the user's existing software.
3. Choose a scene, apply it and optionally enable startup. Closing the editor keeps the lighting session running; Exit releases owned resources.
4. If access is unavailable, keep independent devices running, identify the affected resource as far as evidence permits and offer a relevant action. Do not claim another application's name without evidence and do not stop it automatically.

Some hardware requires an additional driver or administrative setup. The installer must explain that dependency before requesting the normal Windows approval. A standard-user installation can skip unavailable elevated paths and report their limitations. Do not bypass Windows security settings or silently install a driver.

## Ship a known component with the application

Recommended first distribution: include the tested OpenRGB component in the Prismestra installer/portable package. Basic PC lighting must not require an unreviewed executable download at first run. Official OpenRGB already offers Windows portable packages and an SDK for third-party clients. [Official releases](https://openrgb.org/releases.html), [SDK](https://openrgb.org/sdk.html).

Use a private, versioned component directory such as `components/openrgb/<component-version>/`, with separate Prismestra-owned configuration/state. This is a proposed package layout. It must not modify the user's separately installed OpenRGB, its profiles or startup entries. An existing external SDK server can be an explicit advanced option; a foreign occupied endpoint must never be adopted silently.

Prefer an unmodified upstream release when it meets acceptance. If a confirmed gap needs a patch, retain a small reviewed patch set against a pinned upstream revision, publish its changes and build instructions, and seek upstream integration. Do not create a new unrelated RGB application or claim that a third-party SDK client eliminates the underlying driver dependency.

Distribute applicable licenses, notices and corresponding component source/build material with the release. OpenRGB's license includes source-distribution requirements; review the complete selected component/dependency set before shipping. [OpenRGB license](https://github.com/CalcProgrammer1/OpenRGB/blob/master/LICENSE). Do not assume publicly visible source grants unrestricted reuse.

## Trust and updates

The About/Components view and release manifest should identify the original project, exact revision, original or patched status, patch/source/build links and binary digests. A modified component must be clearly attributed rather than presented as an official upstream binary. CI must build the reviewed source and verify shipped files. Byte-for-byte reproducible builds and Windows signing are separate acceptance items; neither is currently claimed.

Update Prismestra and its tested components as a versioned set, retaining the previous version until the new version passes startup checks. Define removal of owned components and an explicit choice about preserving user data. No paid signing certificate purchase is authorized by this document.

## FanControl and other applications

FanControl can remain responsible for fan speed. Prismestra does not require switching cooling products. This is an acceptance requirement, not a currently proven guarantee for every shared controller. Lighting and monitoring can still share buses or interfaces even when they perform different functions. If a device cannot expose those functions concurrently, report that exact limitation rather than stopping the entire application.

Separate installed software from active ownership of the same resource. Several programs may be installed; multiple active lighting writers may override each other. Use a supported vendor sharing API when available. Otherwise require an explicit per-device ownership choice while preserving unrelated functionality. SignalRGB itself documents conflicts and commonly recommends closing competing software; that is evidence of the industry's limitation, not proof of universal coexistence. [SignalRGB conflict documentation](https://docs.signalrgb.com/troubleshooting/conflicting-programs/).

## What reuse can and cannot provide

Keep OpenRGB as the current broad PC adapter, and retain direct BLE/LCD/serial/network adapters where implemented. Evaluate additional vendor adapters per measured gap. RGB.NET is a provider library, not a standalone hardware server; its providers can have vendor SDK/service dependencies. Artemis/Aurora are useful integration references, but their licenses and dependencies must be checked independently. [RGB.NET](https://github.com/DarthAffe/RGB.NET), [Artemis license](https://github.com/Artemis-RGB/Artemis/blob/master/LICENSE), [Aurora OpenRGB setup](https://project-aurora.com/Docs/devices/openrgb/).

More direct candidates already exist: Nexus provides a [Qt-free OpenRGB component](https://github.com/hello-nexus/openrgb-headless/blob/6bf7d9f5348be41dcf6a45dc7ad57f00b0b69b43/MAINTAINING.md), MyRGB has a [pinned official-OpenRGB bundler](https://github.com/kylechrisking/MyRGB/blob/main/tools/build-installer.ps1) and [verified dependency/elevated-component setup](https://github.com/kylechrisking/MyRGB/blob/main/tools/setup-openrgb-admin.ps1), and TuxRGB has a [Rust application's component installer](https://github.com/aurelienchartier29-netizen/TuxRGB/blob/main/packaging/windows/tuxrgb.nsi). Inspect these before building equivalents. None has been adopted or physically validated here. Their licenses differ; copying code requires an explicit license-compatible integration decision. The Nexus component still waits indefinitely on the relevant SMBus mutex, so a ready headless build does not by itself close our coexistence gap.

## Release gates for this path

- Fresh Windows without OpenRGB: a single supported installation and launch path, with no manual SDK/port setup for basic use.
- Existing FanControl: no automatic foreign-process changes, preserved cooling settings, isolated occupied resources and verified supported lighting paths.
- Shared resource held/released by a second process: bounded waits, independent-device progress, eventual recovery after release and no release of an unowned mutex.
- Discovery held beyond output retry exhaustion, editor closed, backend loss/recovery: automated coverage plus native evidence where hardware is available.
- Checked component provenance, redistribution material, versioned update/rollback and clean removal. Signing and clean-machine evidence stay explicitly unverified until performed.

The current late-inventory fix covers only one gate. Safe isolation/deferred discovery in the hardware backend and the bundled installer remain work to do.
