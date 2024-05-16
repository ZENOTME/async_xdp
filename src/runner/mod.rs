mod poller;
use std::sync::{atomic::AtomicUsize, Arc};

pub use poller::*;
use std::thread;

use core_affinity::CoreId;

/// JoinHandle a runner return when a poller is added.
pub type JoinHandle<T> = std::thread::JoinHandle<T>;

/// PollerRunner is a trait that provides a way to run pollers.
pub trait PollerRunner: Send + Sync + 'static {
    /// Add a poller to the runner.
    fn add_poller(&self, poller: Box<dyn Poller>) -> anyhow::Result<JoinHandle<()>>;
}

impl PollerRunner for Box<dyn PollerRunner> {
    fn add_poller(&self, poller: Box<dyn Poller>) -> anyhow::Result<JoinHandle<()>> {
        self.as_ref().add_poller(poller)
    }
}

/// SingleThreadRunner is a simple runner to run each poller in a single thread. It will bind each poller to a different core in Round Robin way.
#[derive(Clone)]
pub struct SingleThreadRunner {}

impl SingleThreadRunner {
    /// Create a new SingleThreadRunner.
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for SingleThreadRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl PollerRunner for SingleThreadRunner {
    fn add_poller(&self, mut poller: Box<dyn Poller>) -> anyhow::Result<JoinHandle<()>> {
        let handle = thread::spawn(move || {
            poller.init().unwrap();
            poller.run().unwrap();
        });
        Ok(handle)
    }
}

/// Bind to a specific core runner.
#[derive(Clone)]
pub struct BindCoreRunner {
    core_ids: Vec<CoreId>,
    next_core_id: Arc<AtomicUsize>,
}

impl BindCoreRunner {
    /// Create a new SingleThreadRunner.
    pub fn new() -> Self {
        let mut core_ids = core_affinity::get_core_ids().unwrap();
        // Make sure the number of cores is even.
        if core_ids.len() % 2 != 0 {
            core_ids = core_ids[0..core_ids.len() - 1].to_vec();
        }
        Self {
            core_ids,
            next_core_id: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Default for BindCoreRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl PollerRunner for BindCoreRunner {
    fn add_poller(&self, mut poller: Box<dyn Poller>) -> anyhow::Result<JoinHandle<()>> {
        let current = self
            .next_core_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let core_id = self.core_ids[current];
        let handle = thread::spawn(move || {
            core_affinity::set_for_current(core_id);
            poller.init().unwrap();
            poller.run().unwrap();
        });
        Ok(handle)
    }
}

#[cfg(test)]
mod test {
    fn assert_object_safe(_: &dyn super::PollerRunner) {}
    fn pass_poller_runner<R: super::PollerRunner>(_runner: &R) {}
    fn assert_box_trait(runner: Box<dyn super::PollerRunner>) {
        pass_poller_runner(&runner);
    }

    #[test]
    fn ensure_static_property() {
        let runner = Box::new(super::SingleThreadRunner::new());
        assert_object_safe(&*runner);
        assert_box_trait(runner);
    }
}
