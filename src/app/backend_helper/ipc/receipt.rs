use crate::app::backend_diagnostics::queue_windows_event;

const WINDOW_MS: u64 = 60_000;

#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub(super) seq: u64,
    pub(super) tick: u64,
    pub(super) observed: u64,
    pub(super) backend: u32,
    pub(super) manager_generation: u64,
}

struct Window {
    first: Sample,
    last: Sample,
    count: u32,
    max_gap: u64,
    max_age: u64,
}

#[derive(Default)]
pub(super) struct Receipt {
    window: Option<Window>,
    last_admitted_generation: Option<u64>,
    wrote_window: bool,
}

pub(super) fn event(kind: &str, epoch: u64, helper: u32, detail: std::fmt::Arguments<'_>) {
    if !cfg!(test) {
        queue_windows_event(
            3,
            format!("event={kind} epoch={epoch} helper_pid={helper} {detail}"),
        );
    }
}

impl Receipt {
    fn admit(&mut self, resident_generation: u64) -> Option<Admission> {
        match self.last_admitted_generation.replace(resident_generation) {
            None => Some(Admission::Initial),
            Some(previous) if previous != resident_generation => Some(Admission::Transition),
            Some(_) => None,
        }
    }

    pub(super) fn ready(
        &mut self,
        epoch: u64,
        helper: u32,
        sample: Sample,
        resident_generation: u64,
    ) {
        let age = sample.observed - sample.tick;
        match self.admit(resident_generation) {
            Some(Admission::Initial) => event(
                "ipc_ready_admitted",
                epoch,
                helper,
                format_args!(
                    "seq={} tick_ms={} observed_ms={} age_ms={age} backend_pid={} manager_generation={}",
                    sample.seq,
                    sample.tick,
                    sample.observed,
                    sample.backend,
                    sample.manager_generation,
                ),
            ),
            Some(Admission::Transition) if !cfg!(test) => {
                queue_windows_event(
                    3,
                    transition_message(epoch, helper, sample, resident_generation),
                );
            }
            Some(Admission::Transition) | None => {}
        }
        if self.wrote_window {
            return;
        }
        if let Some(window) = &mut self.window
            && window.last.backend == sample.backend
            && window.last.manager_generation == sample.manager_generation
            && sample.observed.saturating_sub(window.last.tick) <= super::super::FRESH_MS
        {
            window.max_gap = window.max_gap.max(sample.observed - window.last.tick);
            window.max_age = window.max_age.max(age);
            window.last = sample;
            window.count = window.count.saturating_add(1);
            if sample.observed - window.first.observed >= WINDOW_MS {
                self.finish(epoch, helper);
            }
        } else {
            self.window = Some(Window {
                first: sample,
                last: sample,
                count: 1,
                max_gap: 0,
                max_age: age,
            });
        }
    }

    pub(super) fn finish(&mut self, epoch: u64, helper: u32) {
        if let Some(window) = self.window.take() {
            self.wrote_window = true;
            if !cfg!(test) {
                queue_windows_event(3, window.message(epoch, helper));
            }
        }
    }

    pub(super) fn not_ready(&mut self) {
        self.window = None;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Initial,
    Transition,
}

fn transition_message(epoch: u64, helper: u32, sample: Sample, resident_generation: u64) -> String {
    // This is the resident proxy's publication. Runtime has not necessarily sent a frame.
    // Hexadecimal fields leave room for the Event Log queue-loss counter.
    format!(
        "event=ipc_ready_transition epoch={epoch} helper_pid={helper} seq={:x} tick={:x} observed={:x} backend={:x} manager_gen={:x} resident_gen={resident_generation:x}",
        sample.seq, sample.tick, sample.observed, sample.backend, sample.manager_generation,
    )
}

impl Window {
    // Event Log bounds messages at 256 chars, also after appending queue loss.
    // The positions, sequence numbers and identities use hex.
    fn message(&self, epoch: u64, helper: u32) -> String {
        format!(
            "event=ipc_ready_window e={epoch:x} h={helper:x} b={:x} s={:x}:{:x} t={:x}:{:x} o={:x}:{:x} n={:x} g={} a={} m={:x}",
            self.last.backend,
            self.first.seq,
            self.last.seq,
            self.first.tick,
            self.last.tick,
            self.first.observed,
            self.last.observed,
            self.count,
            self.max_gap,
            self.max_age,
            self.last.manager_generation,
        )
    }
}

#[test]
fn receipt_summarizes_one_continuous_window_per_session() {
    let mut receipt = Receipt::default();
    for (seq, observed) in [(1, 200), (2, 1_000), (3, 5_000), (4, 5_900)] {
        receipt.ready(
            1,
            2,
            Sample {
                seq,
                tick: observed - 40,
                observed,
                backend: 3,
                manager_generation: 1,
            },
            1,
        );
    }
    let window = receipt.window.as_ref().unwrap();
    assert_eq!((window.first.seq, window.last.seq, window.count), (3, 4, 2));
    assert_eq!((window.max_gap, window.max_age), (940, 40));
    receipt.finish(1, 2);
    receipt.ready(
        1,
        2,
        Sample {
            seq: 5,
            tick: 6_000,
            observed: 6_010,
            backend: 3,
            manager_generation: 1,
        },
        1,
    );
    assert_eq!(receipt.last_admitted_generation, Some(1));
    assert!(receipt.wrote_window);
    assert!(receipt.window.is_none());
}

#[test]
fn receipt_breaks_before_a_new_reply_if_the_previous_ready_already_expired() {
    let mut receipt = Receipt::default();
    for (seq, tick, observed) in [(1, 100, 2_090), (2, 2_140, 2_150)] {
        receipt.ready(
            1,
            2,
            Sample {
                seq,
                tick,
                observed,
                backend: 3,
                manager_generation: 1,
            },
            seq,
        );
    }
    let window = receipt.window.as_ref().unwrap();
    assert_eq!((window.first.seq, window.last.seq, window.count), (2, 2, 1));
    receipt.not_ready();
    assert!(receipt.window.is_none());
    assert!(!receipt.wrote_window);
}

#[test]
fn admission_follows_the_published_generation_without_heartbeat_duplicates() {
    use super::super::ReadyGeneration;

    let mut generation = ReadyGeneration::new();
    let mut receipt = Receipt::default();
    for (tick, backend, expected) in [
        (100, Some((42, 1)), Some(Admission::Initial)),
        (110, Some((42, 1)), None),
        (120, None, None),
        (130, Some((42, 1)), Some(Admission::Transition)),
        (140, Some((42, 2)), Some(Admission::Transition)),
        (150, Some((43, 2)), Some(Admission::Transition)),
        (160, Some((43, 2)), None),
    ] {
        let actual = if let Some(backend) = backend {
            generation.observe(backend, tick, tick);
            receipt.admit(generation.current)
        } else {
            generation.previous = None;
            receipt.not_ready();
            None
        };
        assert_eq!(actual, expected, "tick {tick}");
    }
}

#[test]
fn transition_record_fits_event_log_with_the_queue_loss_counter() {
    let sample = Sample {
        seq: u64::MAX,
        tick: u64::MAX,
        observed: u64::MAX,
        backend: u32::MAX,
        manager_generation: u64::MAX,
    };
    let message = transition_message(u64::MAX, u32::MAX, sample, u64::MAX);
    assert!(message.is_ascii());
    assert!(format!("{message} queue_dropped={}", u64::MAX).len() <= 256);
}

#[test]
fn window_record_fits_existing_event_log_limit_even_with_queue_loss() {
    let sample = Sample {
        seq: u64::MAX,
        tick: u64::MAX,
        observed: u64::MAX,
        backend: u32::MAX,
        manager_generation: u64::MAX,
    };
    let window = Window {
        first: sample,
        last: sample,
        count: u32::MAX,
        max_gap: super::super::FRESH_MS,
        max_age: super::super::FRESH_MS,
    };
    assert!(
        format!(
            "{} queue_dropped={}",
            window.message(u64::MAX, u32::MAX),
            u64::MAX
        )
        .len()
            <= 256
    );
}
