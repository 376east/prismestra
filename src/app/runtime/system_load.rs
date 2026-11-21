use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetSystemTimes};

const REFRESH_US: u64 = 750_000;

#[derive(Clone, Copy)]
struct Times {
    idle: u64,
    kernel: u64,
    user: u64,
}

impl Times {
    fn delta(self, previous: Self) -> Option<f32> {
        let idle = self.idle.checked_sub(previous.idle)?;
        let kernel = self.kernel.checked_sub(previous.kernel)?;
        let user = self.user.checked_sub(previous.user)?;
        let total = kernel.checked_add(user)?;
        let busy = total.checked_sub(idle)?;
        (total != 0).then(|| (busy as f64 / total as f64) as f32)
    }
}

fn times() -> Option<Times> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let ticks =
        |value: FILETIME| (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime);
    Some(Times {
        idle: ticks(idle),
        kernel: ticks(kernel),
        user: ticks(user),
    })
}

#[derive(Default)]
pub(super) struct Load {
    next: u64,
    previous: Option<Times>,
    cached: Option<(u64, f32)>,
}

impl Load {
    pub(super) fn poll(&mut self, now_us: u64) {
        if now_us < self.next {
            return;
        }
        self.next = now_us.saturating_add(REFRESH_US);
        let current = times();
        self.cached = current
            .zip(self.previous)
            .and_then(|(current, previous)| current.delta(previous))
            .map(|load| (now_us, load));
        self.previous = current;
    }

    pub(super) fn cached(&self, now_us: u64) -> Option<f32> {
        self.cached
            .filter(|(stamp, _)| now_us.saturating_sub(*stamp) <= 2 * REFRESH_US)
            .map(|(_, load)| load)
    }
}

#[test]
fn cpu_load_subtracts_idle_time_from_kernel_and_user() {
    let first = Times {
        idle: 20,
        kernel: 50,
        user: 30,
    };
    let second = Times {
        idle: 50,
        kernel: 90,
        user: 40,
    };
    assert_eq!(second.delta(first), Some(0.4));
    assert_eq!(first.delta(second), None);
    assert_eq!(first.delta(first), None);
}
