# Reusable device connections

Prismestra reuses backend capabilities instead of treating every physical setup as a new driver. This document describes the implementation, not a claim of universal hardware acceptance.

## Connect a device

Add the appropriate family in Devices, then open Connections for that logical device. Discovery does not activate RGB output. Select a result, save its local binding, then Apply. Applying a discovered strip length updates its canvas geometry in the same runtime transaction. Existing same-sized layouts remain intact. Discovery does not guess the physical arrangement of a strip.

- **OpenRGB:** in an installed session, leave the SDK endpoint empty to use Prismestra’s managed backend at `127.0.0.1:17422`. To use a separate OpenRGB server with SDK 6, enter its address explicitly, such as `127.0.0.1:6742` for a local server with the standard port. Find reads the inventory and controller geometry. Apply revalidates identity and topology before enabling Direct output. Live scenes require a usable Direct mode for that controller.
- **Surplife E1:** the `surplife-e1` family supports an explicitly selected BLE address. Find issues a fixed read-only EA81 capability query, with no configuration or firmware command. Currently only product `0xAA`, LED protocol `11`, one segment and 1..4096 configured points are accepted. Select the result and Apply to adopt the point count. Before output the connection queries capacity again and rejects a mismatch. The 4096 limit bounds host resources; it is not a claim that every controller supports that many physical LEDs. Existing `door_ble` 20-group and 153-point profiles retain their old behavior and are not silently migrated.

A Surplife capability query takes the same in-process endpoint lease as RGB output. An already owned controller must be detached before its discovery query; a rescan never opens a second connection to a running output. External programs still need to relinquish hardware ownership.

### Connect Govee through OpenRGB

1. In Govee Home, enable **LAN Control** for the light and find its local IP address.
2. Use a separate OpenRGB server with SDK 6 and the Govee LAN detector. In that server’s settings, add the light’s address under `GoveeDevices.devices`. The detector expects entries such as `{"ip":"192.0.2.10"}`. If you edit `OpenRGB.json`, merge this entry into the server’s existing configuration and restart that server. Confirm it lists the light with a usable Direct mode.
3. In Prismestra, add an **OpenRGB** device. In **Connections**, enter the separate server’s SDK address, for example `127.0.0.1:6742`. Choose **Find devices**, select the light, save its binding and choose **Apply**.

The OpenRGB instance started by Prismestra has no Govee IP setup in the app. Available effects depend on the controller. [Govee LAN setup](https://app-h5.govee.com/user-manual/wlan-guide) · [OpenRGB detector](https://github.com/hello-nexus/openrgb-headless/blob/6bf7d9f5348be41dcf6a45dc7ad57f00b0b69b43/Controllers/GoveeController/GoveeControllerDetect.cpp#L22-L65)

### Connect WLED

1. Connect the controller to your local network and set the LED count in WLED. The controller must accept DDP.
2. In Prismestra, add a **WLED** device and open **Connections**. Leave the DDP endpoint empty to search the local network, or enter `IP:4048`. Choose **Find devices**.
3. Select the controller, save its binding and choose **Apply**.

Find reads `/json/info` for the controller identity and LED count. You can set an optional HTTP origin with a custom port. Its IP must match the DDP destination. Verified bindings check the saved MAC and LED count before activation and again before sending a frame after the two-second validation interval. If a check fails, Prismestra stops the output and rechecks the saved binding before resuming. IP reuse can go undetected between checks. Legacy DDP-only bindings still load without metadata verification. UDP submission is not confirmation of visible output. [WLED DDP](https://kno.wled.ge/interfaces/ddp/)

### Connect a Home Assistant light

1. In Prismestra, add a **Home Assistant** device and open **Connections**. Choose **Find devices** with the endpoint empty to discover servers without sending a token, or enter your server URL.
2. Select the server, correct its URL if you use HTTPS or a reverse proxy, and choose **Save key** to store its access token in Windows Credential Manager. Choose **Find devices** again to list lights.
3. Choose a light, save its binding and choose **Apply**.

Initial binding rejects unavailable lights and unknown capabilities. Color-capable lights receive color, brightness and color-temperature lights receive brightness, and on/off lights receive switching. Updates affect the whole light at up to once a second.

### Connect Philips Hue through Home Assistant

1. Add your Hue Bridge with the [Philips Hue integration in Home Assistant](https://www.home-assistant.io/integrations/hue/).
2. Follow [Connect a Home Assistant light](#connect-a-home-assistant-light) and choose the Hue light in Prismestra.

Prismestra controls the whole light through Home Assistant. For Hue, this route does not animate individual LEDs.

## Choose one write path for a WLED controller

One WLED controller can appear as a direct DDP output and as one or more `light.*` entities in Home Assistant. The HA entity lease and the DDP controller lease are independent. Compare the configured WLED host and the MAC reported by `/json/info` with the physical device in HA. Include any segment entities and the main light for that controller. [Home Assistant WLED integration](https://www.home-assistant.io/integrations/wled/) · [Prismestra binding keys](../src/devices.rs)

In **Devices and properties**, enable the route you want and disable the other outputs for that controller, then apply the configuration. Choose DDP for per-pixel animation or an HA light for slower whole-light control. To check the selection on a test controller, change the scene. A DDP-only selection sends frames to the configured UDP port (normally `4048`) and issues no HA light commands for that controller. An HA-only selection issues light commands for the selected entity and sends no DDP frames to the controller. HA automations can still write to WLED independently. [WLED DDP](https://kno.wled.ge/interfaces/ddp/) · [Output enable setting](../src/ui/editor.rs)

## Alice pilot

The WebView2 editor has a foreground Alice pilot under **Settings → Lamp controls**. On the first visit, sign in with Yandex ID and confirm catalog and lamp access in the system browser. **Remember sign-in on this PC** stores the access token and expiry in Windows Credential Manager. Opening the dialog with a valid saved sign-in loads the catalog automatically. Sign in again when access expires or is revoked. **Sign out of Alice** clears the local grant. [Yandex OAuth](https://yandex.ru/dev/id/doc/ru/codes/code-url) · [Token revocation](https://yandex.ru/dev/id/doc/ru/tokens/token-invalidate)

Select a supported lamp from any available room. The catalog displays the home, room and lamp names so that lamps with the same name can be told apart. Then turn the selected lamp on or off, or apply brightness to an already lit lamp. Opening the catalog and selecting a lamp only read data. The selection stays in place between commands. Every explicit command rechecks the selected lamp's ID, home and room and is sent once. A lost response requires checking the lamp before issuing another command. [Yandex Smart Home API](https://yandex.ru/dev/dialogs/smart-home/doc/ru/concepts/platform-protocol)

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
