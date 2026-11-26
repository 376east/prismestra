# Compatibility

Prismestra supports specific device families and connection modes. Check your model, firmware and connection before relying on an output.

| Device family | How to connect | Verification scope |
| --- | --- | --- |
| PC RGB via OpenRGB | Setup includes an SDK6 backend. First run searches for Direct-capable controllers. Configure the controller manually in **Devices** if needed. | Inventory and frame submissions checked on one kit at `6839f196`. Coverage depends on the controller and backend. |
| Surplife E1 Bluetooth | First run searches for compatible controllers. You can also select the BLE address manually. | A 13 November 2026 research probe lit one LED on one 153-point strip. Native capability checks and other models need separate acceptance. [Probe record](validation/surplife-153.json) |
| Thermaltake LCD | First run searches for supported USB interfaces. Check the saved device connection if Windows can see the LCD but the app cannot. | JPEG acknowledgments and temperature reads checked on one TT 264A:233C at `6839f196`. |
| Skydimo RGB | Use a configured connection for a supported model. | Frame submissions recorded for one configured output at `6839f196`. See the [local kit record](validation/windows-local-kit.json). |
| WLED | Find an advertised controller on the LAN or enter its DDP endpoint. DDP sends per-pixel animation. | Software receiver tests. Physical output still needs acceptance. |
| Home Assistant | Find the server on the LAN or enter its URL, save a token and select a light for whole-light commands. | Software and HTTP fixture tests. Physical output still needs acceptance. |
| Alice pilot | Sign in with Yandex ID and select a supported lamp for explicit controls or optional scene brightness. | Physical lamp output remains unverified for 0.2.0. [Alice scene binding](https://github.com/376east/prismestra/pull/221) |

If a device needs manual setup, choose its family in **Devices** and open its connection settings. LED layouts may need manual configuration. [First-run setup](windows-installer.md) · [Device connections](universal-connections.md)

Keep FanControl running for fans and pumps. Assign one lighting writer to each device. If a WLED controller also appears in Home Assistant, choose one route and check whether other automations control it. [Choose one WLED write path](universal-connections.md#choose-one-write-path-for-a-wled-controller)

## Device paths and evidence

The original connection review was on 13 November 2026. Each hardware observation below applies to its recorded build or research probe. See [release status](status.md) for 0.2.0 installer checks and [reusable connections](universal-connections.md) for discovery, capability checks and recovery.

| Path | Implemented scope | Software evidence | Hardware observations |
| --- | --- | --- | --- |
| OpenRGB SDK6 | Explicit server, read-only discovery, complete-controller geometry, stable local binding, explicit Apply and direct colors | [Adapter](../src/transport/openrgb.rs), [packet tests](../src/transport/openrgb_tests.rs), [integration tests](../tests/openrgb_pipeline.rs), [patches](../vendor/openrgb2/LOCAL-CHANGES.md) | Inventory and frame submissions checked on one [local kit](validation/windows-local-kit.json) on 9 November 2026 at `6839f196`. Visible colors were outside that check. |
| WLED DDP | LAN discovery or explicit endpoint, reported LED count and saved identity, paced RGB8 output | [Metadata](../src/transport/wled_info.rs), [transport](../src/transport/wled.rs), [integration tests](../tests/wled_pipeline.rs) | Physical output unverified. UDP submission has no delivery acknowledgment. |
| Home Assistant | LAN server discovery, HTTP(S) light discovery, protected token, capability-aware whole-light commands, at most 1 Hz with dedup | [Transport](../src/transport/home_assistant.rs), [capabilities](../src/devices/home_assistant.rs), [binding tests](../tests/home_assistant_bindings.rs), [worker tests](../src/runtime/network_ha_tests.rs) | Physical output unverified. The response records service submission. |
| Alice pilot | Yandex ID, supported lamp selection, on/off and brightness controls, optional scene brightness | [Editor implementation](../src/ui/windows/alice.rs), [scene commands](../src/ui/windows/alice/scene_apply_slice.rs) | Physical output unverified for 0.2.0 |
| BLE | Legacy 20/153 profiles and Surplife E1. The E1 path validates the model and reported single-segment point count before sending frames. | [Connection limits](universal-connections.md), [protocol](surplife-e1-pixels.md), [BLE transport](../src/transport/ble.rs) | On 13 November 2026, marker 0 lit one physical LED on one 153-point Surplife E1 in a pre-integration probe. Native capability handshake, other counts and other controllers require separate acceptance. [Observation](validation/surplife-153.json) |
| LCD | Thermaltake-family packet path, media composition and temperature overlays | [LCD module](../src/lcd), [protocol provenance](../src/lcd/fixtures/protocol-provenance.md) | JPEG acknowledgments and MI00 temperature checked on one [TT 264A:233C](validation/windows-local-kit.json) on 9 November 2026 at `6839f196`. Visible pixels and overlay appearance were outside that check. |
| Selected native RGB | Skydimo, Fusion2, ENE and Valkyrie protocol implementations | [Transports and fixtures](../src/transport), [native adapters](../src/runtime/native.rs) | Compatibility is model- and firmware-specific. Per-device acceptance is pending. |
| Windows Dynamic Lighting / LampArray | Planned | Direct adapter pending | Unsupported here |

On 9 November 2026, the packaged Windows preview at `76e91606b992` completed a 327-second full-kit run covering editor close and reopen, all seven output counters, and normal tray exit with restoration. The [revision-specific application record](validation/windows-preview1.json) includes its resource sample and limits. Visible colors and LCD pixels were outside that run.

## What counts as support

Record the exact model, hardware revision, firmware, OS, connection mode and backend. For each operation, distinguish:

1. Implemented protocol and parser behavior.
2. Tests with fixtures or a simulated server.
3. An accessible setup and activation path in the application.
4. A physical device check, with observable results and limitations.

Successful compilation does not complete the other stages. A report should say whether discovery, static color, animated frames, stopping, reconnecting and returning ownership were tested. Do not infer untested operations from one successful color change.

## Output boundaries

- Prismestra can open with an empty map or any combination of supported outputs. First run adds detected compatible devices and skips BLE scanning when Windows reports no Bluetooth radio. Saved bindings remain when devices become unavailable. Home Assistant and WLED use configured endpoints. Local USB, PC and Bluetooth outputs can work without Wi-Fi or internet access. [First-run tests](../src/app/first_run.rs) · [Windows radio detection](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/guid-bthport-device-interface)
- The OpenRGB adapter requires SDK6. Inventory does not initialize hardware. Explicit activation can select a direct mode. Limits and topology handling are documented in its [source](../src/transport/openrgb.rs) and [patch record](../vendor/openrgb2/LOCAL-CHANGES.md).
- Virtual controllers are excluded. The same physical identity cannot be activated through multiple SDK addresses. Identical ambiguous metadata on separate servers is conservatively rejected until the physical namespace can be distinguished. A full controller needs a complete point map; synthetic fallback positions are not physical calibration.
- WLED can discover advertised LAN controllers or query an explicit endpoint, read configured LED count and save identity. Verified bindings recheck identity/count before DDP activation; legacy explicit DDP-only bindings remain supported. There is no remote confirmation of visible pixels.
- Home Assistant sends slower commands to whole lights. Tokens belong in local credential storage, outside portable scenes.
- Home Assistant keys are shared by lights on the same canonical server endpoint and stored in [Windows Credential Manager](../src/credentials_windows.rs). Removing one light preserves the server key. After changing a key, Apply creates a new worker and reads it again. Missing credentials or server rejection can be reported after previous outputs have drained; Return is an explicit recovery action.
- After a light has been bound, a temporary REST failure keeps the worker in Connecting while it retries. It checks the same entity's state and capabilities before submitting the latest scene color. Retry delays are 1, 5, 15, then 30 seconds, with longer waits when HA supplies `Retry-After`. Invalid credentials and other permanent errors use the normal failure path. A successful POST still confirms service submission only. [HA worker](../src/runtime/network.rs)
- Read-only temperatures do not grant fan or pump control. The application does not replace Fan Control.

Device reports are welcome through the [device report form](https://github.com/376east/prismestra/issues/new?template=device.yml). For a Surplife E1 strip, see the [BLE acceptance steps and report fields](ble-device-acceptance.md).
