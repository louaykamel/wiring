use futures::{FutureExt, StreamExt};
use pin_project::pin_project;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{tcp::OwnedWriteHalf, TcpStream},
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
};
use url::Url;

use super::{
    listener::{ConnectInfo, Local, Peer, WireListenerEvent},
    unwire::{Unwire, Unwiring},
    ConnectConfig, SplitStream,
};

type WireId = u64;

#[derive(Debug, Clone)]
pub struct WireInfo {
    wire_id: WireId,
    access_key: u128,
    connect_info: ConnectInfo,
}

impl WireInfo {
    pub(crate) fn new(wire_id: WireId, access_key: u128, connect_info: ConnectInfo) -> Self {
        Self {
            wire_id,
            access_key,
            connect_info,
        }
    }
    pub fn wire_id(&self) -> WireId {
        self.wire_id
    }
    pub fn access_key(&self) -> u128 {
        self.access_key
    }
}

impl Unwiring for WireInfo {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            Ok(Self {
                wire_id: wire.unwiring().await?,
                access_key: wire.unwiring().await?,
                connect_info: wire.unwiring().await?,
            })
        }
    }
}

pub trait Wire: AsyncWrite + Unpin + Send + 'static + Sync + Sized {
    type Stream: SplitStream;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send;
    fn wire<T: Wiring>(&mut self, t: T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            t.wiring(self).await?;
            self.flush().await?;
            Ok(())
        }
    }
    fn wiring<T: Wiring>(&mut self, item: T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        item.wiring(self)
    }
}

impl Wire for OwnedWriteHalf {
    type Stream = TcpStream;

    fn stream(&mut self) -> impl std::future::Future<Output = Result<TcpStream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "TcpStream from OwnedWriteHalf is not supported",
            ))
        }
    }
}

impl Wire for TcpStream {
    type Stream = Self;

    fn stream(&mut self) -> impl std::future::Future<Output = Result<TcpStream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "TcpStream from stream is not supported",
            ))
        }
    }
}

impl<T: AsyncWrite + Send + Sync + Unpin + 'static, C: ConnectConfig> Wire for WireStream<T, C> {
    type Stream = WireStream<C::Stream, C>;

    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            // establish connection with server.
            let peer = self.peer.as_ref().ok_or(std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "Wire doesn't have peer connect info",
            ))?;
            let connect_info = &peer.wire_info.connect_info;

            let stream: <C as ConnectConfig>::Stream = peer.connect_config.connect_stream(connect_info).await?;

            if let Some(local_handle) = &peer.local_handle {
                // bi connect
                let (reply, rx) = oneshot::channel();
                local_handle
                    .send(WireListenerEvent::OutcominWire {
                        stream,
                        forward_info: Some((peer.wire_info.wire_id(), peer.wire_info.access_key())),
                        reply,
                    })
                    .ok();
                let mut wire = rx
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))??;
                // unwire the first message from server.
                let remote_info: WireInfo = wire.unwire().await?;
                let remote_wire_id = remote_info.wire_id();
                wire = wire.with_peer(Peer::<C>::new(
                    remote_info,
                    Some(local_handle.clone()),
                    peer.connect_config.clone(),
                ));
                self.wiring(remote_wire_id).await?;
                Ok(wire)
            } else {
                let mut wire = WireStream::new(stream);
                // connect one way, we wire none, which is 0u8
                0u8.wiring(&mut wire).await?;
                // wire the forward info
                let forward_info = Some((peer.wire_info.wire_id(), peer.wire_info.access_key()));
                wire.wire(forward_info).await?;
                // create the wirestream without local
                // must decode
                let remote_info: WireInfo = wire.unwire().await?;
                let remote_wire_id = remote_info.wire_id();
                wire = wire.with_peer(Peer::new(remote_info, None, peer.connect_config.clone()));
                self.wiring(remote_wire_id).await?;
                Ok(wire)
            }
        }
    }
}

impl<T: AsyncRead, C: ConnectConfig> AsyncRead for WireStream<T, C> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.project().stream.poll_read(cx, buf)
    }
}

impl<T: AsyncWrite, C: ConnectConfig> AsyncWrite for WireStream<T, C> {
    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().stream.poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().stream.poll_shutdown(cx)
    }
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().stream.poll_write(cx, buf)
    }
    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().stream.poll_write_vectored(cx, bufs)
    }
}

pub trait HandleWire<C: ConnectConfig> {
    type Error: std::error::Error;
    /// Handle the incoming wire from the client.
    fn handle_wire(&mut self, stream: WireStream<C::Stream, C>) -> Result<(), Self::Error>;
}

impl<C: ConnectConfig> HandleWire<C> for UnboundedSender<WireStream<C::Stream, C>> {
    type Error = tokio::sync::mpsc::error::SendError<WireStream<C::Stream, C>>;
    fn handle_wire(
        &mut self,
        stream: WireStream<C::Stream, C>,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<WireStream<C::Stream, C>>> {
        self.send(stream)
    }
}

#[pin_project]
#[derive(Debug)]
pub struct WireStream<T, C: ConnectConfig> {
    /// NOTE: this local should be under feature as well. only needed
    /// It contains the local wire info, including the handle to register more wires.
    /// And inbox to receive incoming wires from remote.
    pub(crate) local: Option<Local<C>>,
    /// The peer/remote info, used to establish further wires
    pub(crate) peer: Option<Peer<C>>,
    #[pin]
    /// The stream which hold connection to remote end
    pub(crate) stream: T,
}

impl<T, C: ConnectConfig> WireStream<T, C> {
    pub fn new(stream: T) -> Self {
        Self {
            local: None,
            peer: None,
            stream,
        }
    }
    pub(crate) fn with_local(mut self, local: Local<C>) -> Self {
        self.local.replace(local);
        self
    }
    pub(crate) fn with_peer(mut self, peer: Peer<C>) -> Self {
        self.peer.replace(peer);
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NoHandle;

#[derive(Debug, Clone, Copy)]
pub struct WireConfig<C: ConnectConfig, H = NoHandle> {
    config: C,
    handle: H,
}

#[allow(dead_code)]
impl<C: ConnectConfig> WireConfig<C> {
    /// Create wireconfig with provided connect config
    pub fn new(config: C) -> Self {
        Self {
            config,
            handle: NoHandle,
        }
    }
    /// Create wire config with handle, for internal use
    pub(crate) fn with_handle(
        config: C,
        handle: tokio::sync::mpsc::UnboundedSender<WireListenerEvent<C>>,
    ) -> WireConfig<C, tokio::sync::mpsc::UnboundedSender<WireListenerEvent<C>>> {
        WireConfig::<C, _> { config, handle }
    }
    /// Connect to remote in client mode
    pub async fn connect(&self, connect_info: &ConnectInfo) -> Result<WireStream<C::Stream, C>, std::io::Error> {
        let stream = self.config.connect_stream(&connect_info).await?;

        let mut wire = WireStream::new(stream);
        // wire none and none for both wire_info and forward info, as we're not forwarding this.
        wire.wire(0u16).await?;
        // unwire the pushed remote_wire_info.
        let wire_info = wire.unwire().await?;
        let peer = Peer::new(wire_info, None, self.config.clone());
        // set the peer info
        Ok(wire.with_peer(peer))
    }
}

#[allow(dead_code)]
impl<C: ConnectConfig> WireConfig<C, tokio::sync::mpsc::UnboundedSender<WireListenerEvent<C>>> {
    /// Push the stream and RETURN (if set and not nested forward), else it's handled at listener level with handle_wire
    pub async fn wire<const RETURN: bool>(
        &self,
        mut stream: C::Stream,
    ) -> Result<Option<WireStream<C::Stream, C>>, std::io::Error> {
        let mut temp_wire = WireStream::<C::Stream, C>::new(stream);

        let remote_info = temp_wire.unwire().await?;
        let forward_info = temp_wire.unwire().await?;
        stream = temp_wire.stream;
        if RETURN {
            let (reply, rx) = oneshot::channel();
            let message = WireListenerEvent::Incomingwire {
                stream,
                remote_info,
                forward_info,
                reply: Some(reply),
            };
            self.handle
                .send(message)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotConnected, e))?;
            let w = rx
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Interrupted, e))?;
            w
        } else {
            let message = WireListenerEvent::Incomingwire {
                stream,
                remote_info,
                forward_info,
                reply: None,
            };
            self.handle
                .send(message)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotConnected, e))?;
            Ok(None)
        }
    }
    /// Shutdown the listener
    pub fn shutdown(&self) {
        self.handle.send(WireListenerEvent::<C>::Shutdown).ok();
    }
    /// Connect to remote with enabled bi-directional communication
    pub async fn connect(&self, connect_info: &ConnectInfo) -> Result<WireStream<C::Stream, C>, std::io::Error> {
        let stream = self.config.connect_stream(&connect_info).await?;
        let (reply, rx) = oneshot::channel();

        let event = WireListenerEvent::OutcominWire {
            stream,
            forward_info: None,
            reply,
        };
        self.handle
            .send(event)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
        let mut w = rx
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))??;
        let peer_info = w.unwire().await?;
        // the first
        Ok(w.with_peer(Peer::new(peer_info, Some(self.handle.clone()), self.config.clone())))
    }
}

pub trait Wiring: Send + Sync {
    const SAFE: bool = true;
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send;
}

impl Wiring for WireInfo {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            self.wire_id.wiring(wire).await?;
            self.access_key.wiring(wire).await?;
            self.connect_info.wiring(wire).await
        }
    }
}

impl<'a> Wiring for &'a WireInfo {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            self.wire_id.wiring(wire).await?;
            self.access_key.wiring(wire).await?;
            (&self.connect_info).wiring(wire).await
        }
    }
}

impl Wiring for String {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_bytes().wiring(wire).await }
    }
}

impl<'a> Wiring for &'a String {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_bytes().wiring(wire).await }
    }
}

impl<'a> Wiring for &'a str {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_bytes().wiring(wire).await }
    }
}

impl<'a> Wiring for &'a [u8] {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let len = self.len() as u64;
            len.wiring(wire).await?;
            wire.write_all(self).await
        }
    }
}

impl<'a, const LEN: usize> Wiring for &'a [u8; LEN] {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { wire.write_all(self).await }
    }
}

impl<const LEN: usize> Wiring for [u8; LEN] {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { wire.write_all(&self).await }
    }
}

impl Wiring for Url {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_str().wiring(wire).await }
    }
}

impl<'a> Wiring for &'a Url {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_str().wiring(wire).await }
    }
}

impl<T> Wiring for tokio::sync::oneshot::Sender<T>
where
    T: Unwiring + 'static,
{
    const SAFE: bool = false;
    fn wiring<W: Wire>(mut self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let mut new: W::Stream = wire.stream().await?;
            let task = async move {
                tokio::select! {
                    _ = self.closed() => {
                        // channel closed
                    },
                    item = new.unwire::<T>() => {
                        if let Ok(item) = item {
                            self.send(item).ok();
                        }
                    },
                }
            };
            tokio::spawn(task.boxed());
            Ok(())
        }
    }
}

impl<T> Wiring for UnboundedSender<T>
where
    T: Unwiring + 'static,
{
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let new: W::Stream = wire.stream().await?;
            let closed_handle = self.clone();
            let (mut read, mut send) = new.split()?;
            let shutdown = async move {
                closed_handle.closed().await;
                send.shutdown().await.ok();
            };
            let j = tokio::spawn(shutdown.boxed());
            let task = async move {
                while let Ok(item) = read.unwire().await {
                    if let Err(_) = self.send(item) {
                        break;
                    };
                }
                j.abort();
            };
            tokio::spawn(task.boxed());
            Ok(())
        }
    }
}

impl<T: Wiring + 'static + Clone> Wiring for tokio::sync::broadcast::Receiver<T> {
    /// note safe to store
    const SAFE: bool = false;
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let w = wire.stream().await?;
            let (mut r, mut w) = w.split()?;
            let mut rx = self;
            let task = async move {
                while let Ok(item) = rx.recv().await {
                    if let Err(_) = w.wire(item).await {
                        break;
                    }
                }
            };

            let j = tokio::spawn(task.boxed());
            let detect_shutdown = async move {
                r.read_u8().await.ok();
                // if read_u8 was due to timeout we should not abort right?
                j.abort();
            };
            tokio::spawn(detect_shutdown.boxed());
            Ok(())
        }
    }
}

impl<T: Wiring + 'static + Clone> Wiring for tokio::sync::watch::Receiver<T> {
    /// note safe to store
    const SAFE: bool = false;
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let w = wire.stream().await?;
            let (mut r, mut w) = w.split()?;
            let mut rx = tokio_stream::wrappers::WatchStream::new(self);
            let task = async move {
                while let Some(item) = rx.next().await {
                    if let Err(_) = w.wire(item).await {
                        break;
                    }
                }
            };
            let j = tokio::spawn(task.boxed());
            let detect_shutdown = async move {
                r.read_u8().await.ok();
                j.abort();
            };
            tokio::spawn(detect_shutdown.boxed());
            Ok(())
        }
    }
}

impl<T: Wiring + Unwiring + 'static + Clone> Wiring for tokio::sync::watch::Sender<T> {
    /// note safe to store
    const SAFE: bool = false;
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let mut w = wire.stream().await?;
            // must wire initial value
            let r = self.borrow().clone();
            w.wire(r).await?;
            let task = async move {
                loop {
                    tokio::select! {
                        _ = self.closed() => {
                            w.shutdown().await.ok();
                            break;
                        },
                        item = w.unwire::<T>() => {
                            if let Ok(item ) = item {
                                if let Err(_) = self.send(item) {
                                    break;
                                }
                            } else {
                                break
                            }
                        },
                        else => break,
                    };
                }
            };
            tokio::spawn(task.boxed());

            Ok(())
        }
    }
}

impl<T> Wiring for tokio::sync::broadcast::Sender<T>
where
    T: Unwiring + 'static,
{
    const SAFE: bool = false;
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let mut new: W::Stream = wire.stream().await?;

            let task = async move {
                while let Ok(item) = new.unwire().await {
                    // note: detecting when this is dropped is impossible, as tokio doesn't provide closed()
                    if let Err(_) = self.send(item) {
                        break;
                    };
                }
            };
            tokio::spawn(task.boxed());
            Ok(())
        }
    }
}

impl<T> Wiring for tokio::sync::mpsc::Sender<T>
where
    T: Unwiring + 'static,
{
    const SAFE: bool = false;
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let new: W::Stream = wire.stream().await?;
            let closed_handle = self.clone();
            let (mut read, mut send) = new.split()?;
            let shutdown = async move {
                closed_handle.closed().await;
                send.shutdown().await.ok();
            };
            let j = tokio::spawn(shutdown.boxed());
            let task = async move {
                while let Ok(item) = read.unwire().await {
                    if let Err(_) = self.send(item).await {
                        break;
                    };
                }
                j.abort();
            };
            tokio::spawn(task.boxed());
            Ok(())
        }
    }
}

impl<T: Wiring + 'static> Wiring for tokio::sync::oneshot::Receiver<T> {
    const SAFE: bool = false;
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let new: W::Stream = wire.stream().await?;
            let (mut r, mut w) = new.split()?;
            let task = async move {
                tokio::select! {
                    _ = r.read_u8() => {
                    },
                    item = self => {
                        if let Ok(item) = item {
                            w.wire(item).await.ok();
                        };
                    }
                    else => {
                        ()
                    },
                }
                w.shutdown().await.ok();
            };
            tokio::spawn(task.boxed());
            Ok(())
        }
    }
}

impl<T: Wiring + 'static> Wiring for UnboundedReceiver<T> {
    const SAFE: bool = false;
    fn wiring<W: Wire>(mut self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let new: W::Stream = wire.stream().await?;
            let (mut r, mut w) = new.split()?;
            let task = async move {
                while let Some(item) = self.recv().await {
                    if let Err(_) = w.wire(item).await {
                        break;
                    }
                }
            };
            let h = tokio::spawn(task.boxed());
            let detect_shutdown = async move {
                // the stream not supposed to send anything, this is just to detect when is offline.
                r.read_u8().await.ok();
                // if this is closed, it means the wire is closed as well.
                h.abort();
            };
            tokio::spawn(detect_shutdown.boxed());
            Ok(())
        }
    }
}

impl<T: Wiring + 'static> Wiring for tokio::sync::mpsc::Receiver<T> {
    const SAFE: bool = false;
    fn wiring<W: Wire>(mut self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let new: W::Stream = wire.stream().await?;
            let (mut r, mut w) = new.split()?;
            let task = async move {
                while let Some(item) = self.recv().await {
                    if let Err(_) = w.wire(item).await {
                        break;
                    }
                }
            };
            let h = tokio::spawn(task.boxed());
            let detect_shutdown = async move {
                // the stream not supposed to send anything, this is just to detect when is offline.
                r.read_u8().await.ok();
                // if this is closed, it means the wire is closed as well.
                h.abort();
            };
            tokio::spawn(detect_shutdown.boxed());
            Ok(())
        }
    }
}

impl Wiring for () {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        // Must be wired as rust treats this as type even if it's zerosized. the reason:
        // channel can support sending (), and therefore the other end must detects when () is sent,
        // if it was no-op, then no way for unwire to detect it. think of it as heartbeat.
        wire.wiring(1u8)
    }
}

impl Wiring for u8 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u8(self)
    }
}

impl<'a> Wiring for &'a u8 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u8(*self)
    }
}

impl Wiring for i8 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i8(self)
    }
}

impl<'a> Wiring for &'a i8 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i8(*self)
    }
}

impl Wiring for u16 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u16(self)
    }
}

impl<'a> Wiring for &'a u16 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u16(*self)
    }
}

impl Wiring for i16 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i16(self)
    }
}

impl<'a> Wiring for &'a i16 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i16(*self)
    }
}

impl Wiring for u32 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u32(self)
    }
}

impl<'a> Wiring for &'a u32 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u32(*self)
    }
}

impl Wiring for i32 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i32(self)
    }
}

impl<'a> Wiring for &'a i32 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i32(*self)
    }
}

impl Wiring for u64 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u64(self)
    }
}

impl<'a> Wiring for &'a u64 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u64(*self)
    }
}

impl Wiring for i64 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i64(self)
    }
}

impl<'a> Wiring for &'a i64 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i64(*self)
    }
}

impl Wiring for u128 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u128(self)
    }
}

impl<'a> Wiring for &'a u128 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u128(*self)
    }
}

impl Wiring for i128 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i128(self)
    }
}

impl<'a> Wiring for &'a i128 {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i128(*self)
    }
}

impl<T: Wiring> Wiring for Vec<T> {
    fn wiring<W: Wire>(mut self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let len = self.len() as u64;
            len.wiring(wire).await?;
            while let Some(t) = self.pop() {
                t.wiring(wire).await?;
            }
            Ok(())
        }
    }
}

impl<'a, T: Wiring> Wiring for &'a Vec<T>
where
    &'a T: Wiring,
{
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let len = self.len() as u64;
            len.wiring(wire).await?;
            let mut i = self.iter();
            while let Some(t) = i.next() {
                t.wiring(wire).await?;
            }
            Ok(())
        }
    }
}

impl<T: Wiring> Wiring for std::collections::HashSet<T> {
    fn wiring<W: Wire>(mut self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async move {
            let len = self.len() as u64;
            len.wiring(wire).await?;
            let mut s = self.drain();
            while let Some(t) = s.next() {
                t.wiring(wire).await?;
            }
            Ok(())
        }
    }
}

impl<'a, T: Wiring> Wiring for &'a std::collections::HashSet<T>
where
    &'a T: Wiring + std::fmt::Debug,
{
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        let mut i = self.iter();
        async move {
            let len = self.len() as u64;
            len.wiring(wire).await?;
            while let Some(t) = i.next() {
                t.wiring(wire).await?;
            }
            Ok(())
        }
    }
}

impl<T: Wiring> Wiring for Option<T> {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async {
            if let Some(t) = self {
                1u8.wiring(wire).await?;
                t.wiring(wire).await
            } else {
                0u8.wiring(wire).await
            }
        }
    }
}

impl<T: Wiring, TT: Wiring> Wiring for (T, TT) {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async {
            self.0.wiring(wire).await?;
            self.1.wiring(wire).await
        }
    }
}
