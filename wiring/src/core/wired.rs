use std::{
    collections::HashMap, convert::Infallible, fmt::Debug, marker::PhantomData, pin::Pin, sync::Arc, task::Poll,
};

use futures::FutureExt;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc::{error::SendError, unbounded_channel, UnboundedReceiver, UnboundedSender},
};

use super::{
    unwire::{Unwire, Unwiring},
    wire::{Wire, Wiring},
    SplitStream, WireId,
};

#[pin_project::pin_project]
#[derive(Debug)]
/// The wired
pub struct Wired<W: Wire, R: Wiring, Res: Unwiring = R> {
    /// the inner stream over network
    /// Used for internal wired communication
    #[pin]
    wire: W,
    /// recent_stream_id, we use incremental stream_id
    stream_id: u64,
    /// Active streams.
    streams: HashMap<(WireId, bool), UnboundedReceiver<Vec<u8>>>,
    handle: std::sync::Arc<UnboundedSender<Option<WiredEvent<R>>>>,
    unwired_handle: std::sync::Arc<UnboundedSender<Option<UnwiredEvent>>>,
    _m: PhantomData<Res>,
}

#[pin_project::pin_project]
#[derive(Debug)]

/// Unwired receives events from remote
pub struct Unwired<W: Unwire, H: HandleEvent<Res>, Req: Wiring, Res: Unwiring = Req> {
    #[pin]
    unwire: W,
    handle: std::sync::Arc<UnboundedSender<Option<UnwiredEvent>>>,
    wired_handle: std::sync::Arc<UnboundedSender<Option<WiredEvent<Req>>>>,
    handle_event: H,
    _m: PhantomData<Res>,
}

pub trait HandleEvent<E>: Send + Sync + 'static {
    type Error: std::error::Error;
    fn handle_event(&mut self, event: E) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;
}

impl<E, T> HandleEvent<E> for Option<T>
where
    E: Send + Sync + 'static,
    T: HandleEvent<E>,
{
    type Error = T::Error;
    async fn handle_event(&mut self, event: E) -> Result<(), Self::Error> {
        if let Some(inner) = self.as_mut() {
            inner.handle_event(event).await?;
        }
        Ok(())
    }
}

// a noop handler
impl HandleEvent<()> for () {
    type Error = Infallible;
    async fn handle_event(&mut self, _event: ()) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<E> HandleEvent<E> for UnboundedSender<E>
where
    E: Send + Sync + 'static,
{
    type Error = SendError<E>;
    async fn handle_event(&mut self, event: E) -> Result<(), Self::Error> {
        self.send(event)
    }
}

impl<W, H, R, Res> Unwired<W, H, R, Res>
where
    W: Unwire,
    H: HandleEvent<Res>,
    R: Wiring,
    Res: Unwiring,
    Self: Unwire,
{
    async fn connect(&mut self) -> std::io::Result<InternalStream<R>> {
        let stream_id = self.unwiring::<WireId>().await.map_err(|e| e)?;

        let (tx, rx) = unbounded_channel();

        let handle = WiredStreamHandle {
            stream_id,
            local: false,
            handle: self.wired_handle.clone(),
        };

        self.wired_handle
            .send(WiredEvent::RegisterWriteHalf { stream_id, rx }.into())
            .ok();

        let write_half = InternalWriteHalf::new(handle, tx);

        let (tx, rx) = unbounded_channel();

        let handle = WiredStreamHandle {
            stream_id,
            local: false,
            handle: self.wired_handle.clone(),
        };

        let read_half = InternalReadHalf::new(handle, rx);
        let event = UnwiredEvent::RegisterStream {
            stream_id,
            local: false,
            tx,
        };

        self.handle.send(Some(event)).ok();

        Ok(InternalStream {
            read: read_half,
            write: write_half,
        })
    }
}
impl<W: Unwire, H, R, Res> Unwire for Unwired<W, H, R, Res>
where
    W: Unwire,
    H: HandleEvent<Res>,
    R: Wiring + Debug + 'static + Send,
    Res: Unwiring,
{
    type Stream = InternalStream<R>;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async move { self.connect().await }
    }
}

impl<W: Unwire, H: HandleEvent<Res>, R: Wiring, Res: Unwiring> AsyncRead for Unwired<W, H, R, Res> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.project().unwire.poll_read(cx, buf)
    }
}

impl<W: Wire, R: Wiring, Res: Unwiring> Wired<W, R, Res> {
    /// Create new stream and insert it it, then wire the stream to remote unwire,
    async fn connect(&mut self) -> std::io::Result<InternalStream<R>> {
        self.stream_id += 1;
        let stream_id = self.stream_id;

        let stream_handle = WiredStreamHandle::<R> {
            stream_id,
            local: true,
            handle: self.handle.clone(),
        };

        let (tx, rx) = unbounded_channel();

        let write_half = InternalWriteHalf::new(stream_handle, tx);
        let key = (stream_id, true);

        self.streams.insert(key, rx);

        let stream_handle = WiredStreamHandle::<R> {
            stream_id,
            local: true,
            handle: self.handle.clone(),
        };

        let (tx, rx) = unbounded_channel();
        // we tell unwire to register the tx in-case they received any message to be sent to ;
        let register = UnwiredEvent::RegisterStream {
            stream_id,
            local: true,
            tx,
        };

        self.unwired_handle.send(register.into()).ok();

        let read_half = InternalReadHalf::new(stream_handle, rx);
        let internal_stream = InternalStream {
            read: read_half,
            write: write_half,
        };

        self.wire.wiring(stream_id).await?;
        Ok(internal_stream)
    }
}

impl<W: Wire, R: Wiring, Res: Unwiring> Wire for Wired<W, R, Res>
where
    R: Debug + Send + 'static + Sync,
    Res: Send + Sync + 'static,
{
    type Stream = InternalStream<R>;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async move { self.connect().await }
    }
}

impl<W: Wire, R: Wiring, Res: Unwiring> AsyncWrite for Wired<W, R, Res> {
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.wire.is_write_vectored()
    }
    #[inline]
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().wire.poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().wire.poll_shutdown(cx)
    }
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().wire.poll_write(cx, buf)
    }
    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().wire.poll_write_vectored(cx, bufs)
    }
}

#[pin_project::pin_project]
#[derive(Debug)]
pub struct InternalWriteHalf<T> {
    // the handle is used to send messages to wire, and drop the wire.
    handle: WiredStreamHandle<T>,
    #[pin]
    buf: std::io::Cursor<Vec<u8>>,
    /// a buffer provided by wired to enables us pushing our message.
    /// Techincally it's not required, but it provides a proxy with wired.
    tx: UnboundedSender<Vec<u8>>,
}

impl<T> InternalWriteHalf<T> {
    fn new(handle: WiredStreamHandle<T>, tx: UnboundedSender<Vec<u8>>) -> Self {
        Self {
            handle,
            buf: Default::default(),
            tx,
        }
    }
}
impl<T: Send + Sync + 'static> AsyncWrite for InternalWriteHalf<T> {
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.buf.is_write_vectored()
    }
    #[inline]
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let mut p = self.as_mut().project();

        let r = p.buf.as_mut().poll_flush(cx);
        match r {
            Poll::Pending => Poll::Pending,
            Poll::Ready(r) => {
                if r.is_err() {
                    return Poll::Ready(r);
                }
                let m = p.buf.as_mut().get_mut().get_mut();
                let message = std::mem::take(m);

                let new_buf_for_next_messge = Vec::with_capacity(message.len());
                self.buf = std::io::Cursor::new(new_buf_for_next_messge);
                // send message to our wire to be wire it to remote
                let event = WiredEvent::Message {
                    stream_id: self.handle.stream_id,
                    local: self.handle.local,
                };

                // we send the mesage to the shared buffer buffer,
                let r = self.tx.send(message);
                if let Err(e) = r {
                    return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, e)));
                }
                // if client buffer is actually bounded, then
                let r = self.handle.handle.send(event.into());
                // to allow dropping inner
                if let Err(e) = r {
                    return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, e)));
                }
                Poll::Ready(Ok(()))
            }
        }
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.handle
            .handle
            .send(
                WiredEvent::Drop {
                    stream_id: self.handle.stream_id,
                    local: self.handle.local,
                    broadcast_to_remote: true,
                }
                .into(),
            )
            .ok();
        self.project().buf.poll_shutdown(cx)
    }
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let p = self.project();
        p.buf.poll_write(cx, buf)
    }
    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let p = self.project();
        p.buf.poll_write_vectored(cx, bufs)
    }
}

impl<T: Send + Sync + Debug + 'static> Wire for InternalWriteHalf<T> {
    type Stream = InternalStream<T>;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async move {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "You cannot stream nested wire over wired stream",
            ))
        }
    }
}

#[pin_project::pin_project]
#[derive(Debug)]
pub struct InternalStream<T> {
    #[pin]
    read: InternalReadHalf<T>,
    #[pin]
    write: InternalWriteHalf<T>,
}

impl<T: Send + Sync> AsyncRead for InternalStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.project().read.poll_read(cx, buf)
    }
}

impl<T: Send + Sync + 'static> AsyncWrite for InternalStream<T> {
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.write.is_write_vectored()
    }
    #[inline]
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().write.poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().write.poll_shutdown(cx)
    }
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().write.poll_write(cx, buf)
    }
    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().write.poll_write_vectored(cx, bufs)
    }
}

impl<T: Send + Sync + Debug + 'static> Wire for InternalStream<T> {
    type Stream = InternalStream<T>;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async move {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Unable to create internalstream from internalstream",
            ))
        }
    }
}

impl<T: Send + Sync + Debug + 'static> Unwire for InternalStream<T> {
    type Stream = InternalStream<T>;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async move {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Unable to create internalstream from internalstream",
            ))
        }
    }
}

impl<T> SplitStream for InternalStream<T>
where
    T: Send + Sync + Debug + 'static,
{
    type Unwire = InternalReadHalf<T>;
    type Wire = InternalWriteHalf<T>;
    fn split(self) -> Result<(Self::Unwire, Self::Wire), std::io::Error> {
        Ok((self.read, self.write))
    }
}

#[pin_project::pin_project]
#[derive(Debug)]
pub struct InternalReadHalf<T> {
    // the handle is used to send messages to wire, and drop the wire.
    handle: WiredStreamHandle<T>,
    #[pin]
    buf: std::io::Cursor<Vec<u8>>,
    /// Received new message from remote passed through somehow :)
    rx: UnboundedReceiver<Vec<u8>>,
}

impl<T> InternalReadHalf<T> {
    fn new(handle: WiredStreamHandle<T>, rx: UnboundedReceiver<Vec<u8>>) -> Self {
        Self {
            handle,
            buf: Default::default(),
            rx,
        }
    }
}

impl<T: Send + Sync + Debug + 'static> Unwire for InternalReadHalf<T> {
    type Stream = InternalStream<T>;
}

impl<T: Send + Sync> AsyncRead for InternalReadHalf<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut p = self.as_mut().project();
        if p.buf.get_ref().len() == 0 {
            let new = p.rx.poll_recv(cx);
            match new {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(new_msg)) => {
                    *p.buf.as_mut() = std::io::Cursor::new(new_msg);
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())),
            }
        }
        p.buf.as_mut().poll_read(cx, buf)
    }
}
// the remote can send unwire. todo create two type for unwired and wired, where it checks if is already dropped.
// tx.closed()
#[derive(Debug)]
pub struct WiredStreamHandle<T> {
    stream_id: WireId,
    local: bool,
    handle: std::sync::Arc<UnboundedSender<Option<WiredEvent<T>>>>,
    // to
}

impl<T> Drop for WiredStreamHandle<T> {
    fn drop(&mut self) {
        let stream_id = self.stream_id;
        let local = self.local;
        // the drop of stream handle is sent to RpcEvent which is the unwire event?
        let drop = WiredEvent::Drop {
            stream_id,
            local,
            broadcast_to_remote: true,
        };
        self.handle.send(drop.into()).ok();
    }
}

#[derive(Debug)]
/// The event we're expecting to wire it to remote node.
pub enum WiringEvent<T> {
    /// The event is also part of wiring
    Event(T),
    /// Received/Sent message to be forwared.
    Message {
        stream_id: WireId,
        local: bool,
        message: Vec<u8>,
    },
    /// when this is sent. it might belong to local or remote, which is adjust
    Drop {
        stream_id: WireId,
        /// the local flag is flipped in wired before broadcast.
        local: bool,
    },
}

impl<T: Wiring> Wiring for WiringEvent<T> {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            match self {
                Self::Event(event) => {
                    0u8.wiring(wire).await?;
                    event.wiring(wire).await?;
                }
                Self::Message {
                    stream_id,
                    local,
                    message,
                } => {
                    1u8.wiring(wire).await?;
                    stream_id.wiring(wire).await?;
                    local.wiring(wire).await?;
                    message.wiring(wire).await?;
                }
                Self::Drop { stream_id, local } => {
                    2u8.wiring(wire).await?;
                    stream_id.wiring(wire).await?;
                    local.wiring(wire).await?;
                }
            }
            Ok(())
        }
    }
    fn wiring_ref<W: Wire>(
        &self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            match self {
                Self::Event(event) => {
                    0u8.wiring(wire).await?;
                    event.wiring_ref(wire).await?;
                }
                Self::Message {
                    stream_id,
                    local,
                    message,
                } => {
                    1u8.wiring(wire).await?;
                    stream_id.wiring_ref(wire).await?;
                    local.wiring_ref(wire).await?;
                    message.wiring_ref(wire).await?;
                }
                Self::Drop { stream_id, local } => {
                    2u8.wiring(wire).await?;
                    stream_id.wiring_ref(wire).await?;
                    local.wiring_ref(wire).await?;
                }
            }
            Ok(())
        }
    }
    fn sync_wiring<W: Wire>(&self, wire: &mut W) -> Result<(), std::io::Error>
    where
        W: std::io::prelude::Write,
    {
        match self {
            Self::Event(event) => {
                0u8.sync_wiring(wire)?;
                event.sync_wiring(wire)?;
            }
            Self::Message {
                stream_id,
                local,
                message,
            } => {
                1u8.sync_wiring(wire)?;
                stream_id.sync_wiring(wire)?;
                local.sync_wiring(wire)?;
                message.sync_wiring(wire)?;
            }
            Self::Drop { stream_id, local } => {
                2u8.sync_wiring(wire)?;
                stream_id.sync_wiring(wire)?;
                local.sync_wiring(wire)?;
            }
        }
        Ok(())
    }
}

impl<T: Unwiring> Unwiring for WiringEvent<T> {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let variant: u8 = wire.unwiring().await.map_err(|e| e)?;

            let r = match variant {
                0u8 => {
                    let event = T::unwiring(wire).await.map_err(|e| e)?;

                    Self::Event(event)
                }
                1u8 => Self::Message {
                    stream_id: wire.unwiring().await?,
                    local: wire.unwiring().await?,
                    message: wire.unwiring().await?,
                },
                2u8 => Self::Drop {
                    stream_id: wire.unwiring().await?,
                    local: wire.unwiring().await?,
                },
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Unexpected Wiringevent variant",
                    ));
                }
            };
            Ok(r)
        }
    }
}

#[derive(Debug)]
pub enum WiredEvent<T> {
    /// The event
    Event(T),
    /// Register write half of established stream by remote.
    RegisterWriteHalf {
        /// this is pushed by unwire connect
        stream_id: WireId,
        rx: UnboundedReceiver<Vec<u8>>,
    },
    /// A message pushed to wire we get it from middle channel.
    Message { stream_id: WireId, local: bool },
    /// Drop a stream, where stream-id and local are the key, and drop_source if it was from remote unwire
    Drop {
        stream_id: WireId,
        local: bool,
        // highligh if drop was a result of drop from remote client disconnecting
        broadcast_to_remote: bool,
    },
}

#[derive(Debug)]
pub enum UnwiredEvent {
    RegisterStream {
        stream_id: WireId,
        local: bool,
        tx: UnboundedSender<Vec<u8>>,
    },
    /// Drop exisiting stream by simply dropping the status multiple times
    /// status must dropped by all parties to inidcate it's dropped.
    Drop {
        stream_id: WireId,
        local: bool,
        broadcast_to_wire: bool,
    },
    /// Forward message to stream
    /// Sent by unwired
    Message {
        stream_id: WireId,
        local: bool,
        message: Vec<u8>,
    },
}

#[derive(Debug)]
pub struct WiredServer<S, H, R, Res = R>
where
    S: SplitStream,
    R: Wiring,
    Res: Unwiring,
    H: HandleEvent<Res>,
{
    wired: Wired<S::Wire, R, Res>,
    unwired: Unwired<S::Unwire, H, R, Res>,
    u_rx: UnboundedReceiver<Option<UnwiredEvent>>,
    w_rx: UnboundedReceiver<Option<WiredEvent<R>>>,
}

#[derive(Debug)]
pub struct WiredHandle<R> {
    handle: UnboundedSender<Option<WiredEvent<R>>>,
    drophandles: Option<DropHandles>,
}

impl<R> Clone for WiredHandle<R>
where
    R: Wiring,
{
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            drophandles: None,
        }
    }
}

impl<R: Wiring> WiredHandle<R> {
    /// Send event to remote
    pub fn send<T: Into<Option<R>>>(&self, message: T) -> Result<(), SendError<Option<R>>> {
        let message: Option<R> = message.into();
        let m = message.map(|m| WiredEvent::Event(m));
        let r = self.handle.send(m);
        match r {
            Err(SendError(Some(WiredEvent::Event(event)))) => {
                let message = event.into();
                return Err(SendError(message));
            }
            Err(SendError(_)) => {
                return Err(SendError(None));
            }
            Ok(()) => return Ok(()),
        }
    }
    /// Shutdown
    pub fn shutdown(mut self) -> impl std::future::Future<Output = Option<()>> + Send {
        self.send(None).ok();
        async move {
            if let Some(mut d) = self.drophandles.take() {
                if let Some(wired) = d.wired.take() {
                    wired.await.ok();
                };
                if let Some(unwired) = d.unwired.take() {
                    unwired.await.ok();
                };
                Some(())
            } else {
                None
            }
        }
    }
}

#[derive(Debug)]
pub struct DropHandles {
    wired: Option<tokio::task::JoinHandle<()>>,
    unwired: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for DropHandles {
    fn drop(&mut self) {
        if let Some(wired) = self.wired.as_ref() {
            wired.abort();
        }
        if let Some(unwired) = self.unwired.as_ref() {
            unwired.abort();
        }
    }
}

#[allow(dead_code)]
impl DropHandles {
    /// Abort both wired and unwired.
    pub fn abort(mut self) -> impl std::future::Future<Output = ()> + Send {
        if let Some(wired) = self.wired.as_ref() {
            wired.abort();
        }
        if let Some(unwired) = self.unwired.as_ref() {
            unwired.abort();
        }
        async move {
            if let Some(wired) = self.wired.take() {
                wired.await.ok();
            };
            if let Some(unwired) = self.unwired.take() {
                unwired.await.ok();
            };
        }
    }
}

impl<S, H, LocalEvent, RemoteEvent> WiredServer<S, H, LocalEvent, RemoteEvent>
where
    S: SplitStream,
    H: HandleEvent<RemoteEvent>,
    LocalEvent: Wiring + Debug + Send + Sync + 'static,
    RemoteEvent: Unwiring + Debug + Send + 'static,
{
    /// Create new wiredserver
    pub fn new(stream: S, handle_event: H) -> std::io::Result<Self> {
        let (unwire, wire) = stream.split()?;
        let (tx, w_rx) = unbounded_channel();
        let handle = Arc::new(tx);
        let (u_tx, u_rx) = unbounded_channel();
        let unwired_handle = Arc::new(u_tx);
        let unwired = Unwired {
            unwire,
            handle: unwired_handle.clone(),
            wired_handle: handle.clone(),
            handle_event,
            _m: PhantomData,
        };
        let wired = Wired {
            wire,
            stream_id: 0,
            streams: HashMap::new(),
            handle: handle,
            unwired_handle,
            _m: PhantomData,
        };

        Ok(Self {
            wired,
            unwired,
            u_rx,
            w_rx,
        })
    }

    /// Run the server and return Wiredhandle to start sending requests to remote
    pub fn run(self) -> WiredHandle<LocalEvent> {
        let unwired = self.unwired;
        let u_rx = self.u_rx;

        let w_tx = self.wired.handle.clone();
        let w = w_tx.clone();
        let uwired_task = async move {
            unwired.run(u_rx).await;
            w.send(None).ok();
        };
        let u_j = tokio::spawn(uwired_task.boxed());

        let wired = self.wired;
        let w_rx = self.w_rx;

        let wired_task = async move {
            wired.run(w_rx).await;
        };

        let w_j = tokio::spawn(wired_task.boxed());

        let d = DropHandles {
            wired: Some(w_j),
            unwired: Some(u_j),
        };

        let h = WiredHandle {
            handle: w_tx.as_ref().clone(),
            drophandles: Some(d),
        };
        h
    }
}

impl<W, R, Res> Wired<W, R, Res>
where
    W: Wire,
    R: Wiring,
    Res: Unwiring,
    Self: Wire,
{
    async fn run(mut self, mut rx: UnboundedReceiver<Option<WiredEvent<R>>>) {
        while let Some(e) = rx.recv().await {
            let Some(event) = e else {
                rx.close();
                continue;
            };
            match event {
                WiredEvent::Event(event) => {
                    let wiring_event = WiringEvent::Event(event);
                    if let Err(_) = self.wire(wiring_event).await {
                        rx.close();
                    };
                }
                WiredEvent::RegisterWriteHalf { stream_id, rx } => {
                    let key = (stream_id, false);
                    self.streams.insert(key, rx);
                }
                WiredEvent::Drop {
                    stream_id,
                    local,
                    broadcast_to_remote,
                } => {
                    let key = (stream_id, local);
                    if let Some(_) = self.streams.remove(&key) {
                        if broadcast_to_remote {
                            let wiring_event = WiringEvent::<R>::Drop {
                                stream_id,
                                local: !local,
                            };

                            if let Err(_) = self.wire(wiring_event).await {
                                rx.close();
                            };
                        }
                        let drop = UnwiredEvent::Drop {
                            stream_id,
                            local,
                            broadcast_to_wire: false,
                        };
                        self.unwired_handle.send(drop.into()).ok();
                    }
                }

                WiredEvent::Message { stream_id, local } => {
                    let key = (stream_id, local);
                    if let Some(s_rx) = self.streams.get_mut(&key) {
                        if let Some(message) = s_rx.try_recv().ok() {
                            // when sending message, if it's local here, then in remote should be remote.
                            let local = !local;
                            let wiring_event = WiringEvent::<R>::Message {
                                stream_id,
                                local,
                                message,
                            };
                            if let Err(_) = self.wire(wiring_event).await {
                                rx.close();
                            };
                        };
                    }
                }
            }
        }
    }
}

impl<W: Unwire, H: HandleEvent<Res>, R: Wiring, Res: Unwiring> Unwired<W, H, R, Res>
where
    R: Debug + Send + 'static,
    Res: Debug + Send + 'static,
    W: Send + Sync + 'static,
{
    async fn run(mut self, rx: UnboundedReceiver<Option<UnwiredEvent>>) {
        let helper = Self::helper_task(self.wired_handle.clone(), rx).boxed();
        let j = tokio::spawn(helper);

        while let Ok(w) = self.unwire::<WiringEvent<Res>>().await {
            match w {
                WiringEvent::Event(event) => {
                    self.handle_event.handle_event(event).await.ok();
                }
                WiringEvent::Message {
                    stream_id,
                    local,
                    message,
                } => {
                    self.handle
                        .send(Some(UnwiredEvent::Message {
                            stream_id,
                            local,
                            message,
                        }))
                        .ok();
                }
                WiringEvent::Drop { stream_id, local } => {
                    self.handle
                        .send(Some(UnwiredEvent::Drop {
                            stream_id,
                            local,
                            broadcast_to_wire: true,
                        }))
                        .ok();
                }
            }
        }

        self.handle.send(None).ok();
        drop(self);
        j.await.ok();
    }
    async fn helper_task(
        w_tx: std::sync::Arc<UnboundedSender<Option<WiredEvent<R>>>>,
        mut rx: UnboundedReceiver<Option<UnwiredEvent>>,
    ) {
        let mut streams: HashMap<(u64, bool), UnboundedSender<Vec<u8>>> = HashMap::new();

        while let Some(event) = rx.recv().await {
            match event {
                Some(event) => {
                    match event {
                        UnwiredEvent::Message {
                            stream_id,
                            local,
                            message,
                        } => {
                            let key = (stream_id, local);
                            if let Some(tx) = streams.get(&key) {
                                tx.send(message).ok();
                            }
                        }
                        UnwiredEvent::RegisterStream { stream_id, local, tx } => {
                            let key = (stream_id, local);
                            streams.insert(key, tx);
                        }
                        UnwiredEvent::Drop {
                            stream_id,
                            local,
                            broadcast_to_wire,
                        } => {
                            let key = (stream_id, local);
                            if let Some(_) = streams.remove(&key) {
                                // check if needed to be broadcasted to wire, in-case this is received from remote
                                if broadcast_to_wire {
                                    let wired_event = WiredEvent::Drop {
                                        stream_id,
                                        local,
                                        // no need to broadcast it to remote. as it's received from remote.
                                        broadcast_to_remote: false,
                                    };
                                    w_tx.send(wired_event.into()).ok();
                                }
                            }
                        }
                    }
                }
                None => {
                    rx.close();
                }
            }
        }
    }
}
