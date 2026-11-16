# Prismestra product scope

Prismestra coordinates PC RGB, ambient lighting, selected BLE devices, LCDs and compatible home lights in one local scene. The Rust canvas/effects/capture/LCD components and OpenRGB SDK adapter are implemented. [Current implementation](README.md), [verified status and remaining gates](docs/status.md).

## Boundaries

- No fan curves, PWM writes, cooling guardians, cooling recovery or replacement of FanControl. Read-only temperature sources remain in scope.
- Device discovery, binding and activation are separate. Loading a portable profile never authorizes native I/O.
- Portable scenes/assets and local hardware preferences remain separate.
- Each endpoint has one owner; cancellation must drain pending I/O before releasing its resources.
- Windows is the current platform. Other hardware and firmware are supported only when explicitly implemented and tested.

## User experience

The editor explains what is connected, which scene is active, what is unavailable and how to add or replace a device. Its WebView2 interface can close while the Rust runtime continues in the resident session. This lifecycle is implemented; a new real reboot/logon is a separate acceptance gate. The previous native editor remains available. No permanent Chromium/Electron engine is required by the product.

Schedules target lighting groups. LCD overlays use logical temperature names, including aggregate RAM and Liquid. Compatibility should distinguish implemented protocol, software verification and physical acceptance.

## Acceptance

A release requires a reproducible build, simple first connection, real RGB/LCD output through the ordinary user flow, profile/assets migration checks, explicit compatibility limits and CPU/RAM measurements against a comparable scene. Bit-for-bit reproducibility is not currently established, so the source preview does not claim that gate passed. Passing isolated tests alone is insufficient. [Implementation and remaining gates](docs/status.md).
