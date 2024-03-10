mod poller;
use std::sync::{atomic::AtomicUsize, Arc};

pub use poller::*;
use std::thread;

use core_affinity::CoreId;

/// JoinHandle a runner return when a poller is added.
pub type JoinHandle<T> = std::thread::JoinHandle<T>;

/// PollerRunner is a trait that provides a way to run pollers.
pub trait PollerRunner: Clone + Send + Sync + 'static {
    /// Add a poller to the runner.
    fn add_poller<T: Poller>(&self, poller: T) -> anyhow::Result<JoinHandle<()>>;
}

/// SingleThreadRunner is a simple runner to run each poller in a single thread. It will bind each poller to a different core in Round Robin way.
#[derive(Clone)]
pub struct SingleThreadRunner {
    core_ids: Vec<CoreId>,
    next_core_id: Arc<AtomicUsize>,
}

impl SingleThreadRunner {
    /// Create a new SingleThreadRunner.
    pub fn new() -> Self {
        let mut core_ids = core_affinity::get_core_ids().unwrap();
        // Make sure the number of cores is even.
        if core_ids.len() % 2 != 0 {
            core_ids = core_ids[0..core_ids.len() - 1].to_vec();
        }
        Self {
            core_ids,
            next_core_id: Arc::new(AtomicUsize::new(10)),
        }
    }
}

impl Default for SingleThreadRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl PollerRunner for SingleThreadRunner {
    fn add_poller<T: Poller>(&self, mut poller: T) -> anyhow::Result<JoinHandle<()>> {
        let current = self
            .next_core_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let core_id = self.core_ids[current];
        let handle = thread::spawn(move || {
            core_affinity::set_for_current(core_id);
            poller.init().unwrap();
            loop {
                if let Err(err) = poller.run_once() {
                    log::error!("Poller run_once failed: {:?}", err);
                    break;
                }
            }
        });
        Ok(handle)
    }
}
