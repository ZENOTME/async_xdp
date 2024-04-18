use xsk_rs::{
    umem::frame::{Data, DataMut},
    FrameDesc, Umem,
};

use crate::FrameFreeHandle;

/// A Frame represents a frame in the UMEM.
/// It's aim to make the frame be used in a more simple and safe way.
/// When ownership of the frame is user, we used the Drop trait to guarantee that the frame will be freed safely.
/// When ownership return back to the manager, we call `take_desc` to extract the frame desc and the frame will be drop without free.
pub struct Frame {
    desc: Option<FrameDesc>,
    free: Box<dyn FrameFreeHandle>,
    umem: Umem,
}

impl Frame {
    /// Get the frame desc.
    pub fn desc(&self) -> &FrameDesc {
        self.desc
            .as_ref()
            .expect("Gurarantee the frame is valid util return by to context")
    }

    /// Create a new Frame.
    pub(crate) unsafe fn new(desc: FrameDesc, umem: &Umem, free: Box<dyn FrameFreeHandle>) -> Self {
        Self {
            desc: Some(desc),
            umem: umem.clone(),
            free,
        }
    }

    pub(crate) unsafe fn take_desc(&mut self) -> FrameDesc {
        self.desc
            .take()
            .expect("Gurarantee the frame is valid util return by to context")
    }
}

impl Frame {
    /// Adjust the frame address ahead to frame headroom.
    /// # NOTE:
    /// If offset out of the frame headroom, it panic.
    pub fn adjust_head(&mut self, offset: i32) {
        self.umem.adjust_addr(
            self.desc
                .as_mut()
                .expect("Gurarantee the frame is valid util return by to context"),
            offset,
        )
    }

    /// Return the data of the frame.
    pub fn data_ref(&self) -> Data {
        unsafe {
            self.umem.data(
                &self
                    .desc
                    .expect("Gurarantee the frame is valid util return by to context"),
            )
        }
    }

    /// Return the mutable data of the frame.
    pub fn data_mut(&mut self) -> DataMut {
        unsafe {
            self.umem.data_mut(
                self.desc
                    .as_mut()
                    .expect("Gurarantee the frame is valid util return by to context"),
            )
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        if let Some(desc) = self.desc.take() {
            if let Err(err) = self.free.free(desc) {
                log::error!("free frame failed: {:?}", err);
            }
        }
    }
}
