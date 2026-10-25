//! Touch panel polled on its own thread, so drags are tracked smoothly even
//! while the main task is busy generating text.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use esp_idf_svc::sys;
use m5_common::tab5_ui::{Swipe, Tap};

extern "C" {
    fn tab5_touch_init() -> sys::esp_err_t;
    fn tab5_touch_read(x: *mut u16, y: *mut u16) -> bool;
}

const POLL: Duration = Duration::from_millis(20);

pub struct Touch {
    /// Lines of scrolling not yet applied.
    lines: Arc<AtomicI32>,
    /// Last tap not yet taken, as `x << 16 | y`, plus one; 0 for none.
    tap: Arc<AtomicU32>,
}

impl Touch {
    pub fn start() -> anyhow::Result<Self> {
        sys::esp!(unsafe { tab5_touch_init() })?;
        let lines = Arc::new(AtomicI32::new(0));
        let tap = Arc::new(AtomicU32::new(0));
        let (l, t) = (lines.clone(), tap.clone());
        std::thread::Builder::new().name("touch".into()).stack_size(4096).spawn(move || {
            let mut swipe = Swipe::default();
            let mut taps = Tap::default();
            loop {
                let (mut x, mut y) = (0u16, 0u16);
                let p = unsafe { tab5_touch_read(&mut x, &mut y) }.then_some((x as i32, y as i32));
                l.fetch_add(swipe.update(p), Ordering::Relaxed);
                if let Some((x, y)) = taps.update(p) {
                    t.store(((x as u32) << 16 | y as u32) + 1, Ordering::Relaxed);
                }
                std::thread::sleep(POLL);
            }
        })?;
        Ok(Self { lines, tap })
    }

    /// Scrolling accumulated since the last call.
    pub fn take_lines(&self) -> i32 {
        self.lines.swap(0, Ordering::Relaxed)
    }

    /// The last tap since the previous call, if any.
    pub fn take_tap(&self) -> Option<(i32, i32)> {
        let v = self.tap.swap(0, Ordering::Relaxed).checked_sub(1)?;
        Some(((v >> 16) as i32, (v & 0xffff) as i32))
    }
}
