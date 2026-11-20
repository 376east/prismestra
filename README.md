<p align="center">
  <img src="docs/assets/prismestra-banner.svg" alt="Prismestra. Different devices. One scene." width="1200">
</p>

<p align="center">
  <a href="https://github.com/malyarq/prismestra/actions/workflows/windows.yml"><img src="https://github.com/malyarq/prismestra/actions/workflows/windows.yml/badge.svg" alt="Windows checks"></a>
  <img src="https://img.shields.io/badge/platform-Windows-89b4fa?style=flat-square" alt="Windows">
  <img src="https://img.shields.io/badge/core-Rust-f9bc7a?style=flat-square" alt="Rust core">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--2.0--or--later-a6e3a1?style=flat-square" alt="GPL-2.0-or-later"></a>
</p>

<p align="center">
  <strong><a href="#download-for-windows">Download for Windows</a></strong> ·
  <a href="docs/compatibility.md">Supported devices</a> ·
  <a href="CHANGELOG.md">Changelog</a> ·
  <a href="#build-and-run">Build from source</a>
</p>

Prismestra brings PC RGB, Bluetooth lights, LCD displays and home lighting into a single scene. It runs locally on Windows and keeps the scene running when you close the editor.

An open-source project from [37.6 East](https://github.com/376east).

## Download for Windows

**[Get Prismestra Setup for Windows x64](https://github.com/malyarq/prismestra/releases/download/v0.1.0-preview.6/Prismestra-Setup-0.1.0.6-x64.exe)** · [Preview 6 release notes and known issues](https://github.com/malyarq/prismestra/releases/tag/v0.1.0-preview.6)

Run Setup and choose whether Prismestra should start when you sign in. See the [device compatibility table](docs/compatibility.md) and the release notes for first-run behavior on your setup.

The offline installer includes the application, its private headless OpenRGB backend, Visual C++ and WebView2 installers, and the optional signed PawnIO installer for supported PC lighting. You do not need a separate OpenRGB installation. Updates preserve scenes; uninstall is available in Windows Settings and preserves your data. See [installation and permissions](docs/windows-installer.md) for details and the recorded test scope. Developer source downloads are not installers.

**Early preview.** The scene engine, native editor and OpenRGB/WLED/Home Assistant setup paths are implemented and have software tests. Home Assistant tokens use Windows Credential Manager. A resident session keeps lighting and sensor subscriptions alive when the editor closes. The [local integration record](docs/validation/windows-local-kit.json) covers transport checks on one PC kit and identifies the tested source revision. Physical colors, firmware behavior and performance require separate checks. See the [compatibility table](docs/compatibility.md) before choosing a device.

Start with [verified status](docs/status.md) for what has actually passed, and [repository layout and development commands](docs/development.md) for the source, component builds, packages and local-only artifacts. This repository is the single source of the current product; machine profiles and installed binaries belong outside version control.

The desktop editor now embeds its local interface through WebView2 and sends validated commands to the same Rust session. Scene drafts and the RGB scene library are stored by Rust. Device, LCD, capture, sensor, connection and schedule forms reuse the existing native validators and activation paths. The previous Win32 editor remains available with `--classic-editor`. See [desktop editor behavior](docs/desktop-editor.md).

## What it does

- **One canvas.** Place outputs in a shared scene, combine effects and sample screen content through the [lighting engine](src/lighting) and [desktop capture](src/capture).
- **RGB and LCD together.** Compose images, animation and temperature overlays through the [LCD pipeline](src/lcd), alongside independently paced lighting outputs.
- **Reuse existing support.** Reach an OpenRGB SDK6 server through a [bounded adapter](src/transport/openrgb.rs), send realtime frames to [WLED](src/transport/wled.rs), and submit whole-light colors to [Home Assistant](src/transport/home_assistant.rs).
- **Keep personal setup local.** [Portable scenes](src/app/document.rs) are separate from [local device bindings](src/app/lighting_bindings.rs).
- **Keep control.** Installed startup connects supported local devices automatically; manual network setup and explicit output controls remain available. A successful submission is reported as submission, not proof that a physical light changed.

Prismestra does not control fans or pumps. Keep your existing cooling software. Temperature inputs are read-only.

## Build and run

Packaged Windows x64 builds require Microsoft Edge WebView2 Runtime 105 or newer for the new editor and the x64 Visual C++ v14 runtime. The Setup EXE bundles their offline installers; the older portable ZIPs require a separate runtime installation. See Microsoft's [WebView2 distribution information](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution) and [Visual C++ runtime downloads](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist#latest-supported-redistributable-version). The classic editor does not need WebView2.

Install a current stable [Rust toolchain](https://www.rust-lang.org/tools/install) with the Windows MSVC build tools, then:

```powershell
git clone https://github.com/malyarq/prismestra.git
cd prismestra
cargo build --release --locked --bin pc-control-ui
.\target\release\pc-control-ui.exe
```

The internal crate and executable still use `pc-control` and `pc-control-ui`. The product name is **Prismestra**.

When running from source or an older portable ZIP, first launch opens a preview. Set up and inspect a supported connection before applying it. When using an external backend, configure its server separately; Prismestra does not install or silently take over another controller.

For one-program operation, configure a [managed local OpenRGB backend](docs/managed-openrgb.md) once. Prismestra runs its headless server, verifies ownership/readiness, recovers failures and releases it on exit. Default management uses ordinary permissions; equipment requiring administrator access uses a separately authorized helper.

The current ZIP requires an existing OpenRGB executable. A bundled component and fresh-machine installation are part of the proposed [delivery and coexistence contract](docs/delivery-and-compatibility.md), not shipped capabilities. Research and reuse requirements for future changes are recorded in [project rules](AGENTS.md).

The Windows ZIP includes separate portable launchers: `Open Prismestra.cmd` opens a preview, and `Start saved scene.cmd` activates your saved `data/profile.json` with local connections from `data/state`. See [portable setup and launch](docs/portable-launch.md). Personal profiles, bindings and credentials are not bundled. Running the executable directly retains its existing per-user settings behavior.

Closing the editor leaves the session in the tray. Its menu can reopen the editor, stop outputs or exit. Starting again with the same settings directory shows a tray hint and prevents a duplicate runtime.

## Compatibility is a testable claim

USB, a wireless dongle and Bluetooth on the same keyboard can expose different protocols. A supported vendor name alone is not enough.

We distinguish protocol implementation, software tests, an available setup path and testing on a particular physical device. Fake TCP/HTTP servers and UDP receivers validate packets and failure handling. They cannot validate colors, firmware behavior or radio stability. Device-specific details belong in the [compatibility table](docs/compatibility.md).

The immediate focus is RGB, BLE, LCD and local home lighting. Native Windows Dynamic Lighting/LampArray support and additional protocols remain future work. There is no claim of universal Bluetooth or LCD support.

The [product roadmap](docs/roadmap.md) records the next setup, measurement and distribution stages.

## Development

```powershell
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo test --locked -- --test-threads=1
cargo clippy --locked --all-targets -- -D warnings
```

The [Windows workflow](.github/workflows/windows.yml) checks software behavior without real device activation. Tests use fixtures, temporary files and loopback servers. Device and resource results are revision-specific; passing software CI does not certify physical output or establish a performance advantage.

To report a device or help add one, start with [contributing](CONTRIBUTING.md). A precise model, revision, firmware and connection mode are more useful than a vendor-wide support request.

Maintainers can build a package with its exact source, dependency sources, licenses and checksums using the [release instructions](docs/releasing.md).

## Upstream work and license

| Component | Use | License / source |
| --- | --- | --- |
| OpenRGB | External device backend through its SDK | [OpenRGB](https://github.com/CalcProgrammer1/OpenRGB), GPL-2.0-or-later |
| openrgb2 0.4.1 | Vendored Rust SDK client with bounded parsing and direct-output fixes | [Source and patch record](vendor/openrgb2/LOCAL-CHANGES.md), GPL-2.0 |
| WLED | External realtime lighting receiver over DDP | [WLED](https://github.com/Aircoookie/WLED); no firmware source copied into this adapter |
| Home Assistant | External light entities through its REST API | [REST API](https://developers.home-assistant.io/docs/api/rest/); no server source copied |

Original source is [GPL-2.0-or-later](LICENSE), unless a file states otherwise. Third-party components retain their licenses and notices; combined distributions must satisfy all applicable terms. Protocol fixtures record their provenance alongside the data.

Prismestra is independent of OpenRGB, WLED, Home Assistant and hardware vendors.
