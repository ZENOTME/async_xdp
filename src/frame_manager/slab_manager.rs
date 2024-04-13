use std::sync::Arc;

use std::sync::Mutex;

use itertools::Itertools;
use xsk_rs::FrameDesc;

use crate::FrameFreeHandle;

use super::{FrameHandle, FrameManager};

/// Configuration for the SlabManager.
#[derive(Clone, Debug)]
pub struct SlabManagerConfig {
    slab_size: usize,
}

impl SlabManagerConfig {
    /// Create a new SlabManagerConfig.
    pub fn new(slab_size: usize) -> Self {
        Self { slab_size }
    }
}

impl Default for SlabManagerConfig {
    fn default() -> Self {
        Self::new(4096)
    }
}

#[derive(Clone, Debug)]
/// A manager that uses a slab allocator to manage the umem memory region.
pub struct SlabManager {
    config: SlabManagerConfig,
    available: Arc<Mutex<Vec<Vec<FrameDesc>>>>,
    free: Arc<Mutex<Vec<FrameDesc>>>,
}

impl SlabManager {
    fn alloc_one_slab(&self) -> anyhow::Result<Vec<FrameDesc>> {
        let mut available = self
            .available
            .lock()
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        if let Some(new_available) = available.pop() {
            return Ok(new_available);
        }
        Err(anyhow::anyhow!("no more available slab"))
    }

    fn free_one_slab(&self, desc: Vec<FrameDesc>) -> anyhow::Result<()> {
        assert!(
            desc.len() <= self.config.slab_size,
            "desc.len() = {}, slab_size = {}",
            desc.len(),
            self.config.slab_size
        );
        let mut available = self
            .available
            .lock()
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        available.push(desc);
        Ok(())
    }

    fn free_one_frame(&self, frame: FrameDesc) -> anyhow::Result<()> {
        let mut free = self.free.lock().map_err(|err| anyhow::anyhow!("{err}"))?;
        free.push(frame);
        if free.len() == self.config.slab_size {
            let free = std::mem::replace(&mut *free, Vec::with_capacity(self.config.slab_size));
            self.free_one_slab(free)?;
        }
        Ok(())
    }
}

impl FrameManager for SlabManager {
    type T = SlabHandle;
    type C = SlabManagerConfig;
    type F = SlabFree;

    fn new(config: Self::C, frames: Vec<FrameDesc>) -> anyhow::Result<Self> {
        let available: Vec<Vec<FrameDesc>> = frames
            .into_iter()
            .chunks(config.slab_size)
            .into_iter()
            .map(|chunk| chunk.collect())
            .collect();
        Ok(Self {
            config,
            available: Arc::new(Mutex::new(available)),
            free: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn handle(&self) -> anyhow::Result<Self::T> {
        Ok(SlabHandle {
            manager: self.clone(),
            available: Vec::with_capacity(self.config.slab_size),
            free: Vec::with_capacity(self.config.slab_size),
        })
    }

    fn free_handle(&self) -> Self::F {
        SlabFree {
            manager: self.clone(),
        }
    }
}

/// A free handle for the SlabManager.
pub struct SlabFree {
    manager: SlabManager,
}

impl FrameFreeHandle for SlabFree {
    fn free(&self, frame: FrameDesc) -> anyhow::Result<()> {
        self.manager.free_one_frame(frame)
    }

    fn clone_box(&self) -> Box<dyn FrameFreeHandle> {
        Box::new(Self {
            manager: self.manager.clone(),
        })
    }
}

/// A slab handle used to allocate, free frames.
#[derive(Clone, Debug)]
pub struct SlabHandle {
    manager: SlabManager,
    available: Vec<FrameDesc>,
    free: Vec<FrameDesc>,
}

impl FrameHandle for SlabHandle {
    fn reserve(&mut self, count: usize) -> anyhow::Result<usize> {
        while self.available.len() < count {
            if let Ok(slab) = self.manager.alloc_one_slab() {
                self.available.extend(slab);
            } else {
                return Ok(self.available.len());
            }
        }

        Ok(count)
    }

    fn alloc_one(&mut self) -> anyhow::Result<FrameDesc> {
        if let Some(frame) = self.available.pop() {
            Ok(frame)
        } else {
            let slab = self.manager.alloc_one_slab()?;
            self.available.extend(slab);
            Ok(self.alloc_one().unwrap())
        }
    }

    fn free<I>(&mut self, frames: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = FrameDesc>,
    {
        let mut iter = frames.into_iter().map(|mut frame| {
            frame.reset();
            frame
        });
        while self.free.len() < self.manager.config.slab_size {
            if let Some(frame) = iter.next() {
                self.free.push(frame);
            } else {
                return Ok(());
            }
        }
        self.manager.free_one_slab(std::mem::replace(
            &mut self.free,
            Vec::with_capacity(self.manager.config.slab_size),
        ))?;
        for frames in iter
            .chunks(self.manager.config.slab_size)
            .into_iter()
            .map(|frames| frames.collect::<Vec<_>>())
        {
            if frames.len() < self.manager.config.slab_size {
                // must be the last frames, we don't need to free it to the manager.
                self.free.extend(frames);
            } else {
                self.manager.free_one_slab(frames)?;
            }
        }
        Ok(())
    }

    fn statistics(&self) -> super::FrameStatistics {
        super::FrameStatistics {
            total_available: self
                .manager
                .available
                .lock()
                .unwrap()
                .iter()
                .map(|slab| slab.len())
                .sum::<usize>()
                + self.manager.free.lock().unwrap().len()
                + self.free.len()
                + self.available.len(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_succeed() {
        let config = SlabManagerConfig::new(4096);
        let frames = vec![FrameDesc::default(); 4096 * 16];
        let manager = SlabManager::new(config, frames).unwrap();

        let mut handle1 = manager.handle().unwrap();
        assert_eq!(handle1.reserve(4096 * 16).unwrap(), 4096 * 16);
        let mut frames1 = Vec::with_capacity(4096 * 16);
        for _ in 0..4096 * 16 {
            frames1.push(handle1.alloc_one().unwrap());
        }

        let mut handle2 = manager.handle().unwrap();
        assert!(handle2.alloc_one().is_err());

        handle2.free(frames1).unwrap();
        let _frame = handle2.alloc_one().unwrap();

        let mut handle3 = manager.handle().unwrap();
        assert_eq!(handle3.reserve(4096 * 16).unwrap(), 4096 * 15);
        assert!(handle1.alloc_one().is_err());
    }
}
