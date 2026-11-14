# Managed local OpenRGB

Prismestra can own a local OpenRGB SDK server without its window or tray icon. Executable, configuration directory and numeric loopback endpoint are machine settings; device models/counts and personal paths are not compiled in. Remote or manually managed servers remain external.

## Configure once

Select an OpenRGB SDK6 executable and an isolated configuration directory. Disable unrelated plugins and detectors for devices driven directly by Prismestra. Exit any server using the intended endpoint. From the extracted Prismestra folder, substitute your paths:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File ".\Configure OpenRGB.ps1" -Executable "C:\Apps\OpenRGB\OpenRGB.exe" -ConfigDirectory "C:\Lighting\OpenRGB-config"
```

Setup pins the EXE SHA256 in `data/state/managed-openrgb.json`. Options: `-StateDirectory`, `-Endpoint` (numeric IPv4/bracketed IPv6) and explicit `-Force` replacement. It never executes a backend, installs drivers, registers tasks or requests elevation. Use Connections discovery and save the profile/bindings, then `Start saved scene.cmd`. Explicit discovery can start the backend before binding identities exist; it waits in its worker. Saving bindings alone does not activate lights. Connected startup starts the backend only for enabled matching OpenRGB bindings.

To return to an external server, exit Prismestra, remove the managed configuration or set `enabled` to false while retaining its other fields, then restart. Do not run two servers on one endpoint.

## Lifecycle

The native manager requests `--server` with the configured loopback host/port, `--noautoconnect` and isolated config. It omits `--gui` and `--startminimized` (which enables GUI). No console, window or tray is requested. [Manager](../src/app/managed_backend.rs), [upstream CLI](https://github.com/CalcProgrammer1/OpenRGB/blob/8e16de27858cf2be6af54a33ce003df74cb9fd3e/cli.cpp).

It checks SDK readiness and listener PID, refuses foreign occupied endpoints and uses crash-limited backoff. Output guards run through connection setup and immediately before device-mode writes. A genuinely recovered backend renews only its output retries with current scene/Off intent. Unrelated outputs keep their lifecycle. Closing the editor keeps the engine/backend alive; Stop suppresses respawn and drains outputs before release; Exit releases the backend. Windows kill-on-close jobs also terminate it after owner failure. Creation is suspended until job assignment. No service or persistent PowerShell watchdog is added. [Runtime integration](../src/app/runtime/managed_backend.rs).

## Administrator access

Default management uses ordinary permissions. Some controller paths require elevation. An administrator may provision an authorized task/helper, selected through `backend-task.json` and setup's `-AuthorizedTaskName`. This is configurable, not a hardcoded personal task. The task launches the same Prismestra EXE with `--backend-helper <state-directory> <authorized-OpenRGB-executable> <authorized-SHA256>`, with pins supplied by the authorized action rather than scene data. The action must pin the helper EXE too. The administrator must trust/protect the action, binaries, dependencies and isolated configuration: EXE pinning alone does not protect a writable plugin tree. This preview does not ship a general elevated-task installer or claim to establish that trust automatically.

The helper verifies the parent's PID, creation time and executable path. Nonce-bound heartbeat, status and cancel files reject stale sessions. Parent death, cancellation or heartbeat loss releases only its owned child. [Helper](../src/app/backend_helper.rs).

## Coexistence with other software

An answering SDK is not proof that device discovery has finished. Discovery can wait on a shared bus while another application uses it. The manager observes the read-only SDK inventory periodically: a changed controller identity/topology renews the managed outputs' retry budget without restarting a healthy backend. An unchanged inventory does not renew exhausted retries. Inventory polling errors do not justify killing an otherwise healthy server. These checks remain active when the editor is closed.

Prismestra does not stop other lighting, monitoring or cooling applications, release another process's mutex, bypass bus coordination or adopt a foreign SDK listener. Its automatic recovery is restricted to its own backend and outputs. A shared bus can be coordinated by the underlying applications; an exclusive device or two independent color writers may require an explicit choice of owner. Compatibility with arbitrary concurrent software is not guaranteed by backend management.

When troubleshooting, distinguish backend ownership, SDK responsiveness, inventory availability and actual output status. A late inventory can recover without restarting the application. Lifecycle diagnostics record owned child exits/restarts; a missing older log does not establish their cause. A permanently blocked external bus owner still needs a user decision. Rebooting or restarting cooling software is not an automatic recovery action.

## Verification limits

Software fixtures cover multiple endpoints, IPv4/IPv6 ownership, foreign ports, wrong hashes, stale identities/status, backoff, cancellation, owner crash/port release, candidate transactions and targeted recovery. Setup tests never run their fake backend. These do not certify physical controllers, clean Windows or suspend/resume. Persistent errors remain visible in Connections/output status; recovery is not an unlimited hot loop. OpenRGB remains a separate internal process and external dependency, not bundled here. Management does not create support for unknown protocols.
