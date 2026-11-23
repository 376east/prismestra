# Managed local OpenRGB

Prismestra can own a local OpenRGB SDK server without its window or tray icon. Executable, configuration directory and numeric loopback endpoint are machine settings; device models/counts and personal paths are not compiled in. Remote or manually managed servers remain external.

The Windows installer bundles and configures a private backend automatically, including its protected authorization policy. Its first-run coordinator can discover devices before saved bindings exist. See [the installed startup path](windows-installer.md).

## Configure a portable backend

In a fresh portable folder, open **Connections → Configure local OpenRGB** («Подключения → Настроить локальный OpenRGB…»). Select your OpenRGB SDK6 executable and an isolated configuration folder. Review the paths, default loopback endpoint and SHA-256, then confirm. To test the connection, select an OpenRGB device in Connections and start discovery. This action is available before a portable configuration or helper reference exists.

For a new or ordinary-user portable configuration, exit the resident session and open a Windows shortcut to the extracted `Prismestra.exe` with `--configure-portable-openrgb "C:\Apps\OpenRGB\OpenRGB.exe" "C:\Lighting\OpenRGB-config"`. An optional final argument sets a numeric loopback endpoint. Confirm the displayed endpoint and SHA-256 to save. Leave any configured helper task in place. If a proposed change would affect that configuration, Prismestra leaves both files intact so that the task's authorization can be reviewed separately.

Select an OpenRGB SDK6 executable and an isolated configuration directory. Disable unrelated plugins and detectors for devices driven directly by Prismestra. Exit any server using the intended endpoint. The native configuration pins the EXE SHA-256 in `data/state/managed-openrgb.json`. Use Connections discovery and save the profile and bindings, then use the [portable EXE shortcut for connected startup](portable-launch.md#first-setup). Explicit discovery can start the backend before binding identities exist. Saving bindings alone does not activate lights. Connected startup starts the backend only for enabled matching OpenRGB bindings.

For a portable configuration without a helper reference, exit Prismestra, remove the managed configuration or set `enabled` to false while retaining its other fields, then restart to use an external server. Do not run two servers on one endpoint.

## Lifecycle

The native manager requests `--server` with the configured loopback host/port, `--noautoconnect` and isolated config. It omits `--gui` and `--startminimized` (which enables GUI). No console, window or tray is requested. [Manager](../src/app/managed_backend.rs), [upstream CLI](https://github.com/CalcProgrammer1/OpenRGB/blob/8e16de27858cf2be6af54a33ce003df74cb9fd3e/cli.cpp).

It checks SDK readiness and listener PID, refuses foreign occupied endpoints and uses crash-limited backoff. Output guards run through connection setup and immediately before device-mode writes. A genuinely recovered backend renews only its output retries with current scene/Off intent. Unrelated outputs keep their lifecycle. Closing the editor keeps the engine/backend alive; Stop suppresses respawn and drains outputs before release; Exit releases the backend. Windows kill-on-close jobs also terminate it after owner failure. Creation is suspended until job assignment. No service or persistent PowerShell watchdog is added. [Runtime integration](../src/app/runtime/managed_backend.rs).

The private component recognizes its existing opt-in plus an inherited parent pipe before probing Windows service startup. That leased application path enters normal headless initialization directly; standalone and actual service startup retain their original entrypoint. This avoids aborting ordinary-user launches when the service dispatcher returns access denied. It does not grant new privileges or relax SDK ownership checks. [Pinned component patch](../components/openrgb/prismestra-pc.patch), [Windows dispatcher contract](https://learn.microsoft.com/en-us/windows/win32/api/winsvc/nf-winsvc-startservicectrldispatchera).

## Administrator access

Default management uses ordinary permissions. Some controller paths require elevation. Windows Setup provisions the installed product's protected task and per-user policy during its installation elevation; the editor stays unelevated. [Installed permissions](windows-installer.md).

For a portable build, an administrator may provision an authorized task/helper, selected through `data/state/backend-task.json`. The task launches the same Prismestra EXE with `--backend-helper <state-directory> <authorized-OpenRGB-executable> <authorized-SHA256>`, with pins supplied by the authorized action rather than scene data. The action must pin the helper EXE too. The administrator must trust and protect the action, binaries, dependencies and isolated configuration. EXE pinning alone does not protect a writable plugin tree.

A portable ZIP update preserves the existing task reference and OpenRGB configuration. The helper can still run when the authorized action, paths and pins remain valid. Before updating, exit from the tray and back up the previous program files and `data`. If the update fails, restore the previous files at the same paths. If the OpenRGB files were changed too, restore the authorized binary and its matching configuration. If the task is missing or its action or pins need to change, ask the administrator who provisioned it to verify the authorization. Portable Connections and the native setup command cannot reauthorize that task.

The matching source ZIP retains `prismestra-source/scripts/portable/Configure OpenRGB.ps1` for development and administrator compatibility. For initial association with a task the administrator has independently verified, the utility accepts `-AuthorizedTaskName` and an explicit `-StateDirectory` pointing to the portable folder's `data/state`. The utility checks that the task exists. Verification of its action, principal and protected files remains the administrator's responsibility. The utility cannot reauthorize an existing task or validate a changed pin against its action.

The helper verifies the parent's PID, creation time and executable path. Nonce-bound heartbeat, status and cancel files reject stale sessions. The private controls use protocol2 and a boot-relative active-time clock, excluding sleep/hibernation and UTC corrections; deploy the helper launcher and application together. Parent death, cancellation or active heartbeat loss releases only its owned child. A bounded diagnostic history survives helper restarts. [Helper](../src/app/backend_helper.rs), [clock contract, diagnostics and actual boot gates](startup-recovery.md).

## Coexistence with other software

An answering SDK is not proof that device discovery has finished. Discovery can wait on a shared bus while another application uses it. The manager observes the read-only SDK inventory periodically: a changed controller identity/topology renews the managed outputs' retry budget without restarting a healthy backend. An unchanged inventory does not renew exhausted retries. Inventory polling errors do not justify killing an otherwise healthy server. These checks remain active when the editor is closed.

Prismestra does not stop other lighting, monitoring or cooling applications, release another process's mutex, bypass bus coordination or adopt a foreign SDK listener. Its automatic recovery is restricted to its own backend and outputs. A shared bus can be coordinated by the underlying applications; an exclusive device or two independent color writers may require an explicit choice of owner. Compatibility with arbitrary concurrent software is not guaranteed by backend management.

When troubleshooting, distinguish backend ownership, SDK responsiveness, inventory availability and actual output status. A late inventory can recover without restarting the application. Lifecycle diagnostics record owned child exits/restarts; a missing older log does not establish their cause. A permanently blocked external bus owner still needs a user decision. Rebooting or restarting cooling software is not an automatic recovery action.

An existing OpenRGB device can report a different zone layout after a configuration change or installation migration. The selected device shows the saved and detected LED totals and the detected zone sizes, or a separate message if compatible Direct mode is missing. Restore only manually configurable zones from a known previous layout for that controller. Leave fixed or firmware-detected zones to OpenRGB. The binding stores a topology hash and cannot reconstruct the previous sizes. The installed lighting-only SDK does not permit zone resizing. Once the automatic attempts are exhausted, choose **Retry connection** for that device. A managed retry uses the current owned backend and checks the full saved identity and topology again before activating Direct.

## Verification limits

Software fixtures cover multiple endpoints, IPv4/IPv6 ownership, foreign ports, wrong hashes, stale identities/status, backoff, cancellation, owner crash/port release, candidate transactions and targeted recovery. Setup tests never run their fake backend. These do not certify physical controllers, clean Windows or suspend/resume. Persistent errors remain visible in Connections/output status; recovery is not an unlimited hot loop. OpenRGB remains a separate internal process and external dependency, not bundled here. Management does not create support for unknown protocols.
