# Startup and recovery acceptance

The user-facing requirement is an unattended saved scene after Windows sign-in, with existing monitoring/cooling programs left running. A successful build, a startup acknowledgement or running the scheduled task manually does not establish that requirement. The current local hardware profile is an acceptance fixture, not a protocol rule or an application-name allowlist.

## Helper ownership clock

Windows Rust `Instant` uses QPC, which includes standby and hibernation. The previous five-second helper lease could therefore expire while the whole machine slept, before the helper even read a newly refreshed request. UTC timestamps also made ownership sensitive to wall-clock corrections. This is a demonstrated design defect, not an attribution of every historical helper restart. [Microsoft QPC characteristics](https://learn.microsoft.com/en-us/windows/win32/sysinfo/acquiring-high-resolution-time-stamps#low-level-hardware-clock-characteristics).

Request, status, cancel and pause controls now require `version: 2` and `clock_domain: "windows-unbiased-interrupt-ms-v1"`. `timestamp_ms` in request/status is boot-relative **active** milliseconds from `QueryUnbiasedInterruptTime / 10000`. It is not Unix time. This API excludes sleep/hibernation and UTC adjustments. [Microsoft API](https://learn.microsoft.com/en-us/windows/win32/api/realtimeapiset/nf-realtimeapiset-queryunbiasedinterrupttime).

The helper deadline is the heartbeat's absolute active timestamp plus five seconds. Rereading an unchanged or unavailable request never renews it. Future timestamps, overflow, clock-query errors, mixed versions and wrong clock domains fail closed. PID, process creation time, executable path, nonce, exclusive client lock, endpoint ownership and matching cancellation remain required. Process identity still rejects pre-reboot files; a clock-domain tag is not authorization on its own.

`backend-task.json` configuration and native core-startup acknowledgements remain version1; only the private owner/helper control protocol changes. Existing v1 helpers and v2 application processes must not be mixed. Update the authorized helper launcher and application together, retaining a matched rollback pair. The reusable PowerShell contract is [Helper Clock.ps1](../scripts/windows/Helper%20Clock.ps1); it does not register tasks or launch applications. A configured private launcher must explicitly incorporate it. This repository still does not ship a general privileged-task installer.

## Diagnostic history

After validating the owner and acquiring the managed backend, the helper records lifecycle transitions in `state/backend-events.json`. At most32events and16KiB are retained using the existing bounded atomic writer. It records helper/parent/backend PIDs, a bounded summary and a UTC time intended only for human chronology. It stores no control nonce, profile, credentials or frame payloads. The record survives a launcher reopening its separate stderr file.

Only state changes and stop/drain reasons are recorded, not every render tick. Record failures cannot stop lighting or weaken ownership checks. Malformed/oversized history is preserved rather than overwritten. The size is bounded; filesystem latency is not guaranteed bounded. A draining event means the helper is about to release its owned manager, not proof of successful cleanup or child exit. Those require actual child-exit/log receipts.

The installed helper uses the protected outbox for its bootstrap offer, then carries heartbeat, readiness, Pause and Exit over the authenticated local pipe. Its bounded state writer still queues `backend_state` transitions in the outbox. The separate Windows Application queue reports IPC admission, commands, loss and one continuous readiness window per session. The private and portable helper still uses synchronous heartbeat and status files and retains its file-based lifecycle history.

## Acceptance gates

### Late LCD availability

The resident coordinator subscribes to Windows HID interface notifications independently of the editor window. It watches only whole interface paths from the committed LCD output plan. Subscription precedes the initial read-only interface enumeration; callbacks arriving during that enumeration take precedence over its snapshot. Target changes and enumeration run outside the coordinator lock. The callback only updates bounded notification state: it does not open devices, issue reports or reset USB ports. Registration is released outside the callback. [Microsoft notification contract](https://learn.microsoft.com/en-us/windows/win32/api/cfgmgr32/nf-cfgmgr32-cm_register_notification).

The return of a bound interface renews that LCD's finite 1/5/15-second recovery sequence. Other interfaces and duplicate arrivals do not renew it. An initial presence snapshot does not renew an already ready worker. A genuine removal/arrival retains one recovery credit if the old handle has not reported its I/O failure yet; the ready worker is not restarted just because a notification arrived. Stop, suspension, configuration transitions and the old owner's unreleased lease retain their existing guards. Reopening still verifies the exact target and HID descriptor; ASCII case differences in the same Windows symbolic path do not select a different device.

This cannot repair a device that Windows cannot enumerate. A USB device-descriptor failure/code43 is a separate prerequisite failure, not a reason to reset a USB hub or interfere with cooling software. A native registration smoke test and simulated arrivals do not prove physical unplug/replug recovery; that needs a working LCD interface and separate acceptance.

- Deterministic cases cover sleep and UTC changes independently from advancing active time, expired/unreadable heartbeats, failed clock APIs and mixed protocol controls. They do not certify an actual sleep/resume or reboot.
- Existing discovery/late-inventory tests must keep independent outputs alive while an initially busy resource is deferred and later becomes available. Do not add a second competing discovery loop or change foreign startup order.
- Install the final checked package with its matching launcher; preserve saved scene, bindings, sensors, draft and backend configuration. Restart only owned processes.
- On an actual new boot/sign-in, capture boot time, owned process identities, saved-scene output, required controller inventory and advancing instrumented RAM writes over at least five minutes. Keep existing cooling programs unchanged and verify their process/settings continuity during observation.
- Test editor closure and actual sleep/resume separately at an agreed time. Do not label configured Hz or successful SDK writes as measured optical FPS.

There is no general permission to stop foreign programs or release/bypass their synchronization objects. A live foreign owner must release its own mutex; an abandoned object and a currently owned object are distinct states. Mid-scan contention and synchronous kernel calls retain their documented limits. [Windows ownership semantics](https://learn.microsoft.com/en-us/windows/win32/sync/mutex-objects), [component limits](openrgb-component-verification.md).
