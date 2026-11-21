# Local inputs for RGB scenes

Select an effect in the editor and apply it in a connected Windows profile. The input starts when an enabled Canvas output is assigned to an active lighting group. Selecting a draft does not start an input. An effect without fresh data goes dark.

## Keyboard and camera

Keyboard effects use Windows Raw Input. The approximate full-size ANSI map includes arrow keys. Prismestra retains only recent positions and timestamps in memory to draw brief ripples, heat, bolts and columns. It does not record typed text.

Camera effects use the first video device enumerated by Windows. They estimate either the image color or the position, strength and color of recent movement. Prismestra discards full frames after local analysis. Camera access is controlled by Windows. [Camera settings](https://learn.microsoft.com/en-us/windows/apps/develop/camera/camera-privacy-setting)

## Counter-Strike 2 and Dota 2

1. Select an effect in the game group, such as Game resources, and choose **Save CS2 configuration** or **Save Dota 2 configuration** in the editor.
2. Save `gamestate_integration_prismestra.cfg` under `game/csgo/cfg` for CS2 or `game/dota/cfg/gamestate_integration` for Dota 2. Steam's **Browse local files** opens the installation directory.
3. Apply the scene, enable a Canvas RGB group and start the game. If telemetry is disabled, add `-gamestateintegration` in the game's Steam launch options.

The adapters consume Valve Game State Integration data through the local receiver. The CS2 adapter derives health, the active weapon's magazine, round phase, objective status, shots, damage and eliminations. The Dota 2 adapter reads local hero health and mana, match state, damage, eliminations and the start of an ability cooldown. Each game export carries a credential scoped to that game route. The app does not open game processes or modify their files automatically.

## Other local producers

Select **Save endpoint and key** in a game or external-data effect. The saved JSON contains local endpoints and an Authorization value scoped to Prismestra's receiver. Keep this file and the CS2 configuration private. The receiver listens on IPv4 loopback while a game or external-data effect needs it. Requests must be POST with an explicit length and `Content-Type: application/json`. Send the exported Authorization value as the header for the two generic routes.

For `/game`, provide a full current snapshot. Optional `health` and `resource` are fractions. `phase` is one of `menu`, `ready`, `live`, `won`, `lost`. Optional `hazard` is a boolean. Send `event` only when it occurs.

```json
{"phase":"live","health":0.64,"resource":0.37,"hazard":false,"event":"damage"}
```

Events are `damage`, `elimination`, `shot`, `ability`, `objective`, `round_start`, `victory` and `defeat`. Other titles need a producer that supplies this schema.

For `/external`, each POST supplies one signal. Choose the matching effect for the signal that the producer sends.

| Effect | Example JSON body |
|---|---|
| Metric ring | `{"kind":"metric","value":0.65}` |
| Alert | `{"kind":"alert","severity":0.8}` |
| Weather | `{"kind":"weather","condition":"rain","strength":0.6}` |
| Media ambience | `{"kind":"media","color":[90,140,230],"progress":0.35,"playing":true}` |
| Browser pulse | `{"kind":"browser","event":"download_complete"}` |

Weather conditions are `clear`, `cloud`, `rain`, `snow`, `storm` and `fog`. Browser events are `navigate`, `new_tab` and `download_complete`. Scalar fields range from 0 to 1. Paths, URLs, song names and credentials from the producer do not belong in these payloads.

Media ambience can also follow the current Windows media session if it publishes an artwork thumbnail and a timeline. Export the endpoint and key to enable a local media producer. A fresh explicit media payload takes priority. The app extracts an accent from the thumbnail in memory and keeps only the color, playback progress and playing state.
