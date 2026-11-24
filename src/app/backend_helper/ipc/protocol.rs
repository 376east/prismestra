use super::super::{
    FRESH_MS,
    clock::{self, LEASE_MS},
};

pub const WIRE_BYTES: usize = 40;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Heartbeat,
    Pause,
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Active,
    Paused,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reply {
    pub phase: Phase,
    pub backend: Backend,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Backend {
    pub pid: u32,
    pub generation: u64,
    pub ready: bool,
    pub inventory_confirmed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Frame {
    Command {
        epoch: u64,
        seq: u64,
        tick: u64,
        command: Command,
    },
    Ready {
        epoch: u64,
        seq: u64,
        tick: u64,
        reply: Reply,
    },
    Ack {
        epoch: u64,
        seq: u64,
        tick: u64,
        phase: Phase,
    },
}

impl Frame {
    pub fn encode(self) -> [u8; WIRE_BYTES] {
        let (epoch, seq, tick, kind, value, backend) = match self {
            Self::Command {
                epoch,
                seq,
                tick,
                command,
            } => (
                epoch,
                seq,
                tick,
                match command {
                    Command::Heartbeat => 1,
                    Command::Pause => 2,
                    Command::Exit => 3,
                },
                0,
                Backend::default(),
            ),
            Self::Ready {
                epoch,
                seq,
                tick,
                reply,
            } => (
                epoch,
                seq,
                tick,
                4,
                match reply.phase {
                    Phase::Active => 0,
                    Phase::Paused => 1,
                    Phase::Closed => 2,
                } | (u8::from(reply.backend.ready) << 2)
                    | (u8::from(reply.backend.inventory_confirmed) << 3),
                reply.backend,
            ),
            Self::Ack {
                epoch,
                seq,
                tick,
                phase,
            } => (
                epoch,
                seq,
                tick,
                5,
                match phase {
                    Phase::Active => 0,
                    Phase::Paused => 1,
                    Phase::Closed => 2,
                },
                Backend::default(),
            ),
        };
        let mut bytes = [0; WIRE_BYTES];
        bytes[..4].copy_from_slice(&[2, kind, value, 0]);
        bytes[4..12].copy_from_slice(&epoch.to_le_bytes());
        bytes[12..20].copy_from_slice(&seq.to_le_bytes());
        bytes[20..28].copy_from_slice(&tick.to_le_bytes());
        bytes[28..32].copy_from_slice(&backend.pid.to_le_bytes());
        bytes[32..40].copy_from_slice(&backend.generation.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: [u8; WIRE_BYTES]) -> Option<Self> {
        let [version, kind, value, reserved] = bytes[..4].try_into().unwrap();
        let epoch = u64::from_le_bytes(bytes[4..12].try_into().unwrap());
        let seq = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        let tick = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
        let pid = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let generation = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        if version != 2
            || reserved != 0
            || epoch == 0
            || seq == 0
            || (kind != 4 && (pid != 0 || generation != 0))
        {
            return None;
        }
        match (kind, value) {
            (1..=3, 0) => Some(Self::Command {
                epoch,
                seq,
                tick,
                command: [Command::Heartbeat, Command::Pause, Command::Exit][kind as usize - 1],
            }),
            (4, 0 | 1 | 4 | 12) if value & 4 == 0 || (pid != 0 && generation != 0) => {
                Some(Self::Ready {
                    epoch,
                    seq,
                    tick,
                    reply: Reply {
                        phase: [Phase::Active, Phase::Paused, Phase::Closed][(value & 3) as usize],
                        backend: Backend {
                            pid,
                            generation,
                            ready: value & 4 != 0,
                            inventory_confirmed: value & 8 != 0,
                        },
                    },
                })
            }
            (5, 1 | 2) => Some(Self::Ack {
                epoch,
                seq,
                tick,
                phase: if value == 1 {
                    Phase::Paused
                } else {
                    Phase::Closed
                },
            }),
            _ => None,
        }
    }

    pub fn reply_for(self, expected_epoch: u64, expected_seq: u64, now: u64) -> Option<Reply> {
        match self {
            Self::Ready {
                epoch,
                seq,
                tick,
                reply,
            } if epoch == expected_epoch
                && seq == expected_seq
                && clock::fresh(tick, now, FRESH_MS) =>
            {
                Some(reply)
            }
            _ => None,
        }
    }

    pub fn ack_for(self, expected_epoch: u64, expected_seq: u64, now: u64) -> Option<Phase> {
        match self {
            Self::Ack {
                epoch,
                seq,
                tick,
                phase,
            } if epoch == expected_epoch
                && seq == expected_seq
                && clock::fresh(tick, now, FRESH_MS) =>
            {
                Some(phase)
            }
            _ => None,
        }
    }
}

pub struct Control {
    epoch: u64,
    seq: u64,
    tick: u64,
    deadline: u64,
    phase: Phase,
}

impl Control {
    /// Bootstrap uses the most recent request already authenticated by installed policy.
    pub fn new(epoch: u64, authorized_tick: u64, now: u64) -> Option<Self> {
        let deadline = authorized_tick.checked_add(LEASE_MS)?;
        (epoch != 0 && authorized_tick <= now && now < deadline).then_some(Self {
            epoch,
            seq: 0,
            tick: authorized_tick,
            deadline,
            phase: Phase::Active,
        })
    }

    pub fn deadline(&self) -> u64 {
        self.deadline
    }
    pub fn accept(&mut self, frame: Frame, now: u64, mut backend: Backend) -> Option<Frame> {
        let Frame::Command {
            epoch,
            seq,
            tick,
            command,
        } = frame
        else {
            return None;
        };
        if self.phase == Phase::Closed
            || now >= self.deadline
            || epoch != self.epoch
            || seq <= self.seq
            || tick < self.tick
            || !now.checked_sub(tick).is_some_and(|age| age < LEASE_MS)
        {
            return None;
        }
        if command == Command::Heartbeat {
            self.deadline = tick.checked_add(LEASE_MS)?;
        }
        self.seq = seq;
        self.tick = tick;
        match command {
            Command::Heartbeat => {}
            Command::Pause => self.phase = Phase::Paused,
            Command::Exit => self.phase = Phase::Closed,
        }
        if command != Command::Heartbeat {
            return Some(Frame::Ack {
                epoch,
                seq,
                tick: now,
                phase: self.phase,
            });
        }
        backend.ready &= self.phase == Phase::Active;
        backend.inventory_confirmed &= backend.ready;
        Some(Frame::Ready {
            epoch,
            seq,
            tick: now,
            reply: Reply {
                phase: self.phase,
                backend,
            },
        })
    }
}
