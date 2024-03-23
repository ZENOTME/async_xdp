use xsk_rs::{FrameDesc, Umem};

use crate::FrameFreeHandle;

/// A Frame represents a frame in the UMEM.
/// It's aim to make the frame be used in a more simple and safe way.
/// When ownership of the frame is user, we used the Drop trait to gurantee that the frame will be freed safely.
/// When ownership return back to the manager, we call `take_desc` to extract the frame desc and the frame will be drop without free.
pub struct Frame {
    desc: Option<FrameDesc>,
    free: Box<dyn FrameFreeHandle>,
    umem: Umem,
}

impl Frame {
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

impl AsRef<[u8]> for Frame {
    fn as_ref(&self) -> &[u8] {
        let (ptr, len) = unsafe {
            let data = self.umem.data(
                &self
                    .desc
                    .expect("Gurarantee the frame is valid util return by to context"),
            );
            (data.contents().as_ptr(), data.len())
        };
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

impl AsMut<[u8]> for Frame {
    fn as_mut(&mut self) -> &mut [u8] {
        let (ptr, len) = unsafe {
            let mut data = self.umem.data_mut(
                self.desc
                    .as_mut()
                    .expect("Gurarantee the frame is valid util return by to context"),
            );
            (data.contents_mut().as_mut_ptr(), data.len())
        };
        unsafe { std::slice::from_raw_parts_mut(ptr, len) }
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
