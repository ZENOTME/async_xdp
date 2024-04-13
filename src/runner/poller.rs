use std::{collections::HashSet, io::Write, vec};

use crate::frame_manager::FrameHandle;
use itertools::Itertools;
use log::{error, trace, warn};
use smallvec::SmallVec;
use tokio::sync::mpsc::error::TryRecvError;
use xsk_rs::{CompQueue, FillQueue, FrameDesc, RxQueue, TxQueue, Umem};

/// Poller is a background task that polls the packet from xdk socket.
pub trait Poller: Send + 'static {
    /// Initialize the poller.
    fn init(&mut self) -> anyhow::Result<()>;
    /// Run the poller.
    fn run_once(&mut self) -> anyhow::Result<()>;
}

/// The number of frames be pass.
pub const BATCH_SIZSE: usize = 32;

// # TODO: Fix this later.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum XdpSendMsg {
    /// Send the packet using frame return by receive to the context.
    SendFrame {
        elements: [FrameDesc; BATCH_SIZSE],
        len: usize,
    },
    /// Send the packet using data to the context.
    Send(Vec<u8>),
}

/// XdpReceiveMsg is the message to receive the packet from the context.
#[derive(Debug)]
pub(crate) enum XdpReceiveMsg {
    /// Receive the packet from the context.
    Recv(SmallVec<[FrameDesc; BATCH_SIZSE]>),
}

pub(crate) type SpScReceiver<T> = tokio::sync::mpsc::UnboundedReceiver<T>;
pub(crate) type SpScSender<T> = tokio::sync::mpsc::UnboundedSender<T>;
pub(crate) type MpScReceiver<T> = tokio::sync::mpsc::UnboundedReceiver<T>;
pub(crate) type MpScSender<T> = tokio::sync::mpsc::UnboundedSender<T>;

#[derive(Debug)]
pub(crate) struct XdpPoller<T: FrameHandle> {
    // Used for debug.
    #[allow(dead_code)]
    interface_name: String,

    frame_handle: T,

    umem: Umem,
    fill_q: FillQueue,
    comp_q: CompQueue,
    tx_q: TxQueue,
    rx_q: RxQueue,

    send: MpScReceiver<XdpSendMsg>,
    receive: SpScSender<XdpReceiveMsg>,

    rx_burst: Box<[FrameDesc]>,
    complete_burst: Box<[FrameDesc]>,
    fill_burst: Box<[FrameDesc]>,
    tx_burst: Box<[FrameDesc]>,

    trace_mode: bool,
    send_frame_desc: HashSet<usize>,
}

impl<T: FrameHandle> XdpPoller<T> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        interface_name: String,
        umem: &Umem,
        fill_q: FillQueue,
        comp_q: CompQueue,
        tx_q: TxQueue,
        rx_q: RxQueue,
        send: MpScReceiver<XdpSendMsg>,
        receive: SpScSender<XdpReceiveMsg>,
        frame_handle: T,
        fill_q_size: usize,
        comp_q_size: usize,
        rx_q_size: usize,
        tx_q_size: usize,
        trace_mode: bool,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            interface_name,
            umem: umem.clone(),
            frame_handle,
            fill_q,
            comp_q,
            tx_q,
            rx_q,
            send,
            receive,
            rx_burst: vec![Default::default(); rx_q_size].into_boxed_slice(),
            fill_burst: vec![Default::default(); fill_q_size].into_boxed_slice(),
            complete_burst: vec![Default::default(); comp_q_size].into_boxed_slice(),
            tx_burst: vec![Default::default(); tx_q_size].into_boxed_slice(),
            trace_mode,
            send_frame_desc: HashSet::new(),
        })
    }

    fn init(&mut self) -> anyhow::Result<()> {
        self.produce_fill_q(self.fill_burst.len())?;
        Ok(())
    }

    fn produce_fill_q(&mut self, cnt: usize) -> anyhow::Result<()> {
        if cnt == 0 {
            return Ok(());
        }
        assert!(cnt <= self.fill_burst.len());
        let cnt = self.frame_handle.reserve(cnt)?;
        (0..cnt).for_each(|i| {
            let frame = self.frame_handle.alloc_one().unwrap();
            self.fill_burst[i] = frame;
        });
        loop {
            let complete_cnt = unsafe { self.fill_q.produce(self.fill_burst[..cnt].as_ref()) };
            assert!(complete_cnt == 0 || (complete_cnt > 0 && complete_cnt == cnt));
            if complete_cnt == cnt {
                break;
            }
            if self.fill_q.needs_wakeup() {
                self.fill_q.wakeup(self.rx_q.fd_mut(), 0)?
            }
        }
        Ok(())
    }

    fn receive_rx(&mut self) -> anyhow::Result<usize> {
        let recv_cnt = unsafe { self.rx_q.poll_and_consume(&mut self.rx_burst, 0)? };
        if recv_cnt == 0 {
            return Ok(0);
        }
        let chunks = self.rx_burst[..recv_cnt].iter().chunks(BATCH_SIZSE);
        for chunk in chunks.into_iter() {
            let frames = chunk.into_iter().copied().collect();
            self.receive.send(XdpReceiveMsg::Recv(frames))?;
        }
        Ok(recv_cnt)
    }

    fn receive(&mut self) -> anyhow::Result<()> {
        let recv_cnt = self.receive_rx()?;

        if recv_cnt > 0 {
            self.produce_fill_q(recv_cnt)?;
        }

        Ok(())
    }

    fn reap_fill_q(&mut self) -> anyhow::Result<()> {
        let complete_cnt = unsafe { self.comp_q.consume(&mut self.complete_burst) };
        if complete_cnt > 0 {
            if self.trace_mode {
                self.complete_burst[..complete_cnt].iter().for_each(|frame| {
                    if !self.send_frame_desc.remove(&frame.addr()) {
                        error!("The frame desc is not in the set: {:?}: something wrong fail happen", frame.addr());
                    }
                });
            }
            let res = self.complete_burst[..complete_cnt].iter().copied();
            self.frame_handle.free(res)?;
        }
        Ok(())
    }

    fn send_tx(&mut self) -> anyhow::Result<usize> {
        let mut cnt = 0;
        loop {
            match self.send.try_recv() {
                Ok(XdpSendMsg::SendFrame {
                    elements: new_frams,
                    len,
                }) => {
                    self.tx_burst[cnt..cnt + len].copy_from_slice(&new_frams[..len]);
                    cnt += len;
                }
                Ok(XdpSendMsg::Send(data)) => {
                    // Alloce a new frame.
                    self.frame_handle.reserve(1)?;
                    let mut frame = self.frame_handle.alloc_one()?;
                    let mut payload = unsafe { self.umem.data_mut(&mut frame) };
                    payload.cursor().write_all(&data)?;
                    self.tx_burst[cnt] = frame;
                    cnt += 1;
                }
                Err(TryRecvError::Empty) => {
                    break;
                }
                Err(err) => {
                    return Err(anyhow::anyhow!("The receive channel is closed: {:?}", err));
                }
            }
            if self.tx_burst.len() - cnt < BATCH_SIZSE {
                break;
            }
        }
        if cnt > 0 {
            if self.trace_mode {
                self.tx_burst[..cnt].iter().for_each(|frame| {
                    if !self.send_frame_desc.insert(frame.addr()) {
                        warn!("The frame desc is already in the set: {:?}: some pack send fail happen", frame.addr());
                    }
                });
            }
            loop {
                let res = unsafe { self.tx_q.produce_and_wakeup(&self.tx_burst[..cnt])? };
                assert!(res == 0 || (res > 0 && res == cnt));
                if res == cnt {
                    break;
                }
            }
        }
        Ok(cnt)
    }

    fn send(&mut self) -> anyhow::Result<()> {
        self.reap_fill_q()?;
        let _ = self.send_tx()?;
        Ok(())
    }

    fn run_once(&mut self) -> anyhow::Result<()> {
        self.receive()?;
        self.send()?;
        Ok(())
    }
}

impl<T: FrameHandle> Poller for XdpPoller<T> {
    fn init(&mut self) -> anyhow::Result<()> {
        self.init()
    }

    fn run_once(&mut self) -> anyhow::Result<()> {
        self.run_once()?;
        if self.trace_mode {
            if !self.send_frame_desc.is_empty() {
                trace!("sending frame: {:?}", self.send_frame_desc);
            }
        }
        Ok(())
    }
}
