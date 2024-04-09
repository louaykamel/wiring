mod core;
mod test;

pub mod prelude {
    pub use crate::core::{
        listener::{ConnectInfo, WireListener},
        unwire::{Unwire, Unwiring},
        wire::{Wire, WireChannel, WireConfig, WireStream, Wiring},
        wired::{WiredHandle, WiredServer},
        BufStream, BufStreamConfig, ConnectConfig, SplitStream, TcpStreamConfig,
    };
    pub use wiring_derive::{Unwiring, Wiring};
}
