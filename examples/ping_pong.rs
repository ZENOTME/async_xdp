use async_xdp::{
    Frame, FrameManager, SingleThreadRunner, SlabManager, SlabManagerConfig, XdpContextBuilder,
    XdpSendHandle, BATCH_SIZSE,
};
use packet::ether::Packet;
use xsk_rs::{
    config::{SocketConfig, UmemConfig},
    Umem,
};

async fn ping_pong(dev1: &str, dev2: &str) {
    let runner = SingleThreadRunner::new();

    let umem_config = UmemConfig::builder()
        .fill_queue_size((4096).try_into().unwrap())
        .comp_queue_size((4096).try_into().unwrap())
        .build()
        .unwrap();
    let socket_config = SocketConfig::builder()
        .rx_queue_size((4096).try_into().unwrap())
        .tx_queue_size((4096).try_into().unwrap())
        .build();
    let manager_config = SlabManagerConfig::new(4096);

    let (umem, frames) = Umem::new(umem_config, (4096 * 16).try_into().unwrap(), false).unwrap();
    let frame_manager = SlabManager::new(manager_config, frames).unwrap();

    let mut dev1_context_builder = XdpContextBuilder::new(dev1, 0);
    dev1_context_builder
        .with_socket_config(socket_config)
        .with_exist_umem(umem.clone(), frame_manager.clone());
    let mut dev2_context_builder = XdpContextBuilder::new(dev2, 0);
    dev2_context_builder
        .with_socket_config(socket_config)
        .with_exist_umem(umem.clone(), frame_manager);

    let dev1_context = dev1_context_builder.build(&runner).unwrap();
    let dev2_context = dev2_context_builder.build(&runner).unwrap();

    let mut dev1_receive = dev1_context.receive_handle().unwrap();
    let mut dev2_receive = dev2_context.receive_handle().unwrap();
    let dev1_send = dev1_context.send_handle();
    let dev2_send = dev2_context.send_handle();

    loop {
        tokio::select! {
            frames = dev1_receive.receive() => {
                process(&dev2_send, frames.unwrap()).await;
            }
            frames = dev2_receive.receive() => {
                process(&dev1_send, frames.unwrap()).await;
            }
        }
    }
}

async fn process(send_handle: &XdpSendHandle, frames: smallvec::SmallVec<[Frame; BATCH_SIZSE]>) {
    for frame in frames {
        let packet = Packet::new(frame.data_ref()).unwrap();
        if packet.destination().is_broadcast() {
            send_handle.send(frame.data_ref().to_vec()).unwrap();
        } else {
            send_handle.send_frame(vec![frame].into()).unwrap();
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    env_logger::init();

    ping_pong("veth1", "veth2").await
}
