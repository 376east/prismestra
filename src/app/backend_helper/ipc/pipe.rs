use super::super::clock::{ActiveClock, Clock};
use super::protocol::{Command, Frame, WIRE_BYTES};
use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
    rc::Rc,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, FILETIME, GetLastError, HANDLE, WAIT_FAILED,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Storage::FileSystem::{ReadFile, WriteFile},
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId, PeekNamedPipe},
        Threading::{
            CreateEventW, GetProcessId, GetProcessTimes, ResetEvent, Sleep, WaitForMultipleObjects,
            WaitForSingleObject,
        },
    },
};

pub fn active_ms() -> io::Result<u64> {
    ActiveClock.now_ms().map_err(io::Error::other)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerIdentity {
    pub pid: u32,
    pub creation: u64,
}

impl PeerIdentity {
    pub fn from_handle(process: &OwnedHandle) -> io::Result<Self> {
        let process = process.as_raw_handle();
        let pid = unsafe { GetProcessId(process) };
        let [mut creation, mut exit, mut kernel, mut user] = [FILETIME::default(); 4];
        if pid == 0
            || unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }
                == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            pid,
            creation: (u64::from(creation.dwHighDateTime) << 32)
                | u64::from(creation.dwLowDateTime),
        })
    }
}

/// Supplied only by an external policy that has already verified the peer.
/// The experiment has no installed policy implementation and no permissive fallback.
pub struct AttestedPeer {
    pub process: OwnedHandle,
    pub identity: PeerIdentity,
}

#[derive(Clone, Copy)]
pub enum Side {
    Resident,
    Helper,
}

#[derive(Debug)]
pub enum PipeError {
    Policy(io::Error),
    Io(io::Error),
    Identity,
    PeerExited,
    Deadline,
    Frame,
    Busy,
    Closed,
}

impl From<io::Error> for PipeError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

struct Operation {
    pipe: Rc<OwnedHandle>,
    event: OwnedHandle,
    overlapped: OVERLAPPED,
    buffer: [u8; WIRE_BYTES],
    submitted: bool,
}

impl Operation {
    fn start(pipe: Rc<OwnedHandle>, write: Option<Frame>) -> io::Result<Box<Self>> {
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut op = Box::new(Self {
            pipe,
            event: unsafe { OwnedHandle::from_raw_handle(event) },
            overlapped: OVERLAPPED {
                hEvent: event,
                ..Default::default()
            },
            buffer: write.map_or([0; WIRE_BYTES], Frame::encode),
            submitted: false,
        });
        // The heap allocation and pipe stay alive until completion, including
        // when the caller abandons this operation.
        let started = unsafe {
            if write.is_some() {
                WriteFile(
                    op.pipe.as_raw_handle(),
                    op.buffer.as_ptr(),
                    WIRE_BYTES as u32,
                    null_mut(),
                    &mut op.overlapped,
                )
            } else {
                ReadFile(
                    op.pipe.as_raw_handle(),
                    op.buffer.as_mut_ptr(),
                    WIRE_BYTES as u32,
                    null_mut(),
                    &mut op.overlapped,
                )
            }
        };
        if started == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(io::Error::last_os_error());
        }
        op.submitted = true;
        Ok(op)
    }

    fn result(&mut self) -> io::Result<Option<usize>> {
        // Reset before the query, as in windows_io. Completion that races the
        // reset is visible to the query; later completion signals the event.
        unsafe { ResetEvent(self.event.as_raw_handle()) };
        let mut count = 0;
        if unsafe {
            GetOverlappedResult(self.pipe.as_raw_handle(), &self.overlapped, &mut count, 0)
        } != 0
        {
            self.submitted = false;
            return Ok(Some(count as usize));
        }
        let error = unsafe { GetLastError() };
        if matches!(error, ERROR_IO_INCOMPLETE | ERROR_IO_PENDING) {
            return Ok(None);
        }
        self.submitted = false;
        Err(io::Error::from_raw_os_error(error as i32))
    }

    fn cancel(&self) {
        if self.submitted {
            unsafe { CancelIoEx(self.pipe.as_raw_handle(), &self.overlapped) };
        }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        self.cancel();
        while self.submitted {
            // Same completion boundary as transport/windows_io.rs. A cancellation
            // request or signal alone does not release the buffer.
            if !matches!(self.result(), Ok(None)) {
                break;
            }
            if unsafe { WaitForSingleObject(self.event.as_raw_handle(), 25) } == WAIT_FAILED {
                unsafe { Sleep(1) };
            }
        }
    }
}

pub struct Pipe<'a> {
    handle: Rc<OwnedHandle>,
    side: Side,
    peer: OwnedHandle,
    read: Option<Box<Operation>>,
    write: Option<Box<Operation>>,
    latest: Option<Frame>,
    closed: bool,
    revoke: Option<Box<dyn FnOnce() + 'a>>,
}

impl<'a> Pipe<'a> {
    /// The caller connects using explicit bounded overlapped I/O, then supplies
    /// the existing policy as a gate before any frame can be read or sent.
    pub fn admit(
        handle: OwnedHandle,
        side: Side,
        attest: impl FnOnce(u32) -> io::Result<AttestedPeer>,
        revoke: impl FnOnce() + 'a,
    ) -> Result<Self, PipeError> {
        let mut pid = 0;
        let ok = unsafe {
            match side {
                Side::Helper => GetNamedPipeClientProcessId(handle.as_raw_handle(), &mut pid),
                Side::Resident => GetNamedPipeServerProcessId(handle.as_raw_handle(), &mut pid),
            }
        };
        if ok == 0 || pid == 0 {
            return Err(PipeError::Identity);
        }
        let pinned = attest(pid).map_err(PipeError::Policy)?;
        if pinned.identity.pid != pid
            || PeerIdentity::from_handle(&pinned.process)? != pinned.identity
            || unsafe { WaitForSingleObject(pinned.process.as_raw_handle(), 0) } != WAIT_TIMEOUT
        {
            return Err(PipeError::Identity);
        }
        Ok(Self {
            handle: Rc::new(handle),
            side,
            peer: pinned.process,
            read: None,
            write: None,
            latest: None,
            closed: false,
            revoke: Some(Box::new(revoke)),
        })
    }

    /// Only helper status is replaceable. Resident control commands require an
    /// empty write slot and can never silently overwrite Pause or Exit.
    pub fn offer(&mut self, frame: Frame) -> Result<(), PipeError> {
        if self.closed {
            return Err(PipeError::Closed);
        }
        let command = matches!(frame, Frame::Command { .. });
        if command != matches!(self.side, Side::Resident) {
            return Err(PipeError::Frame);
        }
        if command {
            self.reap_write()?;
            if self.write.is_some() || self.latest.is_some() {
                return Err(PipeError::Busy);
            }
        }
        if matches!(self.latest, Some(Frame::Ack { .. })) {
            return Err(PipeError::Busy);
        }
        self.latest = Some(frame);
        Ok(())
    }

    pub fn receive(&mut self, deadline: u64) -> Result<Frame, PipeError> {
        let result = self.receive_inner(deadline);
        if result.is_err() {
            self.close();
        }
        result
    }

    pub fn poll(&mut self, deadline: u64) -> Result<Option<Frame>, PipeError> {
        match self.receive_inner(deadline) {
            Ok(frame) => Ok(Some(frame)),
            Err(PipeError::Deadline) => Ok(None),
            Err(error) => {
                self.close();
                Err(error)
            }
        }
    }

    pub fn check_start(&self, deadline: u64) -> Result<(), PipeError> {
        self.check(deadline)?;
        let mut pending = 0;
        // The helper endpoint was opened for overlapped I/O. Any pending
        // command could revoke authority while manager preparation was blocked.
        if unsafe {
            PeekNamedPipe(
                self.handle.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                &mut pending,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if pending != 0 {
            return Err(PipeError::Busy);
        }
        self.check(deadline).map(|_| ())
    }

    pub fn flush(&mut self, deadline: u64) -> Result<(), PipeError> {
        let result = (|| {
            loop {
                self.check(deadline)?;
                self.pump_write()?;
                if self.write.is_none() && self.latest.is_none() {
                    return Ok(());
                }
                self.wait(deadline)?;
            }
        })();
        if result.is_err() {
            self.close();
        }
        result
    }

    fn receive_inner(&mut self, deadline: u64) -> Result<Frame, PipeError> {
        loop {
            self.check(deadline)?;
            if self.read.is_none() {
                self.read = Some(Operation::start(self.handle.clone(), None)?);
            }
            if let Some(count) = self.read.as_mut().unwrap().result()? {
                let bytes = self.read.take().unwrap().buffer;
                self.check(deadline)?;
                if count != WIRE_BYTES {
                    return Err(PipeError::Frame);
                }
                let frame = Frame::decode(bytes).ok_or(PipeError::Frame)?;
                if matches!(frame, Frame::Command { .. }) != matches!(self.side, Side::Helper) {
                    return Err(PipeError::Frame);
                }
                if matches!(
                    frame,
                    Frame::Command {
                        command: Command::Pause | Command::Exit,
                        ..
                    }
                ) {
                    if matches!(self.latest, Some(Frame::Ready { .. })) {
                        self.latest = None;
                    }
                } else {
                    // Incoming frames can be continuously ready. Give outbound
                    // work one non-blocking turn before returning each frame.
                    self.pump_write()?;
                }
                return Ok(frame);
            }
            self.pump_write()?;
            self.wait(deadline)?;
        }
    }

    fn reap_write(&mut self) -> Result<(), PipeError> {
        if let Some(write) = self.write.as_mut() {
            match write.result()? {
                None => return Ok(()),
                Some(WIRE_BYTES) => self.write = None,
                Some(_) => return Err(PipeError::Frame),
            }
        }
        Ok(())
    }

    fn pump_write(&mut self) -> Result<(), PipeError> {
        self.reap_write()?;
        if self.write.is_some() {
            return Ok(());
        }
        if let Some(frame) = self.latest.take() {
            self.write = Some(Operation::start(self.handle.clone(), Some(frame))?);
        }
        Ok(())
    }

    fn check(&self, deadline: u64) -> Result<u32, PipeError> {
        if self.closed {
            return Err(PipeError::Closed);
        }
        match unsafe { WaitForSingleObject(self.peer.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => return Err(PipeError::PeerExited),
            WAIT_TIMEOUT => {}
            _ => return Err(io::Error::last_os_error().into()),
        }
        let remaining = deadline
            .checked_sub(active_ms()?)
            .filter(|ms| *ms != 0)
            .ok_or(PipeError::Deadline)?;
        Ok(remaining.min(u64::from(u32::MAX - 1)) as u32)
    }

    fn wait(&self, deadline: u64) -> Result<(), PipeError> {
        let remaining = self.check(deadline)?;
        let mut handles: [HANDLE; 3] = [self.peer.as_raw_handle(), null_mut(), null_mut()];
        let mut count = 1;
        for op in [self.read.as_ref(), self.write.as_ref()]
            .into_iter()
            .flatten()
        {
            handles[count] = op.event.as_raw_handle();
            count += 1;
        }
        match unsafe { WaitForMultipleObjects(count as u32, handles.as_ptr(), 0, remaining) } {
            WAIT_OBJECT_0 => Err(PipeError::PeerExited),
            WAIT_TIMEOUT => self.check(deadline).map(|_| ()),
            result if (1..count as u32).contains(&result) => Ok(()),
            _ => Err(io::Error::last_os_error().into()),
        }
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.latest = None;
        if let Some(revoke) = self.revoke.take() {
            revoke();
        }
        for op in [self.read.as_ref(), self.write.as_ref()]
            .into_iter()
            .flatten()
        {
            op.cancel();
        }
        self.read = None;
        self.write = None;
    }
}

impl Drop for Pipe<'_> {
    fn drop(&mut self) {
        self.close();
    }
}
