//! This module provides a way to manage the umem memory frame.

mod slab_manager;
pub use slab_manager::*;

use xsk_rs::FrameDesc;

/// FrameStatistics provides the statistics for the number of frames.
#[derive(Debug)]
pub struct FrameStatistics {
    /// Total number of available frames.
    pub total_available: usize,
}

/// FrameHandle is a trait that provides a way to allocate and free frames.
pub trait FrameHandle: Send + 'static {
    /// Reserve frames.
    fn reserve(&mut self, count: usize) -> anyhow::Result<usize>;
    /// Allocate frames. Call reserve before calling this function.
    fn alloc_one(&mut self) -> anyhow::Result<FrameDesc>;
    /// Free frames.
    fn free<I>(&mut self, frames: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = FrameDesc>;
    /// Statics for the number of frames.
    fn statistics(&self) -> FrameStatistics;
}

/// FrameManager is a trait that provides a way to manager the umem memory frame.
/// It provides a way to return a handle to allocate, free frames. The handle needs to be
/// concurrent safe.
pub trait FrameManager: Clone + Send + 'static {
    /// Handle type.
    type T: FrameHandle;
    /// Config type.
    type C: Default;
    /// Global free handle.
    type F: FrameFreeHandle;
    /// Create a new UmemRegionManager.
    fn new(config: Self::C, frames: Vec<FrameDesc>) -> anyhow::Result<Self>;
    /// Return a handle used to allocate, free frames.
    fn handle(&self) -> anyhow::Result<Self::T>;
    /// Return a global free handle.
    fn free_handle(&self) -> Self::F;
}

/// FrameFreeHandle is a trait that provides a way to free frames. This trait is used in `Drop` to Frame so it
/// must guarantee that the free to manager directly.
pub trait FrameFreeHandle: Sync + Send + 'static {
    /// Free a frame to manager directly.
    fn free(&self, frame: FrameDesc) -> anyhow::Result<()>;
    /// Clone for trait object.
    fn clone_box(&self) -> Box<dyn FrameFreeHandle>;
}
