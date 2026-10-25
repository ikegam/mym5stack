//! A long-lived second thread that takes half of every matmul.
//!
//! Spawning a thread per matmul (~40 per token) is slow on ESP-IDF and
//! fragments internal RAM until task creation fails, so the worker is
//! created once and handed borrowed jobs.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

type Job = *mut (dyn FnMut() + Send + 'static);

struct SendJob(Job);
// SAFETY: the closure itself is Send; the pointer is only used while the
// caller of `run` blocks waiting for it.
unsafe impl Send for SendJob {}

#[derive(Default)]
struct Slot {
    job: Option<SendJob>,
    done: bool,
    quit: bool,
}

type Shared = Arc<(Mutex<Slot>, Condvar)>;

pub struct Helper {
    shared: Shared,
    thread: Option<JoinHandle<()>>,
}

impl Helper {
    pub fn new() -> Self {
        let shared: Shared = Arc::default();
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("llm-helper".into())
            .stack_size(8 * 1024)
            .spawn(move || worker(s))
            .expect("spawn llm helper");
        Self { shared, thread: Some(thread) }
    }

    /// Runs `theirs` on the helper and `mine` here, returning when both are done.
    pub fn run(&self, theirs: &mut (dyn FnMut() + Send), mine: impl FnOnce()) {
        // SAFETY: erasing the lifetime is sound because `Wait` blocks until
        // the worker has finished with the job, even if `mine` panics.
        let job: Job = unsafe { std::mem::transmute::<*mut (dyn FnMut() + Send + '_), Job>(theirs) };
        let (lock, cv) = &*self.shared;
        {
            let mut slot = lock.lock().unwrap();
            slot.job = Some(SendJob(job));
            slot.done = false;
        }
        cv.notify_all();
        let _wait = Wait(&self.shared);
        mine();
    }
}

impl Default for Helper {
    fn default() -> Self {
        Self::new()
    }
}

struct Wait<'a>(&'a Shared);

impl Drop for Wait<'_> {
    fn drop(&mut self) {
        let (lock, cv) = &**self.0;
        let mut slot = lock.lock().unwrap_or_else(|e| e.into_inner());
        while !slot.done {
            slot = cv.wait(slot).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).quit = true;
        cv.notify_all();
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

fn worker(shared: Shared) {
    let (lock, cv) = &*shared;
    loop {
        let job = {
            let mut slot = lock.lock().unwrap();
            loop {
                if slot.quit {
                    return;
                }
                if let Some(job) = slot.job.take() {
                    break job;
                }
                slot = cv.wait(slot).unwrap();
            }
        };
        // SAFETY: see `Helper::run`.
        unsafe { (*job.0)() };
        lock.lock().unwrap().done = true;
        cv.notify_all();
    }
}
