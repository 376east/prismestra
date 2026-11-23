use std::time::Duration;

const SLOW_MS: u64 = 1_000;

#[derive(Clone, Copy)]
pub(super) enum Stage {
    Identity,
    ReadOwner,
    ReadCancel,
    ReadPause,
    WriteOwner,
    WriteStatus,
    Journal,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Sample {
    pub(super) ended_ms: u64,
    pub(super) elapsed_ms: u64,
    pub(super) wake_ms: u64,
    pub(super) stages: [u32; 7],
}

pub(super) struct Trace<'a> {
    clock: &'a dyn Fn() -> Option<u64>,
    report: &'a dyn Fn(Sample),
    wake_due: Option<u64>,
}

impl<'a> Trace<'a> {
    pub(super) fn new(clock: &'a dyn Fn() -> Option<u64>, report: &'a dyn Fn(Sample)) -> Self {
        Self {
            clock,
            report,
            wake_due: None,
        }
    }

    pub(super) fn begin(&mut self, enabled: bool) -> Tick<'_, 'a> {
        let start = if enabled { (self.clock)() } else { None };
        let wake_ms = start
            .zip(self.wake_due.take())
            .map_or(0, |(now, due)| now.saturating_sub(due));
        Tick {
            trace: self,
            start,
            wake_ms,
            stages: [0; 7],
        }
    }
}

pub(super) struct Tick<'trace, 'callbacks> {
    trace: &'trace mut Trace<'callbacks>,
    start: Option<u64>,
    wake_ms: u64,
    stages: [u32; 7],
}

impl Tick<'_, '_> {
    pub(super) fn measure<T>(&mut self, stage: Stage, operation: impl FnOnce() -> T) -> T {
        let before = self.start.and_then(|_| (self.trace.clock)());
        let result = operation();
        if let Some((before, after)) = before.zip(before.and_then(|_| (self.trace.clock)()))
            && let Some(elapsed) = after.checked_sub(before)
        {
            let slot = &mut self.stages[stage as usize];
            *slot = slot.saturating_add(elapsed.min(u32::MAX as u64) as u32);
        }
        result
    }

    pub(super) fn sleep(mut self, duration: Duration, wait: impl FnOnce(Duration)) {
        let enabled = self.start.is_some();
        self.finish();
        self.trace.wake_due = if enabled {
            (self.trace.clock)().and_then(|now| now.checked_add(duration.as_millis() as u64))
        } else {
            None
        };
        wait(duration);
    }

    fn finish(&mut self) {
        let Some(start) = self.start.take() else {
            return;
        };
        let Some(ended_ms) = (self.trace.clock)() else {
            return;
        };
        let Some(elapsed_ms) = ended_ms.checked_sub(start) else {
            return;
        };
        if self.wake_ms.saturating_add(elapsed_ms) >= SLOW_MS {
            (self.trace.report)(Sample {
                ended_ms,
                elapsed_ms,
                wake_ms: self.wake_ms,
                stages: self.stages,
            });
        }
    }
}

impl Drop for Tick<'_, '_> {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn wake_lateness_excludes_requested_sleep_and_slow_io_keeps_its_stage() {
        let now = Cell::new(0);
        let events = RefCell::new(Vec::new());
        let clock = || Some(now.get());
        let report = |event| events.borrow_mut().push(event);
        let mut trace = Trace::new(&clock, &report);
        trace
            .begin(true)
            .sleep(Duration::from_millis(200), |_| now.set(20_200));
        drop(trace.begin(true));
        for stage in [
            Stage::Identity,
            Stage::ReadOwner,
            Stage::ReadCancel,
            Stage::ReadPause,
            Stage::WriteOwner,
            Stage::WriteStatus,
            Stage::Journal,
        ] {
            let mut tick = trace.begin(true);
            assert_eq!(
                tick.measure(stage, || {
                    now.set(now.get() + 26_000);
                    17
                }),
                17
            );
        }
        let events = events.into_inner();
        assert_eq!(events.len(), 8);
        assert_eq!(
            events[0],
            Sample {
                ended_ms: 20_200,
                elapsed_ms: 0,
                wake_ms: 20_000,
                stages: [0; 7]
            }
        );
        for (stage, event) in events[1..].iter().enumerate() {
            let mut expected = [0; 7];
            expected[stage] = 26_000;
            assert_eq!(event.stages, expected);
            assert_eq!(event.wake_ms, 0);
            assert_eq!(event.elapsed_ms, 26_000);
        }
    }

    #[test]
    fn normal_timeout_disabled_trace_and_missing_clock_are_quiet() {
        let now = Cell::new(Some(0));
        let clock = || now.get();
        let report = |_| panic!("unexpected Windows Application sample");
        let mut trace = Trace::new(&clock, &report);
        trace
            .begin(true)
            .sleep(Duration::from_millis(200), |_| now.set(Some(200)));
        let mut tick = trace.begin(true);
        tick.measure(Stage::ReadOwner, || now.set(Some(201)));
        drop(tick);
        let mut tick = trace.begin(false);
        tick.measure(Stage::WriteOwner, || now.set(Some(20_201)));
        drop(tick);
        let mut tick = trace.begin(true);
        tick.measure(Stage::ReadCancel, || now.set(None));
    }
}
