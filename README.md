## Async Xdp

This is a rust async interface based on [xsk_rs](https://docs.rs/xsk-rs). 

## Design

The target of it is to provide high level interface to use xdp socket in async and simple way. 

The basic design of it is to provide `XdpContext` for user. `XdpContext` allow user to use interface like `fn send` or `async fn receive`. These request will send to the background thread and they will do the exact job using xsk_rs and send the result back to XdpContext.

It include serval component to do this: 
1. FrameManager trait to manage the frame alloc and free. 
    
    async_xdp provide a slab manger for this. But it allow user to custom their own frame manager.
2. Runner trait for a background thread to pull the xdp frame. 
    
    async_xdp provide a simple SingleThreadRunner to create thread for each xdp socket. But it allow user to custom their own frame runner. 

## Example
For runable example, see `examples/ping_pong.rs`

```
// 1. Create relate config like xsk_rs
let umem_config = UmemConfig::builder()
    .fill_queue_size((4096).try_into().unwrap())
    .comp_queue_size((4096).try_into().unwrap())
    .build()
    .unwrap();
let socket_config = SocketConfig::builder()
    .rx_queue_size((4096).try_into().unwrap())
    .tx_queue_size((4096).try_into().unwrap())
    .build();
let (umem, frames) = Umem::new(umem_config, (4096 * 16).try_into().unwrap(), false).unwrap();

// 2. Create the frame manager 
let manager_config = SlabManagerConfig::new(4096);
let frame_manager = SlabManager::new(manager_config, frames).unwrap();

// 3. Create the runner 
let runner = SingleThreadRunner::new();

// 4. Build the context
let mut dev1_context_builder = XdpContextBuilder::new(dev1, 0);
dev1_context_builder
    .with_socket_config(socket_config)
    .with_exist_umem(umem.clone(), frame_manager.clone());
let dev1_context = dev1_context_builder.build(&runner).unwrap();

// 5. Get the receive and send handle from context and use them happily!
let mut dev1_receive = dev1_context.receive_handle().unwrap();
let dev1_send = dev1_context.send_handle();
```

## Progress

This project is still WIP, I have use it in my own project for now but it still lack of lots of thing todo:
- [ ] doc
- [ ] test
- [ ] ci

## Others

inspierd by https://github.com/skyzh/uring-positioned-io.s