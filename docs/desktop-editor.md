# Desktop editor

The interface is compiled into Prismestra; it does not start a local HTTP server or load a website. WebView2 renders the window, while the existing Rust session owns scenes, device transports and sensor subscriptions. Source: [embedded host](../src/ui/windows/web_shell.rs), [command bridge](../src/ui/windows/modern.rs), [native form adapter](../src/ui/windows/native_forms.rs).

## Scenes and profiles

A scene in the library contains RGB effect parameters. Editing and previewing it do not change the running scene. Apply explicitly updates the Rust scene. The preview and library thumbnails use the same native Renderer as the engine; their regular sample grid is not a physical device layout. Source: [scene renderer](../src/runtime/scene.rs), [catalog](../src/ui/catalog.rs).

Device topology, output groups, LCD media, screen capture and temperature routes belong to the profile. Open, Save and Export use the existing profile and media handling. Local hardware bindings and credentials remain separate from portable profiles. Applying a connection is an explicit hardware operation; saving its form does not activate it. Source: [profiles](../src/app/document.rs), [connections](../src/ui/windows/connections.rs).

**Scene File** opens a separate composition preview in the library or scene editor. It starts with a copy of the current draft effect. You can add color layers, set layer opacity, choose a region for one logical LCD and choose the ordered edges for one logical strip. **Open File** restores the composition from a scene JSON file. **Save File** writes the composition and the logical assignments. The preview draws the LCD and strip from the same composed frame. Use the existing library for named RGB scenes and the profile for physical output. Source: [scene files](../src/app/scene_file.rs), [preview](../src/app/scene_file/preview.rs).

In **Screen**, choose **Scene edges** as the strip source to follow the active RGB scene. LEDs sample the perimeter clockwise from the top left. The existing reverse setting flips the order. The profile keeps the selected source and retains the monitor settings for switching back. A strip using scene edges does not need a local monitor binding. [Routing and geometry](../src/app/ambient.rs)

Closing the editor first requests the latest RGB draft and waits for its atomic save. An error or timeout cancels closing instead of discarding changes. Unsaved device, connection and source forms still require Apply or Revert. The active scene keeps running in the tray. [Close handshake](../src/ui/windows/modern.rs)

**Turn lights off** in the editor requests darkness for the active lighting groups while the app stays open. **Stop outputs** in the tray releases the devices and keeps Prismestra in the tray. **Exit** releases the devices and closes Prismestra. Some controllers hold their last color after they are released. [Lighting state](../src/app/runtime.rs) · [Tray actions](../src/ui/windows/resident.rs)

## ARGB channels

Open **ARGB channels** from the properties of a device bound to managed OpenRGB. The editor reads the selected controller's channel sizes and limits from SDK6 without activating any output. A fixed or automatic zone shows its current size. For a manually sized zone, choose a fan or strip template, or enter the exact number of addressable LEDs for the channel. The ring or strip is a preview of the address count. A passive splitter repeats the same addresses on every branch, so use the longest branch's address count.

The starter counts are based on published specifications: 8 and 12 for fans, and 10, 30 and 60 for strips. Match the count to the actual model and strip length. [Corsair SP RGB Elite](https://www.corsair.com/newsroom/press-release/your-guiding-light-for-great-cooling-corsair-launches-sp-rgb-elite-fan-series) · [ARCTIC P12 PWM PST A-RGB](https://www.arctic.de/en/P12-PWM-PST/) · [Corsair Lighting Node PRO guide](https://assets.corsair.com/image/upload/corsairmedia/sys_master/productcontent/WW_LightingNodePRO_QSG_revAD_.pdf) · [Adafruit RGB NeoPixel strips](https://learn.adafruit.com/adafruit-neopixel-uberguide/neopixel-strips)

The editor currently offers inventory and preview. Saving a new channel size requires the protected OpenRGB update path. [Managed OpenRGB](managed-openrgb.md)

## Runtime and recovery

The new editor requires Windows x64 and Microsoft Edge WebView2 Runtime 105 or newer. Microsoft describes runtime installation and servicing in its [distribution guide](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution). All editor resources are local. Navigation, resource requests, downloads, new windows and permissions are restricted by the host; the browser frontend cannot issue network requests through its page policy. Device backends retain their own explicit network connections.

Use `Prismestra.exe --classic-editor` to open the former Win32 surface. If the new host fails to initialize, its error is shown on that surface. A normal launch requires access to the user's taskbar/tray; the restricted test sandbox does not provide it.

Settings directories now also contain `scene-library.json`, `editor-scene-draft.json` and a `webview-editor` browser data directory. The first two are validated application data, not browser project storage. Invalid existing catalog/draft files are preserved; initialization reports their error. Do not publish a personal settings directory.

## Verification boundaries

For a configured local profile, `Prismestra.exe --project profile.json --state-dir state --start-connected` opens the saved hardware outputs and starts the exact saved temperature routes at startup. The connection file is `state/lighting-local.json`; assets and sensor routes remain in the same state directory as the editor. Missing or incompatible bindings produce an error rather than selecting another device or inventing temperatures. No discovery runs automatically. The flag requires an explicit profile and is forbidden in snapshots and selftests. Without the flag, startup remains a software preview. Source: [startup options](../src/ui/windows.rs), [saved source subscriptions](../src/ui/windows/sources/bindings.rs).

The source has Rust, frontend-IPC and packaging checks. New UI and hardware acceptance must identify the exact tested executable revision. Existing Preview3 device records do not prove a new WebView2 build's physical behavior, interaction, clean-machine readiness or resource use. Source review of controller teardown is not a measurement that browser processes have exited.
