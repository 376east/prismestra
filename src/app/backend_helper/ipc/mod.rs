//! Local installed-helper transport adapted from the split-token fixture at 75389d3.
mod pipe;
mod protocol;
mod receipt;
#[cfg(test)]
mod tests;

use super::{
    BackendControl, FRESH_MS, HelperChannels, Identity, InstalledStateHistory, Request, Snapshot,
    Status, clock, identity_from_handle, ipc_offer::Offer, parse_control_record, retry,
    write_live_record,
};
use pipe::{AttestedPeer, PeerIdentity, Pipe, PipeError, Side, active_ms};
use protocol::{Backend, Command, Control, Frame, Phase};
use std::{
    io,
    net::SocketAddr,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    ptr::{null, null_mut},
    sync::Mutex,
    thread,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
        WAIT_OBJECT_0,
    },
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    },
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_TYPE_MESSAGE, SetNamedPipeHandleState,
        },
        Threading::{
            CreateEventW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            WaitForMultipleObjects,
        },
    },
};

pub(super) const OFFER_FILE: &str = "helper-ipc-offer.json";
const CLIENT_RIGHTS: u32 = 0x0010_0183;
const EXCHANGE_MS: u64 = 600;
const REPLY_BUDGET_MS: u64 = 1_600;

fn next_command(
    now: u64,
    next_heartbeat: u64,
    expected: Option<(u64, Command)>,
    restarting: bool,
    paused: bool,
) -> Option<Command> {
    let waiting = expected.filter(|(sent, _)| now.saturating_sub(*sent) < REPLY_BUDGET_MS);
    if !restarting && !paused {
        return waiting
            .is_none_or(|(_, command)| command != Command::Pause)
            .then_some(Command::Pause);
    }
    (waiting.is_none() && now >= next_heartbeat).then_some(Command::Heartbeat)
}

pub(super) fn read_offer(path: &Path) -> Result<Option<Offer>, String> {
    super::super::installed_policy::open_ipc_offer(path)?
        .map(parse_control_record)
        .transpose()
        .map_err(super::control_error)
}

fn name(offer: &Offer) -> Vec<u16> {
    format!(
        r"\\.\pipe\PrismestraInstalled-{}-{}-{}-{}",
        offer.owner.pid, offer.owner.created, offer.helper.pid, offer.helper.created
    )
    .encode_utf16()
    .chain([0])
    .collect()
}

fn handle(raw: HANDLE) -> io::Result<OwnedHandle> {
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn process(pid: u32) -> io::Result<OwnedHandle> {
    handle(unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    })
}

fn exact_process(process: &OwnedHandle, pid: u32) -> io::Result<Identity> {
    identity_from_handle(process.as_raw_handle(), pid, |_, _| Ok(())).map_err(io::Error::other)
}

fn attested(process: OwnedHandle, identity: &Identity) -> AttestedPeer {
    AttestedPeer {
        process,
        identity: PeerIdentity {
            pid: identity.pid,
            creation: identity.created,
        },
    }
}

fn pipe_error(error: PipeError) -> io::Error {
    match error {
        PipeError::Policy(error) | PipeError::Io(error) => error,
        error => io::Error::other(format!("{error:?}")),
    }
}

fn create_server(offer: &Offer, logon_sid: &str) -> io::Result<OwnedHandle> {
    if !logon_sid.starts_with("S-1-5-5-")
        || !logon_sid
            .bytes()
            .skip(2)
            .all(|c| c.is_ascii_digit() || c == b'-')
    {
        return Err(io::Error::other("invalid Limited logon SID"));
    }
    let sddl: Vec<_> =
        format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x{CLIENT_RIGHTS:08x};;;{logon_sid})")
            .encode_utf16()
            .chain([0])
            .collect();
    let mut descriptor = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let pipe = handle(unsafe {
        CreateNamedPipeW(
            name(offer).as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            128,
            128,
            0,
            &attrs,
        )
    });
    unsafe { LocalFree(descriptor) };
    pipe
}

fn open_client(offer: &Offer) -> io::Result<OwnedHandle> {
    let pipe = handle(unsafe {
        CreateFileW(
            name(offer).as_ptr(),
            CLIENT_RIGHTS,
            0,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            null_mut(),
        )
    })?;
    if unsafe {
        SetNamedPipeHandleState(pipe.as_raw_handle(), &PIPE_READMODE_MESSAGE, null(), null())
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(pipe)
}

fn wait_connect(
    pipe: &OwnedHandle,
    owner: &OwnedHandle,
    deadline: u64,
    stop: impl Fn(),
) -> io::Result<()> {
    let event = handle(unsafe { CreateEventW(null(), 1, 0, null()) })?;
    let mut op = OVERLAPPED {
        hEvent: event.as_raw_handle(),
        ..Default::default()
    };
    if unsafe { ConnectNamedPipe(pipe.as_raw_handle(), &mut op) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error().map(|code| code as u32) {
        Some(ERROR_PIPE_CONNECTED) => return Ok(()),
        Some(ERROR_IO_PENDING) => {}
        _ => return Err(error),
    }
    let remaining = active_ms()
        .ok()
        .map_or(0, |now| deadline.saturating_sub(now));
    let handles = [owner.as_raw_handle(), event.as_raw_handle()];
    let ready = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, remaining as u32) }
        == WAIT_OBJECT_0 + 1;
    if !ready {
        stop();
        unsafe { CancelIoEx(pipe.as_raw_handle(), &op) };
    }
    let mut bytes = 0;
    let completed = unsafe { GetOverlappedResult(pipe.as_raw_handle(), &op, &mut bytes, 1) };
    if ready && completed != 0 {
        Ok(())
    } else {
        Err(io::Error::other(
            "Installed IPC connect ended before admission",
        ))
    }
}

pub(super) fn supervise(
    channels: &HelperChannels<'_>,
    request: &Request,
    helper: &Identity,
    start: impl FnOnce(&dyn Fn() -> Result<(), String>) -> Result<(), String>,
    snapshot: impl Fn() -> super::super::managed_backend::Snapshot,
    backend_control: impl Fn(BackendControl),
) -> Result<(), String> {
    let authority = channels
        .authority
        .ok_or("Installed helper authority unavailable")?;
    let sid = authority.ipc_logon_sid()?;
    let history = std::cell::RefCell::new(InstalledStateHistory::default());
    let snapshot = || {
        let value = snapshot();
        history.borrow_mut().observe(
            channels,
            request.parent.pid,
            (
                value.backend_pid,
                value.ready,
                value.ready_generation,
                value.detail.clone(),
            ),
        );
        value
    };
    let checking_bootstrap = std::cell::Cell::new(true);
    let verify = |pid| {
        if authority.parent_identity(pid)? != request.parent
            || (checking_bootstrap.replace(false)
                && (channels.admission_control("owner-cancel.json", request)?
                    || channels.admission_control("owner-pause.json", request)?))
        {
            return Err("Installed owner changed, paused or cancelled before IPC".into());
        }
        Ok(())
    };
    serve(
        channels.output,
        make_offer(request, helper),
        &sid,
        verify,
        start,
        snapshot,
        backend_control,
    )
    .map_err(|error| error.to_string())
}

fn make_offer(request: &Request, helper: &Identity) -> Offer {
    Offer {
        nonce: request.nonce.clone(),
        owner: request.parent.clone(),
        helper: helper.clone(),
        epoch: helper.created,
        owner_tick: request.timestamp_ms,
    }
}

fn serve(
    outbox: &Path,
    offer: Offer,
    logon_sid: &str,
    verify: impl Fn(u32) -> Result<(), String>,
    start: impl FnOnce(&dyn Fn() -> Result<(), String>) -> Result<(), String>,
    snapshot: impl Fn() -> super::super::managed_backend::Snapshot,
    backend_control: impl Fn(BackendControl),
) -> io::Result<()> {
    let stop = || backend_control(BackendControl::Stop);
    let mut control = Control::new(offer.epoch, offer.owner_tick, active_ms()?)
        .ok_or_else(|| io::Error::other("Installed IPC bootstrap expired"))?;
    let parent = offer.owner.clone();
    let owner = process(offer.owner.pid)?;
    let server = create_server(&offer, logon_sid)?;
    let path = outbox.join(OFFER_FILE);
    // At most one bounded record per helper. Lease and peer wait do not
    // depend on filesystem completion, even during bootstrap.
    thread::Builder::new()
        .name("backend-helper-ipc-offer".into())
        .spawn(move || {
            let _ = write_live_record(&path, &offer);
        })?;
    wait_connect(&server, &owner, control.deadline(), stop)?;
    let mut pipe = Pipe::admit(
        server,
        Side::Helper,
        |pid| {
            verify(pid).map_err(io::Error::other)?;
            if exact_process(&owner, pid)? != parent {
                return Err(io::Error::other("Installed IPC owner process changed"));
            }
            Ok(attested(owner, &parent))
        },
        stop,
    )
    .map_err(pipe_error)?;
    let mut start = Some(start);
    loop {
        let frame = pipe.receive(control.deadline()).map_err(pipe_error)?;
        verify(parent.pid).map_err(io::Error::other)?;
        let current = if matches!(
            frame,
            Frame::Command {
                command: Command::Heartbeat,
                ..
            }
        ) {
            snapshot()
        } else {
            Default::default()
        };
        let report = Backend {
            pid: current.backend_pid.unwrap_or(0),
            generation: current.ready_generation,
            ready: current.ready,
            inventory_confirmed: current.has_recent_inventory(),
        };
        let reply = control
            .accept(frame, active_ms()?, report)
            .ok_or_else(|| io::Error::other("Installed IPC command denied"))?;
        if let Some(start_manager) = start.take() {
            if !matches!(
                frame,
                Frame::Command {
                    command: Command::Heartbeat,
                    ..
                }
            ) {
                return Err(io::Error::other(
                    "Installed IPC stopped before the first Heartbeat",
                ));
            }
            start_manager(&|| {
                pipe.check_start(control.deadline())
                    .map_err(|error| pipe_error(error).to_string())
            })
            .map_err(io::Error::other)?;
        }
        if let Frame::Ack { phase, .. } = reply {
            match phase {
                Phase::Paused => backend_control(BackendControl::Pause),
                Phase::Closed => stop(),
                Phase::Active => unreachable!(),
            }
        }
        pipe.offer(reply).map_err(pipe_error)?;
        if matches!(
            reply,
            Frame::Ack {
                phase: Phase::Closed,
                ..
            }
        ) {
            // Stop was requested before even attempting to publish the ACK.
            let _ = pipe.flush(
                control
                    .deadline()
                    .min(active_ms()?.saturating_add(EXCHANGE_MS)),
            );
            return Ok(());
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Drain {
    helper: u32,
    backend: Option<u32>,
}

impl Drain {
    pub(super) fn complete(self, endpoint: SocketAddr) -> bool {
        retry::old_processes_absent([self.helper, self.backend.unwrap_or(self.helper)])
            && matches!(
                super::super::managed_backend::reserve_listener_conflict(endpoint),
                Ok(None)
            )
    }
}

pub(super) struct Resident<'a> {
    pub(super) retired: Option<Drain>,
    pub(super) highest_epoch: u64,
    pub(super) session: Option<Session<'a>>,
}

pub(super) struct Session<'a> {
    pipe: Pipe<'a>,
    epoch: u64,
    sequence: u64,
    expected: Option<(u64, Command)>,
    deadline: u64,
    next_heartbeat: u64,
    paused: bool,
    helper: Identity,
    receipt: receipt::Receipt,
}

impl<'a> Resident<'a> {
    pub(super) fn new() -> Self {
        Self {
            retired: None,
            highest_epoch: 0,
            session: None,
        }
    }

    pub(super) fn open(
        &mut self,
        offer: Offer,
        request: &Request,
        output: &'a Mutex<Snapshot>,
    ) -> io::Result<()> {
        if offer.epoch <= self.highest_epoch || offer.epoch != offer.helper.created {
            return Err(io::Error::other(
                "Installed IPC offer epoch is retired or invalid",
            ));
        }
        let self_process = process(request.parent.pid)?;
        let owner = exact_process(&self_process, request.parent.pid)?;
        let helper = process(offer.helper.pid)?;
        let helper_identity = exact_process(&helper, offer.helper.pid)?;
        let now = active_ms()?;
        let deadline = offer
            .admit(request, &owner, &helper_identity, offer.epoch, now)
            .ok_or_else(|| {
                io::Error::other("Installed IPC offer does not match the live helper")
            })?;
        self.highest_epoch = offer.epoch;
        self.retired = Some(Drain {
            helper: helper_identity.pid,
            backend: None,
        });
        invalidate(output);
        let client = open_client(&offer)?;
        let pipe = Pipe::admit(
            client,
            Side::Resident,
            |pid| {
                if pid != offer.helper.pid || exact_process(&helper, pid)? != offer.helper {
                    return Err(io::Error::other(
                        "Installed IPC kernel server differs from offer",
                    ));
                }
                Ok(attested(helper, &offer.helper))
            },
            move || invalidate(output),
        )
        .map_err(pipe_error)?;
        self.session = Some(Session {
            pipe,
            epoch: offer.epoch,
            sequence: 0,
            expected: None,
            deadline,
            next_heartbeat: now,
            paused: false,
            helper: offer.helper,
            receipt: receipt::Receipt::default(),
        });
        Ok(())
    }

    pub(super) fn tick(
        &mut self,
        request: &Request,
        restarting: bool,
    ) -> io::Result<Option<Status>> {
        let session = self.session.as_mut().expect("installed IPC session");
        let status = session.tick(request, restarting)?;
        if let Some(pid) = status.as_ref().and_then(|status| status.backend_pid)
            && let Some(drain) = &mut self.retired
        {
            drain.backend = Some(pid);
        }
        Ok(status)
    }

    pub(super) fn record_published(&mut self, status: &Status, observed: u64) {
        let session = self.session.as_mut().expect("installed IPC session");
        if status.ready
            && let Some(backend) = status.backend_pid
        {
            session.receipt.ready(
                session.epoch,
                session.helper.pid,
                receipt::Sample {
                    seq: session.sequence,
                    tick: status.timestamp_ms,
                    observed,
                    backend,
                    manager_generation: status.ready_generation,
                },
            );
        } else {
            session.receipt.not_ready();
        }
    }

    pub(super) fn exit(&mut self) {
        let mut confirmed = false;
        if let Some(session) = self.session.as_mut()
            && let Ok(now) = active_ms()
        {
            let until = session.deadline.min(now.saturating_add(EXCHANGE_MS));
            if session.pipe.flush(until).is_ok()
                && let Ok(now) = active_ms()
                && session.send(Command::Exit, now).is_ok()
            {
                while let Ok(frame) = session.pipe.receive(until) {
                    if active_ms().is_ok_and(|at| {
                        frame.ack_for(session.epoch, session.sequence, at) == Some(Phase::Closed)
                    }) {
                        confirmed = true;
                        break;
                    }
                }
            }
        }
        if let Some(mut session) = self.session.take() {
            session.pipe.close();
            session.receipt.finish(session.epoch, session.helper.pid);
            receipt::event(
                "ipc_exit",
                session.epoch,
                session.helper.pid,
                format_args!("seq={} ack={confirmed}", session.sequence),
            );
        }
    }

    pub(super) fn lost(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.pipe.close();
            session.receipt.finish(session.epoch, session.helper.pid);
            receipt::event(
                "ipc_loss",
                session.epoch,
                session.helper.pid,
                format_args!("last_sent_seq={}", session.sequence),
            );
        }
    }
}

fn invalidate(output: &Mutex<Snapshot>) {
    let mut snapshot = output.lock().unwrap_or_else(|p| p.into_inner());
    snapshot.ready = false;
    snapshot.backend_pid = None;
    snapshot.detail = "Waiting for authenticated backend IPC".into();
}

impl Session<'_> {
    fn send(&mut self, command: Command, now: u64) -> io::Result<()> {
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Installed IPC sequence overflow"))?;
        self.pipe
            .offer(Frame::Command {
                epoch: self.epoch,
                seq: sequence,
                tick: now,
                command,
            })
            .map_err(pipe_error)?;
        self.sequence = sequence;
        self.expected = Some((now, command));
        Ok(())
    }

    fn tick(&mut self, request: &Request, restarting: bool) -> io::Result<Option<Status>> {
        let now = active_ms()?;
        if now >= self.deadline {
            return Err(io::Error::other("Installed IPC deadline expired"));
        }
        if let Some(command) = next_command(
            now,
            self.next_heartbeat,
            self.expected,
            restarting,
            self.paused,
        ) {
            self.send(command, now)?;
            self.next_heartbeat = now.saturating_add(EXCHANGE_MS);
        }
        let Some(frame) = self
            .pipe
            .poll(self.deadline.min(now.saturating_add(200)))
            .map_err(pipe_error)?
        else {
            return Ok(None);
        };
        let observed = active_ms()?;
        let Some((sent, command)) = self.expected else {
            return Ok(None);
        };
        if command == Command::Pause
            && frame.ack_for(self.epoch, self.sequence, observed) == Some(Phase::Paused)
        {
            self.paused = true;
            self.expected = None;
            self.receipt.finish(self.epoch, self.helper.pid);
            receipt::event(
                "ipc_pause",
                self.epoch,
                self.helper.pid,
                format_args!("seq={} observed_ms={observed}", self.sequence),
            );
            return Ok(None);
        }
        let Some(reply) = frame.reply_for(self.epoch, self.sequence, observed) else {
            return Ok(None);
        };
        let Frame::Ready { tick, .. } = frame else {
            return Ok(None);
        };
        if command != Command::Heartbeat
            || tick < sent
            || !clock::fresh(tick, observed, FRESH_MS)
            || observed.saturating_sub(sent) >= REPLY_BUDGET_MS
        {
            return Ok(None);
        }
        self.expected = None;
        self.deadline = sent
            .checked_add(clock::LEASE_MS)
            .ok_or_else(|| io::Error::other("IPC deadline overflow"))?;
        Ok(Some(Status {
            version: request.version,
            clock_domain: request.clock_domain.clone(),
            nonce: request.nonce.clone(),
            parent: request.parent.clone(),
            helper: self.helper.clone(),
            timestamp_ms: tick,
            ready_generation: reply.backend.generation,
            ready: restarting && reply.backend.ready,
            backend_pid: (reply.backend.pid != 0).then_some(reply.backend.pid),
            inventory_confirmed: reply.backend.inventory_confirmed,
            detail: "Authenticated backend IPC".into(),
        }))
    }
}
