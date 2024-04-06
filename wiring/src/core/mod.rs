use futures::FutureExt;
use std::{collections::HashMap, fmt::Debug};

use tokio::{
    io::{AsyncRead, AsyncWrite, BufReader, BufWriter},
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
pub(crate) mod wired;

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

#[pin_project::pin_project]
#[derive(Debug)]
pub struct IoSplit<T>(#[pin] T);

impl<T: AsyncRead + AsyncWrite + Unpin + Debug + Send + 'static + Sync> SplitStream for IoSplit<T> {
    type Unwire = tokio::io::ReadHalf<T>;
    type Wire = tokio::io::WriteHalf<T>;
    fn split(self) -> Result<(Self::Unwire, Self::Wire), std::io::Error> {
        Ok(tokio::io::split(self.0))
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin + Debug + Send + 'static + Sync> Wire for IoSplit<T> {
    type Stream = IoSplit<T>;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Cannot to establish stream from IoSplit",
            ))
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin + Debug + Send + 'static + Sync> Unwire for IoSplit<T> {
    type Stream = IoSplit<T>;
}

impl<T> AsyncRead for IoSplit<T>
where
    T: AsyncRead,
{
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.project().0.poll_read(cx, buf)
    }
}

impl<T> AsyncWrite for IoSplit<T>
where
    T: AsyncWrite,
{
    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().0.poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().0.poll_shutdown(cx)
    }
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().0.poll_write(cx, buf)
    }
    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().0.poll_write_vectored(cx, bufs)
    }
}

impl<T, C> SplitStream for WireStream<T, C>
where
    // T: AsyncRead + AsyncWrite + Unpin + Send + Debug + 'static + Sync,
    T: SplitStream,
    C: ConnectConfig,
    C::Stream: SplitStream,
{
    type Unwire = WireStream<<T as SplitStream>::Unwire, C>;
    type Wire = WireStream<<T as SplitStream>::Wire, C>;
    fn split(self) -> Result<(Self::Unwire, Self::Wire), std::io::Error> {
        // to make this more generic, we're using io::split.
        // let (r, w) = tokio::io::split(self.stream);
        let (r, w) = self.stream.split()?;

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
    type RawStream: AsyncRead + AsyncWrite + Send + Sync + Unpin + Debug;
    type Stream: AsyncRead + AsyncWrite + Send + Sync + Unpin + Debug + SplitStream;
    fn connect_stream(
        &self,
        connect_info: &ConnectInfo,
    ) -> impl std::future::Future<Output = Result<Self::RawStream, std::io::Error>> + Send;
    /// Enhance the raw stream and return optimized stream
    fn enhance_stream(&self, raw_stream: Self::RawStream) -> Result<Self::Stream, std::io::Error>;
}

#[derive(Debug, Clone)]
pub struct TcpStreamConfig;

impl ConnectConfig for TcpStreamConfig {
    type RawStream = TcpStream;
    type Stream = TcpStream;
    async fn connect_stream(&self, connect_info: &ConnectInfo) -> Result<Self::RawStream, std::io::Error> {
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
    fn enhance_stream(&self, raw_stream: Self::RawStream) -> Result<Self::Stream, std::io::Error> {
        Ok(raw_stream)
    }
}

#[pin_project::pin_project]
#[derive(Debug)]
pub struct BufStream<T: SplitStream> {
    #[pin]
    reader: BufReader<T::Unwire>,
    #[pin]
    writer: BufWriter<T::Wire>,
}

#[allow(dead_code)]
impl<T: SplitStream> BufStream<T> {
    /// Create new BufStream
    pub fn new(stream: T) -> Result<Self, std::io::Error> {
        let (r, w) = stream.split()?;
        let reader = BufReader::new(r);
        let writer = BufWriter::new(w);
        Ok(Self { reader, writer })
    }
    /// Create BufStream with capacity
    pub fn with_capacity(capacity: usize, stream: T) -> Result<Self, std::io::Error> {
        let (r, w) = stream.split()?;
        let reader = BufReader::with_capacity(capacity, r);
        let writer = BufWriter::with_capacity(capacity, w);
        Ok(Self { reader, writer })
    }
}

impl<T: SplitStream> AsyncWrite for BufStream<T> {
    fn is_write_vectored(&self) -> bool {
        self.writer.is_write_vectored()
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().writer.poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.project().writer.poll_shutdown(cx)
    }
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().writer.poll_write(cx, buf)
    }
    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.project().writer.poll_write_vectored(cx, bufs)
    }
}

impl<T: SplitStream> AsyncRead for BufStream<T> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.project().reader.poll_read(cx, buf)
    }
}

impl<T: SplitStream> SplitStream for BufStream<T>
where
    T::Wire: Debug + Wire + AsyncWrite,
    T::Unwire: Debug + Unwire + AsyncRead,
{
    type Unwire = BufReader<T::Unwire>;
    type Wire = BufWriter<T::Wire>;
    fn split(self) -> Result<(Self::Unwire, Self::Wire), std::io::Error> {
        Ok((self.reader, self.writer))
    }
}

impl<T: SplitStream> Wire for BufStream<T>
where
    T::Wire: Debug,
    T::Unwire: Debug,
{
    type Stream = BufStream<T>;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Cannot establish stream from BufStream",
            ))
        }
    }
}

impl<T> Unwire for BufStream<T>
where
    T: SplitStream,
    T::Wire: Debug,
    T::Unwire: Debug,
{
    type Stream = BufStream<T>;
}

impl<T: AsyncWrite + Unpin + Send + Sync + 'static> Wire for BufWriter<T> {
    type Stream = TcpStream;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Cannot establish stream from BufWriter",
            ))
        }
    }
}

impl<R: AsyncRead + Unpin + Send + Sync + 'static> Unwire for BufReader<R> {
    // dead end so no need to set correct stream here,
    type Stream = TcpStream;
}

#[derive(Debug, Clone)]
pub struct BufStreamConfig<C: ConnectConfig> {
    capacity: usize,
    inner_config: C,
}

#[allow(dead_code)]
impl<C: ConnectConfig> BufStreamConfig<C> {
    /// Create new BufStreamConfig
    pub fn new(config: C) -> Self {
        Self {
            capacity: 8 * 1024,
            inner_config: config,
        }
    }
    /// Create BufStream with capacity
    pub fn with_capacity(capacity: usize, config: C) -> Self {
        Self {
            capacity,
            inner_config: config,
        }
    }
}

impl<C> ConnectConfig for BufStreamConfig<C>
where
    C: ConnectConfig,
    <<C as ConnectConfig>::Stream as SplitStream>::Unwire: std::fmt::Debug,
    <<C as ConnectConfig>::Stream as SplitStream>::Wire: std::fmt::Debug,
{
    type RawStream = C::RawStream;
    type Stream = BufStream<C::Stream>;
    async fn connect_stream(&self, connect_info: &ConnectInfo) -> Result<Self::RawStream, std::io::Error> {
        self.inner_config.connect_stream(connect_info).await
    }
    fn enhance_stream(&self, raw_stream: Self::RawStream) -> Result<Self::Stream, std::io::Error> {
        let stream = self.inner_config.enhance_stream(raw_stream)?;
        BufStream::with_capacity(self.capacity, stream)
    }
}
