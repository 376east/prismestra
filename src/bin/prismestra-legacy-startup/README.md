# Native legacy startup retirement

Setup packaging builds the Windows x64 helper from the same commit as Setup through `scripts/installer/policy-tool/Cargo.toml`. It requires a static CRT and a Windows GUI PE subsystem, checks the inbox imports, and pins the resulting EXE by SHA-256. Setup checks the protected installation path and the hash before each invocation. The helper is an independent binary and has no startup hook.

## Setup call order

1. Confirm Setup is elevated as the same original interactive user with the existing original-user probe. Pass the probe’s canonical SID. Run `prismestra-legacy-startup.exe Prepare <original-sid>` elevated and wait for its result.
2. Run the existing installed migration command as that original ordinary user. Observe the existing restart and runtime-readiness gates.
3. After successful migration, run `prismestra-legacy-startup.exe Disable <original-sid>` elevated and wait. The helper independently verifies the installed migration receipt and destination hashes before each change.

Keep the command line limited to the phase and the SID. Setup supplies the EXE path and a trusted working directory. Exercise both invocations on the intended installer runtime floor.

The helper exits 0 and prints one JSON object. `prepared` confirms the source hint, `disabled` confirms the named tasks and returns the receipt path, and `verified` means there was no active verified legacy task. `unchanged_*` records a deferral for an unverified original logon, ambiguous sources, unconfirmed import or insufficient protection of the task or root folder. A nonzero exit reports an operation failure on stderr and includes a protected receipt path when one was created. Capture both streams.

## Ownership and rollback

Prepare verifies the original SID in the task principal and its single enabled logon trigger, and checks the inbox action, argument list, launcher hash, release pointer and profile under the ordinary token. If the linked token is limited to Identification, the helper obtains the interactive desktop shell’s limited primary token only after matching the SID, session and logon LUID. A missing or mismatched desktop token defers retirement.

Disable considers only verified enabled tasks in the root scheduler folder. It additionally requires SYSTEM or Administrators ownership and a DACL that prevents ordinary users from replacing the task, with a corresponding check on the root folder. Unknown definitions and Run/RunOnce entries are preserved. The helper stages an ACL-protected receipt under ProgramData before calling `SetEnabled`. Rollback rechecks the launcher pin, task security and all XML outside the direct `Task/Settings/Enabled` element. The scheduler API applies changes by path, so privileged concurrent edits can still require manual recovery from the receipt. [Task Scheduler protocol](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-tsch/609994ba-d25a-476a-ad2d-5b8b0340f647) · [Task XML schema](https://learn.microsoft.com/en-us/windows/win32/taskschd/task-scheduler-schema)

Local fixtures use synthetic XML, source files and scheduler state. On a disposable Windows guest, validate both legacy launch roles, default and explicit Enabled, ordinary-writable and foreign tasks, and a partial failure with a protected rollback receipt. Guest verification is the gate for switching Setup to this binary.
