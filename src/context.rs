use std::{
    num::NonZeroU32,
    str::FromStr,
    sync::{Arc, Mutex},
};

use smallvec::SmallVec;
use xsk_rs::{
    config::{Interface, SocketConfig, UmemConfig},
    FrameDesc, Socket, Umem,
};

use crate::{
    FrameManager, JoinHandle, MpScSender, PollerRunner, SpScReceiver, XdpPoller, XdpReceiveMsg,
    XdpSendMsg, BATCH_SIZSE,
};

enum UmemConfigState<T: FrameManager> {
    Init,
    Config {
        umem_config: UmemConfig,
        manager_config: T::C,
        frame_count: NonZeroU32,
    },
    Umem {
        umem: Umem,
        frame_manager: T,
    },
}

/// XdpContextBuilder is used to build the XdpContext.
pub struct XdpContextBuilder<T: FrameManager> {
    config: SocketConfig,
    if_name: String,
    queue_id: u32,
    umem_state: UmemConfigState<T>,
    use_huge_pages: bool,
}

impl<T: FrameManager> XdpContextBuilder<T> {
    /// Create a new builder.
    pub fn new(if_name: &str, queue_id: u32) -> Self {
        Self {
            config: SocketConfig::default(),
            if_name: if_name.to_string(),
            queue_id,
            umem_state: UmemConfigState::Init,
            use_huge_pages: false,
        }
    }

    /// Set the socket config.
    pub fn with_socket_config(&mut self, config: SocketConfig) -> &mut Self {
        self.config = config;
        self
    }

    /// Set the umem config. The xdp context will create a new umem using the config.
    pub fn with_umem_config(
        &mut self,
        config: UmemConfig,
        manager_config: T::C,
        frame_count: NonZeroU32,
    ) -> &mut Self {
        // Guranatee that the umem is configured only once.
        assert!(matches!(self.umem_state, UmemConfigState::Init));
        self.umem_state = UmemConfigState::Config {
            umem_config: config,
            manager_config,
            frame_count,
        };
        self
    }

    /// Set the umem a exist umem. The xdp context will use the exist umem. Used to share the umem between different context.
    pub fn with_exist_umem(&mut self, umem: Umem, frame_manager: T) -> &mut Self {
        // Guranatee that the umem is configured only once.
        assert!(matches!(self.umem_state, UmemConfigState::Init));
        self.umem_state = UmemConfigState::Umem {
            umem,
            frame_manager,
        };
        self
    }

    /// Set the use huge pages.
    pub fn with_use_huge_pages(&mut self, use_huge_pages: bool) -> &mut Self {
        self.use_huge_pages = use_huge_pages;
        self
    }

    /// Build the XdpContext.
    pub fn build<R: PollerRunner>(self, runner: &R) -> anyhow::Result<XdpContext> {
        let (umem, frame_manager) = match self.umem_state {
            UmemConfigState::Init => {
                let config = UmemConfig::default();
                let (umem, frames) =
                    Umem::new(config, (4096 * 16).try_into().unwrap(), self.use_huge_pages)?;
                (umem, T::new(T::C::default(), frames)?)
            }
            UmemConfigState::Config {
                umem_config: config,
                manager_config,
                frame_count,
            } => {
                let (umem, frames) = Umem::new(config, frame_count, self.use_huge_pages)?;
                (umem, T::new(manager_config, frames)?)
            }
            UmemConfigState::Umem {
                umem,
                frame_manager,
            } => (umem, frame_manager),
        };

        let context = XdpContext::new(
            self.config,
            umem,
            self.if_name.clone(),
            self.queue_id,
            runner,
            frame_manager,
        )?;

        Ok(context)
    }
}

/// XdpContext is used to create `XdpReceiveHandle` and `XdpSendHandle` to receive and send the packet.
///
/// # NOTE
/// A device can only have one context.
#[derive(Clone)]
pub struct XdpContext {
    umem: Umem,
    inner: Arc<Mutex<XdpContextInner>>,
}

impl XdpContext {
    pub(crate) fn new<R: PollerRunner, T: FrameManager>(
        socket_config: SocketConfig,
        umem: Umem,
        if_name: String,
        queue_id: u32,
        runner: &R,
        frame_manager: T,
    ) -> anyhow::Result<Self> {
        let inner = XdpContextInner::new(
            socket_config,
            umem.clone(),
            if_name,
            queue_id,
            runner,
            frame_manager,
        )?;
        Ok(Self {
            umem,
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Create a new `XdpReceiveHandle`.
    pub fn receive_handle(&self) -> anyhow::Result<XdpReceiveHandle> {
        let mut inner = self.inner.lock().unwrap();
        let receive = inner.take_receiver().ok_or_else(|| {
            anyhow::anyhow!("The receive handle has been created, can not create another one")
        })?;
        Ok(XdpReceiveHandle {
            receive,
            _context: self.clone(),
        })
    }

    /// Create a new `XdpSendHandle`.
    pub fn send_handle(&self) -> XdpSendHandle {
        let inner = self.inner.lock().unwrap();
        XdpSendHandle {
            send: inner.take_sender(),
            _context: self.clone(),
        }
    }

    /// Return umem
    pub fn umem_ref(&self) -> &Umem {
        &self.umem
    }
}

struct XdpContextInner {
    receive: Option<SpScReceiver<XdpReceiveMsg>>,
    send: MpScSender<XdpSendMsg>,
    join_handle: Option<JoinHandle<()>>,
}

impl XdpContextInner {
    /// Create a new context.
    pub fn new<R: PollerRunner, T: FrameManager>(
        socket_config: SocketConfig,
        umem: Umem,
        if_name: String,
        queue_id: u32,
        runner: &R,
        frame_manager: T,
    ) -> anyhow::Result<Self> {
        // Create the socket.
        let interface = Interface::from_str(&if_name)?;
        let (tx_q, rx_q, fq_cq) = Socket::new(socket_config, &umem, &interface, queue_id)?;
        let Some((fill_q, comp_q)) = fq_cq else {
            return Err(anyhow::anyhow!(
                "Fail to create the socket: should not create two context with the same device",
            ));
        };
        // Create the poller.
        let (receive_sender, receive_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (send_sender, send_receiver) = tokio::sync::mpsc::unbounded_channel();
        let poller = XdpPoller::new(
            if_name,
            &umem,
            fill_q,
            comp_q,
            tx_q,
            rx_q,
            send_receiver,
            receive_sender,
            frame_manager.handle()?,
            // # TODO: fix it later
            4096,
            4096,
            socket_config.rx_queue_size().get() as usize,
            socket_config.tx_queue_size().get() as usize,
        )?;
        // Add the poller to the runner.
        let join = runner.add_poller(poller)?;

        Ok(Self {
            receive: Some(receive_receiver),
            send: send_sender,
            join_handle: Some(join),
        })
    }

    pub fn take_receiver(&mut self) -> Option<SpScReceiver<XdpReceiveMsg>> {
        self.receive.take()
    }

    pub fn take_sender(&self) -> MpScSender<XdpSendMsg> {
        self.send.clone()
    }
}

impl Drop for XdpContextInner {
    fn drop(&mut self) {
        // Wait for the poller to finish.
        let join_handle = self.join_handle.take().unwrap();
        if let Err(err) = join_handle.join() {
            log::error!("XdpContextInner drop failed: {:?}", err);
        }
    }
}

/// XdpReceiveHandle is used to receive the packet.
pub struct XdpReceiveHandle {
    receive: SpScReceiver<XdpReceiveMsg>,
    _context: XdpContext,
}

impl XdpReceiveHandle {
    /// Receive the packet.
    pub async fn receive(&mut self) -> anyhow::Result<SmallVec<[FrameDesc; BATCH_SIZSE]>> {
        match self.receive.recv().await {
            Some(XdpReceiveMsg::Recv(frames)) => Ok(frames),
            None => Err(anyhow::anyhow!("CLose")),
        }
    }
}

/// XdpSendHandle is used to send the packet.
#[derive(Clone)]
pub struct XdpSendHandle {
    send: MpScSender<XdpSendMsg>,
    _context: XdpContext,
}

impl XdpSendHandle {
    /// Send the packet using frame.
    pub fn send_frame(&self, data: SmallVec<[FrameDesc; BATCH_SIZSE]>) -> anyhow::Result<()> {
        let iter = data.chunks(BATCH_SIZSE);
        for frames in iter {
            let mut elements = [Default::default(); BATCH_SIZSE];
            elements[..frames.len()].copy_from_slice(frames);
            self.send.send(XdpSendMsg::SendFrame {
                elements,
                len: frames.len(),
            })?;
        }
        Ok(())
    }

    /// Send the packet using data.
    pub fn send(&self, data: Vec<u8>) -> anyhow::Result<()> {
        self.send.send(XdpSendMsg::Send(data))?;
        Ok(())
    }
}
