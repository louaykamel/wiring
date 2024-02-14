use futures::FutureExt;
use std::{collections::HashMap, fmt::Debug};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
    sync::mpsc::UnboundedSender,
};
use unwire::Unwire;
use wire::Wire;
pub type WireId = u64;
use wire::{WireStream, Wiring};

pub(crate) mod listener;
pub(crate) mod unwire;
pub(crate) mod wire;

use listener::Local;

use self::listener::ConnectInfo;

pub trait SplitStream: Sized + Send + Sync + Unwire + Wire + Debug {
    type Unwire: Unwire;
    type Wire: Wire;
    fn split(self) -> Result<(Self::Unwire, Self::Wire), std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Wire doesn't support split",
        ))
    }
}

impl SplitStream for tokio::net::TcpStream {
    type Unwire = OwnedReadHalf;
    type Wire = OwnedWriteHalf;
    fn split(self) -> Result<(Self::Unwire, Self::Wire), std::io::Error> {
        Ok(self.into_split())
    }
}

impl<T, C> SplitStream for WireStream<T, C>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static + Sync,
    C: ConnectConfig,
{
    type Unwire = WireStream<tokio::io::ReadHalf<T>, C>;
    type Wire = WireStream<tokio::io::WriteHalf<T>, C>;

    fn split(self) -> Result<(Self::Unwire, Self::Wire), std::io::Error> {
        // to make this more generic, we're using io::split.
        let (r, w) = tokio::io::split(self.stream);

        let mut r = WireStream::<_, C>::new(r);
        if let Some(Local { wire_info, incoming }) = self.local {
            let local = Local::new(wire_info, incoming);
            r = r.with_local(local);
        };
        let mut w = WireStream::new(w);
        w.peer = self.peer;
        Ok((r, w))
    }
}

pub trait ConnectConfig: Clone + Send + Sync + 'static + Debug {
    type Stream: AsyncRead + AsyncWrite + Send + Sync + Unpin + Debug;
    fn connect_stream(
        &self,
        connect_info: &ConnectInfo,
    ) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send;
}

#[derive(Debug, Clone)]
pub struct TcpStreamConfig;

impl ConnectConfig for TcpStreamConfig {
    type Stream = TcpStream;
    async fn connect_stream(&self, connect_info: &ConnectInfo) -> Result<Self::Stream, std::io::Error> {
        match connect_info {
            ConnectInfo::TcpStream(addr) => {
                let stream = TcpStream::connect(addr).await?;
                Ok(stream)
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Provided peer connect info unsupported by tcpstreamconfig",
            )),
        }
    }
}
