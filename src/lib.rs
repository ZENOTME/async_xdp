#![deny(missing_docs)]

//! This crate provides a async and easier way to use xdp socket. It's based on the xsk-rs crate.
mod frame_manager;
pub use frame_manager::*;
mod context;
pub use context::*;
mod runner;
pub use runner::*;

pub use xsk_rs::*;