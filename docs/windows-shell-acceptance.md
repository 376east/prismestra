# Windows shell acceptance

Use a disposable Windows guest and a synthetic profile. Record the Windows build, Git revision, SHA-256 of Setup or the portable archive, and the launch path for each scenario. Keep the process and window traces with the result. The checker reads normalized evidence and leaves all Windows settings untouched.

| Scenario | Start the trace before | End the scenario after |
| --- | --- | --- |
| `setup` | Opening Setup | Installation completes and first launch returns its expected result |
| `portable` | Opening the shipped portable entry point | The editor opens and the synthetic scene is available |
| `logon` | Signing in to the test account | The installed app starts and restores the synthetic scene |
| `rgb-recovery` | Interrupting the simulated RGB receiver | The receiver gets the latest scene after recovery |
| `upgrade` | Running Setup over the pinned earlier release | The updated app can read the preserved profile and local bindings |
| `uninstall` | Launching the installed uninstaller | The uninstall receipt confirms the expected owned artifacts were removed |
| `legacy-migration` | Starting the equivalent legacy import path | The migration receipt completes and the target files match the fixture |

Capture process start and stop events, with executable names, parent PIDs and session IDs. Use process identity `PID:start-filetime`, including when the same PID appears twice. Resolve the parent to the instance alive when the child started. Record the launcher or Setup as a product root, even if it is itself a shell. Register detached Task Scheduler launches and elevated helpers as separate product roots using the verified executable and the scenario's PID evidence. For an actual logon, collect process events from before sign-in and establish window coverage for the interactive desktop. A manually replayed command covers the launch path only. [Windows process start events](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/krnlprov/win32-processstarttrace) · [PID lifetime](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-process)

In each measured desktop, record visible console shows and windows already visible at the scenario boundary. A guest collector can use `SetWinEventHook` with a message loop and resolve the window owner with `GetWindowThreadProcessId`. Include both classic console windows and Windows Terminal windows. Keep unresolved visible console events in the evidence with a null process. Run a known short-lived shell and a visible console outside the product interval to establish that both collectors report events. Mark the matching control false if a subscription failed, events were lost, the desktop was uncovered, or the positive control failed. [WinEvent hooks](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwineventhook) · [Console events](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/dnacc/console-accessibility)

Normalize one JSON file per scenario with `scenario`, `revision`, `artifact_sha256`, and `controls`. The `controls` keys are `process_start_stop`, `visible_console`, and `full_scenario`. Include `product_roots` and `harness_roots` as process identity lists. Each `processes` entry has `id`, `parent` (identity or null), and `image`. Each `windows` entry has `process` (identity or null), `class`, and boolean `visible`. Retain only the process and window data needed for the test. Harness shells are recognized by ancestry, including when a shell launches a product root. A shell under a product root still counts against the product.

Every non-null parent must be present in the trace and refer to the current process instance at the child's start. If a parent cannot be resolved and the missing process affects coverage, set `process_start_stop` to false.

For process capture, open an elevated PowerShell session in a disposable guest. Set the interval long enough for the planned scenario, wait for the capture-ready message, and perform the scenario from the intended desktop. The harness writes start and stop events plus a short `cmd.exe` control on each side of the measured interval. [CIM event capture](https://learn.microsoft.com/en-us/powershell/module/cimcmdlets/register-cimindicationevent)

```powershell
.\scripts\installer\capture_process_events.ps1 -DisposableGuest -DurationSeconds 900 -OutputPath .\uninstall-processes.json
python scripts/normalize_process_events.py uninstall-processes.json --scenario uninstall --revision <commit> --artifact-sha256 <sha256> --product-root '<pid>:<start-filetime>' --output uninstall-partial.json
```

Identify each `--product-root` from its observed launch, including detached scheduled tasks when applicable. The normalizer writes a partial receipt. Add actual window observations and establish scenario coverage before using the result as a complete acceptance receipt.

Start the window observer on each measured desktop before the process interval. Allow time for an actual visible control console before and after the product action. The observer retains the desktop and session, the window class, and the window owner's PID and creation time when available. It does not collect window titles. The owner can be a console host or Terminal broker. Leave the owner unresolved in the acceptance receipt unless the evidence identifies the owning product or harness process instance. [Window ownership API](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowthreadprocessid)

```powershell
python scripts/capture_console_windows.py --disposable-guest --duration-seconds 1000 --output uninstall-windows.json
```

```powershell
python scripts/shell_acceptance.py evidence.json --scenario uninstall --revision <commit> --artifact-sha256 <sha256>
```

Exit code `0` confirms the scenario on the expected artifact with complete controls. Exit code `1` reports a shell started by the product or a visible product console. Exit code `2` means evidence is invalid or incomplete, including an unattributed shell or visible console. Apply the same checker to the control release and to the combined Setup, runtime and portable candidate. Preserve the three component heads and the final candidate tree in the evidence.
