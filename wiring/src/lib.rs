#![feature(maybe_uninit_array_assume_init)]
//#![feature(generic_const_exprs)]
#![feature(test)]

mod core;

#[allow(dead_code)]
mod test;

pub mod prelude {
    pub use crate::core::{
        listener::{ConnectInfo, WireListener},
        unwire::{BufUnWire, Unwire, Unwiring},
        wire::{BufWire, Wire, WireChannel, WireConfig, WireStream, Wiring},
        wired::{WiredHandle, WiredServer},
        BufStream, BufStreamConfig, ConnectConfig, SplitStream, TcpStreamConfig,
    };
    pub use wiring_derive::{Unwiring, Wiring};
}
