# Reusable device connections

Prismestra reuses backend capabilities instead of treating every physical setup as a new driver. This document describes the implementation, not a claim of universal hardware acceptance.

## Connect a device

Add the appropriate family in Devices, then open Connections for that logical device. Discovery does not activate RGB output. Select a result, save its local binding, then Apply. Applying a discovered strip length updates its canvas geometry in the same runtime transaction. Existing same-sized layouts remain intact. Discovery does not guess the physical arrangement of a strip.

- **OpenRGB:** leave the SDK endpoint empty to use `127.0.0.1:6742`, or supply another explicit server. Find reads the inventory and complete controller geometry. Apply revalidates identity and topology before enabling Direct output. This covers devices supported by the selected OpenRGB backend; it does not implement missing OpenRGB drivers.
- **WLED:** leave the DDP endpoint empty and Find performs a short LAN DNS-SD browse. Alternatively enter an explicit `IP:4048`. Find reads `/json/info` to obtain the configured point count and MAC-based identity. The optional HTTP origin field supports non-default HTTP ports; its IP must match the DDP destination. A verified route checks the saved MAC and LED count before activation and before starting another frame once its two-second validation interval expires. A failed check stops that output. Recovery checks the same saved binding again before sending. IP reuse can go undetected between checks. Legacy explicit DDP-only bindings still load without metadata verification. UDP submission is not confirmation of visible output.
- **Home Assistant:** an empty endpoint finds advertised servers without sending a token. Select a server, correct the URL for HTTPS/reverse proxies if needed, save its key in Windows Credential Manager, then Find again to list lights. Choose one light and save its binding. Initial binding rejects unavailable entities and unknown capabilities. Color-capable lights receive color; brightness/color-temperature-only lights receive brightness; on/off lights receive switching. This path controls one whole light at up to 1 Hz, not individual LEDs.
- **Surplife E1:** the `surplife-e1` family supports an explicitly selected BLE address. Find issues a fixed read-only EA81 capability query, with no configuration or firmware command. Currently only product `0xAA`, LED protocol `11`, one segment and 1..4096 configured points are accepted. Select the result and Apply to adopt the point count. Before output the connection queries capacity again and rejects a mismatch. The 4096 limit bounds host resources; it is not a claim that every controller supports that many physical LEDs. Existing `door_ble` 20-group and 153-point profiles retain their old behavior and are not silently migrated.

A Surplife capability query takes the same in-process endpoint lease as RGB output. An already owned controller must be detached before its discovery query; a rescan never opens a second connection to a running output. External programs still need to relinquish hardware ownership.

## Choose one write path for a WLED controller

One WLED controller can appear as a direct DDP output and as one or more `light.*` entities in Home Assistant. The HA entity lease and the DDP controller lease are independent. Compare the configured WLED host and the MAC reported by `/json/info` with the physical device in HA. Include any segment entities and the main light for that controller. [Home Assistant WLED integration](https://www.home-assistant.io/integrations/wled/) · [Prismestra binding keys](../src/devices.rs)

In **Devices and properties**, enable the route you want and disable the other outputs for that controller, then apply the configuration. Choose DDP for per-pixel animation or an HA light for slower whole-light control. To check the selection on a test controller, change the scene. A DDP-only selection sends frames to the configured UDP port (normally `4048`) and issues no HA light commands for that controller. An HA-only selection issues light commands for the selected entity and sends no DDP frames to the controller. HA automations can still write to WLED independently. [WLED DDP](https://kno.wled.ge/interfaces/ddp/) · [Output enable setting](../src/ui/editor.rs)

## Alice pilot

The WebView2 editor has a foreground Alice pilot under **Settings → Office lamp**. On the first visit, sign in with Yandex ID and confirm catalog and lamp access in the system browser. **Remember sign-in on this PC** stores the access token and expiry in Windows Credential Manager. Opening the dialog with a valid saved sign-in loads the catalog automatically. Sign in again when access expires or is revoked. **Sign out of Alice** clears the local grant. [Yandex OAuth](https://yandex.ru/dev/id/doc/ru/codes/code-url) · [Token revocation](https://yandex.ru/dev/id/doc/ru/tokens/token-invalidate)

Select one lamp from the room named «Кабинет». Then turn it on or off, or apply a brightness to an already lit lamp. Opening the catalog and selecting a lamp only read data. The selection stays in place between commands. Every explicit command rechecks the selected lamp and is sent once. A lost response requires checking the lamp before issuing another command. [Yandex Smart Home API](https://yandex.ru/dev/dialogs/smart-home/doc/ru/concepts/platform-protocol)

## Discovery boundaries

DNS-SD browses `_wled._tcp.local.` and `_home-assistant._tcp.local.` for at most three seconds, with bounded result/event counts and explicit shutdown. It does not scan IP ranges. Advertisements are untrusted hints; TXT URLs are not followed and credentials are never supplied during discovery. IPv6 link-local addresses requiring an interface scope are currently omitted. mDNS can be disabled or blocked between network segments; manual endpoints remain available.

WLED metadata supports upstream default counts through 16384; larger custom builds require a separately verified extension. A changed IP can be rediscovered using the controller's identity. Rediscovery/Apply is currently required for address changes; no claim of automatic roaming across DHCP addresses is made.

## Recover a device connection

Native activation enables bounded recovery for registered RGB and LCD outputs, including an initial connection failure before the first frame. A failed and fully reaped output can reopen after 1, 5 and 15 seconds, at most three attempts per configuration/power epoch. A successful retry does not replenish this budget. Configuration changes, shutdown and suspend cancel pending attempts; capture and temperature readers do not inherit this policy.

Recovery revalidates saved identity/capacity and republishes current scene intent, including Off, instead of replaying an old frame. It leaves healthy outputs running. Home Assistant's missing or rejected credentials require user action. A verified WLED route with a mismatched MAC or LED count can only resume automatically if the saved binding validates again within its retry budget. To bind a replacement controller or new geometry, run discovery and Apply. A synchronous start/lease error may remain in global diagnostics after a later successful recovery.

Device properties show submitted-frame counts, including on failure. Surplife errors identify capability-query stages separately from E104 frame submission. A transient EA81 timeout was observed on the current controller before any color frame; later read-only queries succeeded. Initial-failure recovery handles that failure path, while the cause of the underlying timeout remains unconfirmed.

## Evidence

Implementations: [discovery](../src/app/lighting_bindings/discovery.rs), [WLED metadata](../src/transport/wled_info.rs), [DNS-SD](../src/transport/service_discovery.rs), [HA capabilities](../src/devices/home_assistant.rs), [E1 codec](../src/lighting/door.rs), [BLE transport](../src/transport/ble.rs), [recovery](../src/app/runtime/recovery.rs).

Software checks include real Windows HTTP/UDP loopback fixtures, identity/count rejection before UDP output, capability conversion, protocol fragments and fake-driver recovery/lease scenarios. Hardware evidence for the previous 153-point implementation remains [revision-specific](surplife-e1-pixels.md). WLED/HA physical output, a second BLE controller/different physical length and real unplug/reconnect are not confirmed by these software checks.

Hardware coverage is recorded separately from software delivery. Missing test hardware does not block development or make a software fixture count as physical acceptance.

| Path | Software evidence | Physical coverage |
| --- | --- | --- |
| OpenRGB SDK6 | Inventory, identity/topology checks and native output pipeline | Current PC kit only; device support also depends on OpenRGB |
| WLED | [Discovery, saved metadata, geometry, native HTTP/UDP output, MAC ownership and replacement rejection](../tests/wled_pipeline.rs) | Unverified |
| Home Assistant | [Binding and capability tests](../tests/home_assistant_bindings.rs), HTTP fixtures and command mapping | Unverified |
| Surplife E1 | Codec, capacity validation and [notification read-error propagation](../src/transport/ble_windows.rs) | Current 153-point controller only; other controllers/lengths unverified |
| LCD | Worker, lifecycle and output tests | Current Thermaltake screen only |

Loopback HTTP/UDP tests exercise the real software transport against local fixtures. Notification tests inject a read failure into the callback's shared-result path; they do not reproduce a real WinRT or radio failure. Physical unplug/replug remains unverified for all paths.
