use std::{
    fmt::Debug,
    io::{Error, ErrorKind, Write},
    pin::Pin,
    sync::Arc,
    task::Poll,
};

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
    wired::{HandleEvent, WiredHandle, WiredServer},
    ConnectConfig, IoSplit, SplitStream,
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

pub trait Wire: AsyncWrite + Unpin + Send + Sync + Sized {
    type Stream: SplitStream;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send;
    fn wire<T: Wiring>(&mut self, t: T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            t.wiring(self).await?;
            self.flush().await?;
            Ok(())
        }
    }
    fn wire_ref<T: Wiring>(&mut self, t: &T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            t.wiring_ref(self).await?;
            self.flush().await?;
            Ok(())
        }
    }
    #[inline]
    fn sync_wire<T: Wiring>(&mut self, t: &T) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        t.sync_wiring(self)?;
        self.flush()?;
        Ok(())
    }
    #[inline]
    fn sync_wire_f32(&mut self, n: &f32) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        self.write_all(&n.to_be_bytes())
    }
    #[inline]
    fn sync_wire_u64(&mut self, n: &u64) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        self.write_all(&n.to_be_bytes())
    }
    #[inline]
    fn sync_wire_all<const CHECK: bool>(&mut self, bytes: &[u8]) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        self.write_all(bytes)
    }
    /// Reserve capacity on the underlaying wire if it does allow it.
    #[allow(dead_code)]
    fn reserve_capacity(&mut self, _add: usize) {}
    fn wiring<T: Wiring>(&mut self, item: T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        item.wiring(self)
    }
}

/// It converts the wire to unchecked.
#[pin_project::pin_project]
struct Unchecked<'a, T: Wire> {
    #[pin]
    wire: &'a mut T,
}

impl<'a, T: Wire> AsyncWrite for Unchecked<'a, T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let p = self.project();
        p.wire.poll_write(cx, buf)
    }
    fn is_write_vectored(&self) -> bool {
        self.wire.is_write_vectored()
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let p = self.project();
        p.wire.poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let p = self.project();
        p.wire.poll_shutdown(cx)
    }
}

impl<'a, T: std::io::Write + Wire> Write for Unchecked<'a, T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.wire.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.wire.flush()
    }
}

impl<'a, W: Wire + std::io::Write> Wire for Unchecked<'a, W> {
    type Stream = TcpStream;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Cannot to establish stream from Unchecked wire",
            ))
        }
    }
    #[inline]
    fn wire<T: Wiring>(&mut self, t: T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        let r = self.sync_wire(&t);
        async move { r }
    }
    #[inline]
    fn wire_ref<T: Wiring>(&mut self, t: &T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        let r = self.sync_wire(t);
        async move { r }
    }
    #[inline]
    fn sync_wire<T: Wiring>(&mut self, t: &T) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        t.sync_wiring(self)
    }
    #[inline(always)]
    fn sync_wire_f32(&mut self, n: &f32) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        self.wire.sync_wire_f32(n)
    }
    #[inline]
    fn sync_wire_u64(&mut self, n: &u64) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        self.wire.sync_wire_u64(n)
    }
    #[inline(always)]
    fn sync_wire_all<const CHECK: bool>(&mut self, bytes: &[u8]) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        // Unchecked wrapper enforce nocheck.
        self.wire.sync_wire_all::<false>(bytes)
    }
}

impl Wire for Vec<u8> {
    type Stream = TcpStream;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Cannot to establish stream from Vec<u8>",
            ))
        }
    }
    #[inline]
    fn wire<T: Wiring>(&mut self, t: T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        let r = self.sync_wire(&t);
        async move { r }
    }
    #[inline]
    fn wire_ref<T: Wiring>(&mut self, t: &T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        let r = self.sync_wire(t);
        async move { r }
    }
    #[inline]
    fn sync_wire<T: Wiring>(&mut self, t: &T) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        t.sync_wiring(self)
    }
    #[inline(always)]
    fn sync_wire_f32(&mut self, n: &f32) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        self.extend_from_slice(&n.to_be_bytes());
        Ok(())
    }
    #[inline]
    fn sync_wire_u64(&mut self, n: &u64) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        self.extend_from_slice(&n.to_be_bytes());
        Ok(())
    }
    #[inline(always)]
    fn sync_wire_all<const CHECK: bool>(&mut self, bytes: &[u8]) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        if CHECK {
            self.extend_from_slice(bytes);
        } else {
            let bl = bytes.len();
            let l = self.len();
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.as_mut_ptr().add(l), bl);
                self.set_len(l + bl);
            }
        }
        Ok(())
    }
    #[inline]
    fn reserve_capacity(&mut self, add: usize) {
        self.reserve(add)
    }
}

impl<'a> Wire for std::io::Cursor<&'a mut [u8]> {
    type Stream = TcpStream;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Cannot to establish stream from std::io::Cursor<_>",
            ))
        }
    }
    fn wire<T: Wiring>(&mut self, t: T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        let r = self.sync_wire(&t);
        async move { r }
    }
    fn wire_ref<T: Wiring>(&mut self, t: &T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        let r = self.sync_wire(t);
        async move { r }
    }
    #[inline]
    fn sync_wire<T: Wiring>(&mut self, t: &T) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        t.sync_wiring(self)?;
        std::io::Write::flush(self)
    }
}

impl Wire for std::io::Cursor<Vec<u8>> {
    type Stream = Self;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Cannot to establish stream from std::io::Cursor<Vec<u8>>",
            ))
        }
    }
    fn wire<T: Wiring>(&mut self, t: T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        let r = self.sync_wire(&t);
        async move { r }
    }
    fn wire_ref<T: Wiring>(&mut self, t: &T) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        let r = self.sync_wire(t);
        async move { r }
    }
    #[inline]
    fn sync_wire<T: Wiring>(&mut self, t: &T) -> Result<(), std::io::Error>
    where
        Self: Write,
    {
        t.sync_wiring(self)?;
        std::io::Write::flush(self)
    }
}

impl<RW: AsyncRead + AsyncWrite + Unpin + Send + Sync + Debug + 'static> Wire for tokio::io::BufStream<RW> {
    type Stream = Self;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Cannot to establish stream from std::io::Cursor<Vec<u8>>",
            ))
        }
    }
}
impl<RW: AsyncRead + AsyncWrite + Unpin + Send + Sync + Debug + 'static> SplitStream for tokio::io::BufStream<RW> {
    type Unwire = Self;
    type Wire = Self;
    fn split(self) -> Result<(Self::Unwire, Self::Wire), std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Cannot to establish stream from WriteHalf",
        ))
    }
}

impl<RW: AsyncRead + AsyncWrite + Unpin + Send + Sync + Debug + 'static> Unwire for tokio::io::BufStream<RW> {
    type Stream = Self;
}

impl<T: AsyncWrite + Send + AsyncRead + 'static + Sync + Unpin + Debug> Wire for tokio::io::WriteHalf<T> {
    type Stream = IoSplit<T>;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Cannot to establish stream from WriteHalf",
            ))
        }
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

#[derive(Debug)]
struct ConsumeWire<T>(Option<T>);

impl<T: SplitStream> Unwire for ConsumeWire<T> {
    type Stream = T;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async move {
            if let Some(wire) = Option::take(&mut self.0) {
                return Ok(wire);
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Unable to consume a wire",
            ))
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for ConsumeWire<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Unable to poll_read from a consumed wire",
        )))
    }
}

impl<T: AsyncWrite + Send + Sync + Unpin + 'static, C: ConnectConfig> Wire for WireStream<T, C>
where
    C::Stream: SplitStream,
{
    type Stream = WireStream<C::Stream, C>;

    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            // establish connection with server.
            let peer = self.peer.as_ref().ok_or(std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "Wire doesn't have peer connect info",
            ))?;
            let connect_info = &peer.wire_info.connect_info;

            let stream: <C as ConnectConfig>::RawStream = peer.connect_config.connect_stream(connect_info).await?;
            let stream = peer.connect_config.enhance_stream(stream)?;
            if let Some(local_handle) = &peer.local_handle {
                // bi connect
                let (reply, rx) = oneshot::channel();
                local_handle
                    .send(WireListenerEvent::OutgoingWire {
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
    /// Convert the wire into a bi-channel or handle or receive
    pub async fn into<R: Unwiring>(self) -> Result<R, std::io::Error>
    where
        C: ConnectConfig<Stream = T>,
        Self: SplitStream,
    {
        // Convert the wire into a consumed wire, to enable stream fn taking ownership of the stream instead of unwiring
        // incoming wire.
        let mut consume = ConsumeWire(Some(self));
        consume.unwire::<R>().await
    }
    /// Convert the wire stream to bi wired
    pub fn wired<LocalEvent, RemoteEvent, H: HandleEvent<RemoteEvent>>(
        self,
        handle_event: H,
    ) -> Result<WiredHandle<LocalEvent>, std::io::Error>
    where
        LocalEvent: Wiring + Debug + 'static,
        RemoteEvent: Unwiring + Debug + 'static,
        Self: SplitStream,
    {
        let h = WiredServer::new(self, handle_event)?.run();
        Ok(h)
    }
}

pub struct WireChannel<Sender, Receiver> {
    pub sender: Sender,
    pub receiver: Receiver,
}

#[allow(dead_code)]
impl<S, R> WireChannel<S, R> {
    fn new(sender: S, receiver: R) -> Self {
        Self { sender, receiver }
    }
    /// Convert this into tuple
    pub fn into_inner(self) -> (S, R) {
        (self.sender, self.receiver)
    }
}

impl<S: Wiring + 'static, R: Unwiring + 'static> Unwiring
    for WireChannel<tokio::sync::mpsc::Sender<S>, tokio::sync::mpsc::Receiver<R>>
{
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let buffer = wire.bounded_buffer();
            let wire = wire.stream().await?;
            let (mut r, mut w) = wire.split()?;

            // create the sender channel side.
            let (sender, mut rx) = tokio::sync::mpsc::channel::<S>(buffer.into());

            let sender_task = async move {
                while let Some(item) = rx.recv().await {
                    if let Err(_) = w.wire(item).await {
                        rx.close();
                        break;
                    }
                }
                w.shutdown().await.ok();
            };
            let s_j = tokio::spawn(sender_task.boxed());
            // create receiver channel
            let (tx, receiver) = tokio::sync::mpsc::channel::<R>(buffer.into());
            let recv_task = async move {
                loop {
                    tokio::select! {
                        _ = tx.closed() => {
                            break;
                        },
                        item = r.unwire::<R>() => {
                            if let Ok(item) = item {
                                tx.send(item).await.ok();
                            } else {
                                break;
                            }

                        },
                    }
                }
                s_j.abort();
            };
            tokio::spawn(recv_task.boxed());
            Ok(Self::new(sender, receiver))
        }
    }
}

impl<S: Wiring + 'static, R: Unwiring + 'static> Unwiring for WireChannel<UnboundedSender<S>, UnboundedReceiver<R>> {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let wire = wire.stream().await?;
            let (mut r, mut w) = wire.split()?;

            // create the sender channel side.
            let (sender, mut rx) = tokio::sync::mpsc::unbounded_channel::<S>();

            let sender_task = async move {
                while let Some(item) = rx.recv().await {
                    if let Err(_) = w.wire(item).await {
                        rx.close();
                        break;
                    }
                }
                w.shutdown().await.ok();
            };
            let s_j = tokio::spawn(sender_task.boxed());
            // create receiver channel
            let (tx, receiver) = tokio::sync::mpsc::unbounded_channel::<R>();
            let recv_task = async move {
                loop {
                    tokio::select! {
                        _ = tx.closed() => {
                            break;
                        },
                        item = r.unwire::<R>() => {
                            if let Ok(item) = item {
                                tx.send(item).ok();
                            } else {
                                break;
                            }

                        },
                    }
                }
                s_j.abort();
            };
            tokio::spawn(recv_task.boxed());
            Ok(Self::new(sender, receiver))
        }
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
        let stream = self.config.enhance_stream(stream)?;

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
        stream: C::RawStream,
    ) -> Result<Option<WireStream<C::Stream, C>>, std::io::Error> {
        let mut stream = self.config.enhance_stream(stream)?;

        let remote_info = stream.unwire().await?;
        let forward_info = stream.unwire().await?;
        // stream = temp_wire.stream;
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
        let raw_stream = self.config.connect_stream(&connect_info).await?;
        let stream = self.config.enhance_stream(raw_stream)?;
        let (reply, rx) = oneshot::channel();

        let event = WireListenerEvent::OutgoingWire {
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

pub trait Wiring: Send + Sync + Sized {
    const SAFE: bool = true;

    const FIXED_SIZE: usize = 0;
    /// Indicates if type is mixed of dynamic and fixed fields.
    const MIXED: bool = true;
    #[allow(unused)]
    fn concat_array(&self, buf: &mut [u8]) {}

    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.wiring_ref(wire).await }
    }
    fn wiring_ref<W: Wire>(&self, wire: &mut W)
        -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send;
    fn wiring_slice<W: Wire>(
        slice: &[Self],
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let len = slice.len();
            len.wiring(wire).await?;
            for i in slice {
                i.wiring_ref(wire).await?;
            }
            Ok(())
        }
    }

    fn wiring_vec<W: Wire>(
        v: Vec<Self>,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let len = v.len();
            len.wiring(wire).await?;
            for i in v {
                i.wiring(wire).await?;
            }
            Ok(())
        }
    }
    #[inline]
    fn wiring_array_ref<W: Wire, const N: usize>(
        array: &[Self; N],
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            for i in array {
                i.wiring_ref(wire).await?;
            }
            Ok(())
        }
    }

    #[inline]
    fn wiring_array<W: Wire, const N: usize>(
        array: [Self; N],
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            for i in array {
                i.wiring(wire).await?;
            }
            Ok(())
        }
    }

    fn wiring_arc<W: Wire>(
        arc: Arc<Self>,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            // we simply use ref here
            let r = arc.as_ref();
            r.wiring_ref(wire).await
        }
    }

    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write;
    #[inline]
    fn sync_wiring_array<W: Wire, const N: usize>(array: &[Self; N], wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        for i in array {
            i.sync_wiring(wire)?;
        }
        Ok(())
    }
    #[inline]
    fn sync_wiring_slice<W: Wire>(slice: &[Self], wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        let len = slice.len();
        len.sync_wiring(wire)?;
        if Self::MIXED {
            for i in slice {
                i.sync_wiring(wire)?;
            }
        } else {
            // the type is fixed and we know the total size, so we reserve one time
            let add = len * Self::FIXED_SIZE;
            // reserve capcaity to ensure unsafe operation without boundchecks are safe
            wire.reserve_capacity(add);
            let mut unchecked = Unchecked { wire };
            for i in slice {
                i.sync_wiring(&mut unchecked)?;
            }
        }

        Ok(())
    }
}
impl Wiring for usize {
    const FIXED_SIZE: usize = 8;
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        (self as u64).wiring(wire)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        (*self as u64).wiring(wire)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        (*self as u64).sync_wiring(wire)
    }

    fn concat_array(&self, buf: &mut [u8]) {
        let r = (*self as u64).to_be_bytes();
        buf[..8].copy_from_slice(&r);
    }
}

impl Wiring for WireInfo {
    const FIXED_SIZE: usize = 0;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            self.wire_id.wiring(wire).await?;
            self.access_key.wiring(wire).await?;
            self.connect_info.wiring(wire).await
        }
    }
    #[inline]
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            self.wire_id.wiring_ref(wire).await?;
            self.access_key.wiring_ref(wire).await?;
            self.connect_info.wiring_ref(wire).await
        }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        self.wire_id.sync_wiring(wire)?;
        self.access_key.sync_wiring(wire)?;
        self.connect_info.sync_wiring(wire)
    }
    #[allow(unused)]
    fn concat_array(&self, buf: &mut [u8]) {}
}

impl Wiring for String {
    const FIXED_SIZE: usize = 0;

    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_bytes().wiring(wire).await }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_bytes().wiring(wire).await }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        let b = self.as_bytes();
        b.len().sync_wiring(wire)?;
        wire.sync_wire_all::<true>(b)
    }
}

impl<'a> Wiring for &'a str {
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_bytes().wiring(wire).await }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_bytes().wiring(wire).await }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        self.as_bytes().sync_wiring(wire)
    }
}

impl Wiring for char {
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { (self as u32).wiring(wire).await }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        (*self as u32).wiring(wire)
    }
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        (*self as u32).sync_wiring(wire)
    }
}

impl<'a, T: Wiring> Wiring for &'a [T] {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        T::wiring_slice(self, wire)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        T::wiring_slice(*self, wire)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        T::sync_wiring_slice(*self, wire)
    }
}

impl<'a, T: Wiring + 'static, const LEN: usize> Wiring for &'a [T; LEN] {
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        T::wiring_array_ref(self, wire)
    }

    #[inline]
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        T::wiring_array_ref(*self, wire)
    }

    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        T::sync_wiring_array(self, wire)
    }
}

impl<T: Wiring, const LEN: usize> Wiring for [T; LEN] {
    const FIXED_SIZE: usize = T::FIXED_SIZE * LEN;
    const MIXED: bool = T::MIXED;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        T::wiring_array(self, wire)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        T::sync_wiring_array(self, wire)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        T::wiring_array_ref(self, wire)
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        let mut start = 0;
        for elem in self.iter() {
            let end = start + T::FIXED_SIZE;
            elem.concat_array(&mut buf[start..end]);
            start = end;
        }
    }
}

impl Wiring for Url {
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_str().wiring(wire).await }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { self.as_str().wiring(wire).await }
    }
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        self.as_str().sync_wiring(wire)
    }
}

impl<T> Wiring for tokio::sync::oneshot::Sender<T>
where
    T: Unwiring + 'static,
{
    const SAFE: bool = false;
    #[inline]
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
    fn wiring_ref<W: Wire>(&self, _: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            Err(Error::new(
                ErrorKind::Unsupported,
                "Wiring oneshot sender by ref is not support",
            ))
        }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Wiring oneshot sender by ref is not supported",
        ))
    }
}

impl<T> Wiring for UnboundedSender<T>
where
    T: Unwiring + 'static,
{
    const SAFE: bool = false;
    #[inline]
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
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        let s = self.clone();
        async move { s.wiring(wire).await }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Sync wiring unboundedsender by ref is not supported",
        ))
    }
}

impl<T: Wiring + 'static + Clone> Wiring for tokio::sync::broadcast::Receiver<T> {
    /// not safe to store
    const SAFE: bool = false;
    #[inline]
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
    fn wiring_ref<W: Wire>(&self, _: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            Err(Error::new(
                ErrorKind::Unsupported,
                "Wiring broadcast receiver by ref is not supported",
            ))
        }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Sync Wiring broadcast receiver by ref is not supported",
        ))
    }
}

impl<T: Wiring + 'static + Clone> Wiring for tokio::sync::watch::Receiver<T> {
    /// not safe to store
    const SAFE: bool = false;
    #[inline]
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
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let r = self.clone();
            r.wiring(wire).await
        }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Sync Wiring watch receiver by ref is not supported",
        ))
    }
}

impl<T: Wiring + Unwiring + 'static + Clone> Wiring for tokio::sync::watch::Sender<T> {
    /// not safe to store
    const SAFE: bool = false;
    #[inline]
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
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let s = self.clone();
            s.wiring(wire).await
        }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Sync wiring watch sender is not supported",
        ))
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
                    // note: detecting when this otherhalf is dropped is impossible, as tokio doesn't provide closed()
                    if let Err(_) = self.send(item) {
                        break;
                    };
                }
            };
            tokio::spawn(task.boxed());
            Ok(())
        }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let s = self.clone();
            s.wiring(wire).await
        }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Wiring broadcast sender is not supported",
        ))
    }
}

impl<T> Wiring for tokio::sync::mpsc::Sender<T>
where
    T: Unwiring + 'static,
{
    const SAFE: bool = false;
    #[inline]
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
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let s = self.clone();
            s.wiring(wire).await
        }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Sync wiring mpsc sender is not supported",
        ))
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
    fn wiring_ref<W: Wire>(&self, _: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            Err(Error::new(
                ErrorKind::Unsupported,
                "Wiring oneshot receiver by ref is not supported",
            ))
        }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Sync Wiring oneshot receiver is not supported",
        ))
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
    fn wiring_ref<W: Wire>(&self, _: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            Err(Error::new(
                ErrorKind::Unsupported,
                "Wiring unbounded receiver by ref is not supported",
            ))
        }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Sync Wiring unbounded receiver is not supported",
        ))
    }
}

impl<T: Wiring + 'static> Wiring for tokio::sync::mpsc::Receiver<T> {
    const SAFE: bool = false;
    #[inline]
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
    fn wiring_ref<W: Wire>(&self, _: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            Err(Error::new(
                ErrorKind::Unsupported,
                "Wiring bounded receiver by ref is not supported",
            ))
        }
    }
    fn sync_wiring<W: Wire>(&self, _: &mut W) -> Result<(), std::io::Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "Sync Wiring bounded receiver is not supported",
        ))
    }
}

impl Wiring for () {
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        // Must be wired as rust treats this as type even if it's zerosized. the reason:
        // channel can support sending (), and therefore the other end must detects when () is sent,
        // if it was no-op, then no way for unwire to detect it. think of it as heartbeat.
        wire.wiring(0u8)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        0u8.wiring(wire)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        0u8.sync_wiring(wire)
    }
}

impl Wiring for bool {
    const FIXED_SIZE: usize = 1;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u8(self as u8)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_u8(*self as u8)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        (*self as u8).sync_wiring(wire)
    }
    fn concat_array(&self, buf: &mut [u8]) {
        buf[0] = *self as u8;
    }
}

impl Wiring for u8 {
    const FIXED_SIZE: usize = 1;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u8(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_u8(*self)
    }
    fn wiring_slice<W: Wire>(
        slice: &[Self],
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            slice.len().wiring(wire).await?;
            wire.write_all(slice).await
        }
    }
    fn wiring_array<W: Wire, const N: usize>(
        array: [Self; N],
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move { wire.write_all(&array).await }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(&[*self])
    }
    #[inline(always)]
    fn sync_wiring_slice<W: Wire>(slice: &[Self], wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        slice.len().sync_wiring(wire)?;
        wire.sync_wire_all::<true>(slice)
    }
    #[inline]
    fn sync_wiring_array<W: Wire, const N: usize>(array: &[Self; N], wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(array)
    }

    #[inline(always)]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[0] = *self;
    }
}

impl Wiring for i8 {
    const FIXED_SIZE: usize = 1;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i8(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_i8(*self)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(&[*self as u8])
    }
    fn concat_array(&self, buf: &mut [u8]) {
        buf[0] = *self as u8
    }
}

impl Wiring for u16 {
    const FIXED_SIZE: usize = 2;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u16(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_u16(*self)
    }
    #[inline(always)]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(&self.to_be_bytes())
    }
    #[inline(always)]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[..2].copy_from_slice(&self.to_be_bytes())
    }
}

impl Wiring for i16 {
    const FIXED_SIZE: usize = 2;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i16(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_i16(*self)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(&self.to_be_bytes())
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[..2].copy_from_slice(&self.to_be_bytes())
    }
}

impl Wiring for u32 {
    const FIXED_SIZE: usize = 4;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u32(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_u32(*self)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(&self.to_be_bytes())
    }
    #[inline]
    fn sync_wiring_array<W: Wire, const N: usize>(array: &[Self; N], wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        // we inilize the t_be_bytes for the array and do it in one go?
        let c = array.map(|n| n.to_be_bytes());
        // instead we concat them
        let r = c.as_slice().as_flattened();
        wire.sync_wire_all::<true>(r)?;
        Ok(())
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[..std::mem::size_of::<Self>()].copy_from_slice(&self.to_be_bytes())
    }
}

impl Wiring for i32 {
    const FIXED_SIZE: usize = 4;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i32(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_i32(*self)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.write_all(&self.to_be_bytes())
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[..std::mem::size_of::<Self>()].copy_from_slice(&self.to_be_bytes())
    }
}

impl Wiring for f32 {
    const FIXED_SIZE: usize = 4;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_f32(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_f32(*self)
    }
    #[inline(always)]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        // wire.write_all(&self.to_be_bytes())
        wire.sync_wire_f32(self)
    }
    #[inline(always)]
    fn concat_array(&self, buf: &mut [u8]) {
        // let bl = bytes.len();
        // self.reserve(bl);
        // let l = self.len();

        unsafe {
            std::ptr::copy_nonoverlapping(self.to_be_bytes().as_ptr(), buf.as_mut_ptr(), 4);
        }

        // buf[..std::mem::size_of::<Self>()].copy_from_slice(&self.to_be_bytes())
    }
}

impl Wiring for u64 {
    const FIXED_SIZE: usize = 8;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u64(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_u64(*self)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        // wire.write_all(&self.to_be_bytes())
        // wire.sync_wire_all(&self.to_be_bytes())
        wire.sync_wire_u64(self)
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[..std::mem::size_of::<Self>()].copy_from_slice(&self.to_be_bytes())
    }
}

impl Wiring for i64 {
    const FIXED_SIZE: usize = 8;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i64(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_i64(*self)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(&self.to_be_bytes())
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[..std::mem::size_of::<Self>()].copy_from_slice(&self.to_be_bytes())
    }
}

impl Wiring for f64 {
    const FIXED_SIZE: usize = 8;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_f64(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_f64(*self)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(&self.to_be_bytes())
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[..std::mem::size_of::<Self>()].copy_from_slice(&self.to_be_bytes())
    }
}

impl Wiring for u128 {
    const FIXED_SIZE: usize = 16;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_u128(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_u128(*self)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(&self.to_be_bytes())
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[..std::mem::size_of::<Self>()].copy_from_slice(&self.to_be_bytes())
    }
}

impl Wiring for i128 {
    const FIXED_SIZE: usize = 16;
    const MIXED: bool = false;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        wire.write_i128(self)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        wire.write_i128(*self)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        wire.sync_wire_all::<true>(&self.to_be_bytes())
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        buf[..std::mem::size_of::<Self>()].copy_from_slice(&self.to_be_bytes())
    }
}

impl<T> Wiring for Box<[T]>
where
    for<'a> T: Wiring + 'a,
{
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let vec = self.into_vec();
            vec.wiring(wire).await
        }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let vec = &**self;
            vec.wiring(wire).await
        }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        let vec = &**self;
        vec.sync_wiring(wire)
    }
}

impl<T> Wiring for Box<T>
where
    for<'a> T: Wiring + 'a,
{
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let inner: T = *self;
            inner.wiring(wire).await
        }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let inner = &**self;
            inner.wiring_ref(wire).await
        }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        let inner = &**self;
        inner.sync_wiring(wire)
    }
}

impl<T> Wiring for std::sync::Arc<T>
where
    T: Wiring,
{
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        T::wiring_arc(self, wire)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let r = self.as_ref();
            r.wiring_ref(wire).await
        }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        let r = self.as_ref();
        r.sync_wiring(wire)
    }
}

impl<T: Wiring> Wiring for Vec<T> {
    const FIXED_SIZE: usize = 0;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        T::wiring_vec(self, wire)
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        T::wiring_slice(self, wire)
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        T::sync_wiring_slice(self, wire)
    }
}

impl<T: Wiring> Wiring for std::collections::HashSet<T> {
    #[inline]
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
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            let len = self.len();
            len.wiring(wire).await?;
            let mut s = self.iter();
            while let Some(t) = s.next() {
                t.wiring_ref(wire).await?;
            }
            Ok(())
        }
    }
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        let len = self.len();
        len.sync_wiring(wire)?;
        let mut s = self.iter();
        while let Some(t) = s.next() {
            t.sync_wiring(wire)?;
        }
        Ok(())
    }
}

impl<T: Wiring> Wiring for Option<T> {
    #[inline]
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
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            if let Some(t) = self {
                1u8.wiring(wire).await?;
                t.wiring_ref(wire).await
            } else {
                0u8.wiring(wire).await
            }
        }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        if let Some(t) = self {
            1u8.sync_wiring(wire)?;
            t.sync_wiring(wire)
        } else {
            0u8.sync_wiring(wire)
        }
    }
}

#[cfg(feature = "generic_const_exprs")]
impl<T: Wiring, TT: Wiring> Wiring for (T, TT)
where
    [(); T::FIXED_SIZE + TT::FIXED_SIZE]:,
{
    const FIXED_SIZE: usize = T::FIXED_SIZE + TT::FIXED_SIZE;
    const MIXED: bool = T::MIXED || T::MIXED;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async {
            self.0.wiring(wire).await?;
            self.1.wiring(wire).await
        }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            (&self.0).wiring_ref(wire).await?;
            (&self.1).wiring_ref(wire).await
        }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        // if both T and TT are fixed, we better concate them.
        if !T::MIXED && !TT::MIXED {
            let mut buf = [0u8; T::FIXED_SIZE + TT::FIXED_SIZE];
            self.concat_array(&mut buf);
            wire.sync_wire_all::<true>(&buf)?;
        } else {
            (&self.0).sync_wiring(wire)?;
            (&self.1).sync_wiring(wire)?;
        }
        Ok(())
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        let (left, buf) = buf.split_at_mut(T::FIXED_SIZE);
        self.0.concat_array(left);
        self.1.concat_array(buf);
    }
}
#[cfg(not(feature = "generic_const_exprs"))]
impl<T: Wiring, TT: Wiring> Wiring for (T, TT) {
    const FIXED_SIZE: usize = T::FIXED_SIZE + TT::FIXED_SIZE;
    const MIXED: bool = T::MIXED || T::MIXED;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async {
            self.0.wiring(wire).await?;
            self.1.wiring(wire).await
        }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            (&self.0).wiring_ref(wire).await?;
            (&self.1).wiring_ref(wire).await
        }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        (&self.0).sync_wiring(wire)?;
        (&self.1).sync_wiring(wire)
    }
    #[inline]
    fn concat_array(&self, buf: &mut [u8]) {
        let (left, buf) = buf.split_at_mut(T::FIXED_SIZE);
        self.0.concat_array(left);
        self.1.concat_array(buf);
    }
}

#[cfg(feature = "generic_const_exprs")]
impl<T: Wiring, TT: Wiring, TTT: Wiring> Wiring for (T, TT, TTT)
where
    [(); T::FIXED_SIZE + TT::FIXED_SIZE + TTT::FIXED_SIZE]:,
{
    const FIXED_SIZE: usize = T::FIXED_SIZE + TT::FIXED_SIZE + TTT::FIXED_SIZE;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async {
            self.0.wiring(wire).await?;
            self.1.wiring(wire).await?;
            self.2.wiring(wire).await
        }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            (&self.0).wiring_ref(wire).await?;
            (&self.1).wiring_ref(wire).await?;
            (&self.2).wiring_ref(wire).await
        }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        if !T::MIXED && !TT::MIXED && !TTT::MIXED {
            let mut buf = [0u8; T::FIXED_SIZE + TT::FIXED_SIZE + TTT::FIXED_SIZE];
            self.concat_array(&mut buf);
            wire.sync_wire_all::<true>(&buf)?;
        } else {
            (&self.0).sync_wiring(wire)?;
            (&self.1).sync_wiring(wire)?;
            (&self.2).sync_wiring(wire)?;
        }
        Ok(())
    }
    #[inline(always)]
    fn concat_array(&self, buf: &mut [u8]) {
        self.0.concat_array(&mut buf[..T::FIXED_SIZE]);
        self.1
            .concat_array(&mut buf[T::FIXED_SIZE..T::FIXED_SIZE + TT::FIXED_SIZE]);
        self.2.concat_array(&mut buf[T::FIXED_SIZE + TT::FIXED_SIZE..]);
    }
}

#[cfg(not(feature = "generic_const_exprs"))]
impl<T: Wiring, TT: Wiring, TTT: Wiring> Wiring for (T, TT, TTT) {
    const FIXED_SIZE: usize = T::FIXED_SIZE + TT::FIXED_SIZE + TTT::FIXED_SIZE;
    #[inline]
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> {
        async {
            self.0.wiring(wire).await?;
            self.1.wiring(wire).await?;
            self.2.wiring(wire).await
        }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            (&self.0).wiring_ref(wire).await?;
            (&self.1).wiring_ref(wire).await?;
            (&self.2).wiring_ref(wire).await
        }
    }
    #[inline]
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: Write,
    {
        (&self.0).sync_wiring(wire)?;
        (&self.1).sync_wiring(wire)?;
        (&self.2).sync_wiring(wire)
    }
    #[inline(always)]
    fn concat_array(&self, buf: &mut [u8]) {
        self.0.concat_array(&mut buf[..T::FIXED_SIZE]);
        self.1
            .concat_array(&mut buf[T::FIXED_SIZE..T::FIXED_SIZE + TT::FIXED_SIZE]);
        self.2.concat_array(&mut buf[T::FIXED_SIZE + TT::FIXED_SIZE..]);
    }
}

/// BufWire is in memory fast buffer/wire
pub struct BufWire<'a> {
    wire: &'a mut Vec<u8>,
}

impl<'a> BufWire<'a> {
    /// Create new buffer, it also set the len to zero.
    pub fn new(wire: &'a mut Vec<u8>) -> Self {
        // the moment we take the wire we set the len to be zero.
        unsafe {
            wire.set_len(0);
        }
        Self { wire }
    }
    /// Wire value T to the bufwire
    pub fn wire<T: Wiring>(&'a mut self, value: &T) -> Result<(), std::io::Error> {
        self.wire.sync_wire(value)
    }
}
