use super::{wire::Wiring, ConnectConfig, IoSplit, SplitStream, WireId};
use futures::{FutureExt, StreamExt};
use std::{
    any::type_name,
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Debug,
    io::Read,
    mem::{size_of, MaybeUninit},
    num::NonZeroUsize,
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf},
    net::{tcp::OwnedReadHalf, TcpStream},
};
use url::Url;

use super::wire::{Wire, WireStream};

pub trait Unwire: AsyncRead + Unpin + Send + Sync + Sized {
    type Stream: Wire + Unwire + SplitStream;
    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "TcpStream from stream is not supported",
            ))
        }
    }

    /// Global bounded channel buffer size, used to unwire bounded channels.
    fn bounded_buffer(&self) -> NonZeroUsize {
        // It's safe
        unsafe { NonZeroUsize::new_unchecked(1usize) }
    }
    fn unwire<T: Unwiring>(&mut self) -> impl std::future::Future<Output = Result<T, std::io::Error>> + Send {
        async move { Ok(T::unwiring(self).await?) }
    }
    fn unwiring<T: Unwiring>(&mut self) -> impl std::future::Future<Output = Result<T, std::io::Error>> + Send {
        async move { Ok(T::unwiring(self).await?) }
    }
    #[inline]
    fn sync_unwire_u8(&mut self) -> Result<u8, std::io::Error>
    where
        Self: std::io::Read,
    {
        let mut b = [0u8; 1];
        self.read_exact(&mut b)?;
        Ok(u8::from_be_bytes(b))
    }
    #[inline]
    fn sync_unwire_u16(&mut self) -> Result<u16, std::io::Error>
    where
        Self: std::io::Read,
    {
        let mut b = [0u8; 2];
        self.read_exact(&mut b)?;
        Ok(u16::from_be_bytes(b))
    }
    #[inline]
    fn sync_unwire_f32(&mut self) -> Result<f32, std::io::Error>
    where
        Self: std::io::Read,
    {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(f32::from_be_bytes(b))
    }
    #[inline(always)]
    fn sync_unwire_u32(&mut self) -> Result<u32, std::io::Error>
    where
        Self: std::io::Read,
    {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(u32::from_be_bytes(b))
    }
    #[inline(always)]
    fn sync_unwire_exact<const N: usize>(&mut self) -> Result<[u8; N], std::io::Error>
    where
        Self: std::io::Read,
    {
        let mut b = [0u8; N];
        self.read_exact(&mut b)?;
        Ok(b)
    }
    #[inline]
    #[allow(unused)]
    fn advance_position(&mut self, amt: u64) -> Result<(), std::io::Error> {
        Ok(())
    }
    #[inline]
    fn sync_unwire_u64(&mut self) -> Result<u64, std::io::Error>
    where
        Self: std::io::Read,
    {
        let mut b = [0u8; 8];
        self.read_exact(&mut b)?;
        Ok(u64::from_be_bytes(b))
    }
    #[inline]
    fn sync_unwire_string(&mut self) -> Result<String, std::io::Error>
    where
        Self: std::io::Read,
    {
        let len = self.sync_unwire_u64()?;
        let mut b = vec![0u8; len as usize];
        self.read_exact(b.as_mut_slice())?;
        String::from_utf8(b).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
    fn sync_unwire<T: Unwiring>(&mut self) -> Result<T, std::io::Error>
    where
        Self: std::io::Read,
    {
        T::sync_unwiring(self)
    }

    fn sync_unwiring<T: Unwiring>(&mut self) -> Result<T, std::io::Error>
    where
        Self: std::io::Read,
    {
        T::sync_unwiring(self)
    }
}

impl Unwire for tokio::net::TcpStream {
    type Stream = Self;
}

impl<T: AsyncRead + tokio::io::AsyncWrite + Sync + Send + Unpin + Debug + 'static> Unwire for tokio::io::ReadHalf<T> {
    type Stream = IoSplit<T>;
}

pub struct BufUnWire<'a> {
    reader: UnsafeReader<'a>,
}
impl<'a> BufUnWire<'a> {
    pub fn new(buffer: &'a [u8]) -> Self {
        let reader = unsafe { UnsafeReader::new(buffer) };
        Self { reader }
    }
    /// Unwire value from the buffer
    pub fn unwire<T: Unwiring>(&mut self) -> Result<T, std::io::Error> {
        self.reader.sync_unwire()
    }
}
pub struct UnsafeReader<'a> {
    slice: &'a [u8],
    source_buffer_len: usize,
    position: usize,
}

impl<'a> UnsafeReader<'a> {
    unsafe fn new(source_buffer: &'a [u8]) -> Self {
        let reader = UnsafeReader {
            slice: source_buffer,
            source_buffer_len: source_buffer.len(),
            position: 0,
        };
        reader
    }

    fn as_ptr(&self) -> *const u8 {
        self.slice.as_ptr()
    }
    #[inline(always)]
    pub unsafe fn unwire_u64(&mut self) -> u64 {
        let value_ptr = self.as_ptr().add(self.position) as *const u64;
        let value: u64 = u64::from_be(value_ptr.read_unaligned());
        self.position += 8;
        value
    }

    #[inline(always)]
    pub unsafe fn unwire_f32(&mut self) -> f32 {
        let value_ptr = self.as_ptr().add(self.position) as *const u32;
        self.position += std::mem::size_of::<f32>();
        f32::from_bits(u32::from_be(value_ptr.read_unaligned()))
    }
    #[inline(always)]
    unsafe fn unwire_u32(&mut self) -> u32 {
        let value_ptr = self.as_ptr().add(self.position) as *const u32;
        let value = u32::from_be(value_ptr.read_unaligned());
        self.position += std::mem::size_of::<u32>();
        value
    }

    #[inline(always)]
    unsafe fn unwire_u8(&mut self) -> u8 {
        let value_ptr = self.as_ptr().add(self.position).read_unaligned();
        self.position += std::mem::size_of::<u8>();
        value_ptr
    }
    #[inline(always)]
    unsafe fn unwire_u16(&mut self) -> u16 {
        let value = u16::from_be_bytes([
            self.as_ptr().add(self.position).read_unaligned(),
            self.as_ptr().add(self.position + 1).read_unaligned(),
        ]);
        self.position += std::mem::size_of::<u16>();
        value
    }
    #[inline(always)]
    unsafe fn unwire_slice<'b>(&'b mut self, len: usize) -> &'b [u8] {
        let slice = std::slice::from_raw_parts(self.as_ptr().add(self.position), len as usize);
        self.position += len;
        slice
    }
    #[inline(always)]
    unsafe fn unwire_exact<const N: usize>(&mut self) -> [u8; N] {
        let value_ptr = self.as_ptr().add(self.position) as *const [u8; N];
        self.position += N;
        value_ptr.read_unaligned()
    }
}

impl<'a> Unwire for UnsafeReader<'a> {
    type Stream = TcpStream;

    #[inline]
    fn sync_unwire<T: Unwiring>(&mut self) -> Result<T, std::io::Error>
    where
        Self: std::io::Read,
    {
        let pos = self.position;
        let mut temp_safe_wire = self.slice;
        let len = T::bytes_length(&mut temp_safe_wire, 1)?;
        // now when advancing the length first, then we recreate it? it's doable.
        let can_read = pos as u64 + len <= self.source_buffer_len as u64;
        // the bytes len already advanced the pos.
        if can_read {
            // reset
            self.position = pos;
            T::sync_unwiring(self)
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::OutOfMemory, "Out of bounds"))
        }
    }
    #[inline(always)]
    fn advance_position(&mut self, amt: u64) -> Result<(), std::io::Error> {
        self.position += amt as usize;
        Ok(())
    }
    #[inline(always)]
    fn sync_unwire_f32(&mut self) -> Result<f32, std::io::Error>
    where
        Self: std::io::Read,
    {
        unsafe { Ok(self.unwire_f32()) }
    }
    #[inline(always)]
    fn sync_unwire_u32(&mut self) -> Result<u32, std::io::Error>
    where
        Self: std::io::Read,
    {
        unsafe { Ok(self.unwire_u32()) }
    }
    #[inline(always)]
    fn sync_unwire_u64(&mut self) -> Result<u64, std::io::Error>
    where
        Self: std::io::Read,
    {
        unsafe { Ok(self.unwire_u64()) }
    }
    #[inline(always)]
    fn sync_unwire_u8(&mut self) -> Result<u8, std::io::Error>
    where
        Self: std::io::Read,
    {
        unsafe { Ok(self.unwire_u8()) }
    }
    #[inline(always)]
    fn sync_unwire_u16(&mut self) -> Result<u16, std::io::Error>
    where
        Self: std::io::Read,
    {
        unsafe { Ok(self.unwire_u16()) }
    }
    #[inline(always)]
    fn sync_unwire_exact<const N: usize>(&mut self) -> Result<[u8; N], std::io::Error>
    where
        Self: std::io::Read,
    {
        unsafe { Ok(self.unwire_exact()) }
    }
    #[inline(always)]
    fn sync_unwire_string(&mut self) -> Result<String, std::io::Error>
    where
        Self: std::io::Read,
    {
        unsafe {
            let len = self.unwire_u64() as usize;
            let slice = self.unwire_slice(len);
            let s = std::str::from_utf8(slice).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(s.to_string())
        }
    }
}

impl<'a> AsyncRead for UnsafeReader<'a> {
    fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "UnsafeReader doesn't support asyncread",
        )))
    }
}

impl<'a> Read for UnsafeReader<'a> {
    #[inline]
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "UnsafeReader doesn't support Read::read at the moment",
        ))
    }
    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        unsafe {
            let count = buf.len();
            self.as_ptr()
                .add(self.position)
                .copy_to_nonoverlapping(buf.as_mut_ptr(), count);
            self.position += count;
        }
        Ok(())
    }
}

impl<'a> Unwire for &'a [u8] {
    type Stream = TcpStream;
    #[inline]
    fn unwire<T: Unwiring>(&mut self) -> impl std::future::Future<Output = Result<T, std::io::Error>> + Send {
        async move { self.sync_unwire() }
    }
    fn advance_position(&mut self, amt: u64) -> Result<(), std::io::Error> {
        let amt = amt as usize;
        if amt as usize > self.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "failed to advance the position",
            ));
        }
        let (_a, b) = self.split_at(amt);
        *self = b;
        Ok(())
    }
    #[inline]
    fn sync_unwire<T: Unwiring>(&mut self) -> Result<T, std::io::Error> {
        T::sync_unwiring(self)
    }

    fn sync_unwire_f32(&mut self) -> Result<f32, std::io::Error>
    where
        Self: std::io::Read,
    {
        // just because we're doing this, it's very slow
        if 4 > self.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "failed to fill whole buffer",
            ));
        }
        let (a, b) = self.split_at(4);
        *self = b;
        Ok(f32::from_be_bytes(a.try_into().unwrap()))
    }
    #[inline(always)]
    fn sync_unwire_u64(&mut self) -> Result<u64, std::io::Error>
    where
        Self: std::io::Read,
    {
        if 8 > self.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "failed to fill whole buffer",
            ));
        }
        let (a, b) = self.split_at(8);
        *self = b;
        Ok(u64::from_be_bytes(a.try_into().unwrap()))
    }
}

impl<R> Unwire for std::io::Cursor<R>
where
    R: AsRef<[u8]> + Sync + Send + Unpin,
{
    type Stream = TcpStream;
    #[inline]
    fn unwire<T: Unwiring>(&mut self) -> impl std::future::Future<Output = Result<T, std::io::Error>> + Send {
        async move { self.sync_unwire() }
    }

    #[inline(always)]
    fn sync_unwire<T: Unwiring>(&mut self) -> Result<T, std::io::Error> {
        T::sync_unwiring(self)
    }
    #[inline(always)]
    fn sync_unwire_f32(&mut self) -> Result<f32, std::io::Error>
    where
        Self: std::io::Read,
    {
        let mut buf = [0u8; 4];
        std::io::Read::read_exact(self, &mut buf)?;
        Ok(f32::from_be_bytes(buf))
    }
}

impl Unwire for OwnedReadHalf {
    type Stream = tokio::net::TcpStream;
}

impl<T: Send + Sync + AsyncRead + Unpin, C> Unwire for WireStream<T, C>
where
    C: ConnectConfig,
    WireStream<C::Stream, C>: SplitStream,
{
    type Stream = WireStream<C::Stream, C>;

    fn stream(&mut self) -> impl std::future::Future<Output = Result<Self::Stream, std::io::Error>> + Send {
        async move {
            let _ = self.unwiring::<WireId>().await?;
            if let Some(incoming) = self.local.as_mut().map(|l| &mut l.incoming) {
                // first we unwire wire_id,first which enable us to use try_recv
                let w = incoming.try_recv().map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Unwire expected wire, but detect potential deadlock/attack",
                    )
                })?;
                Ok(w)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "Unwire doesn't support stream",
                ))
            }
        }
    }
}

pub trait Unwiring: Sized + Send + Sync {
    const FIXED_SIZE: usize = 0;
    const MIXED: bool = true;
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send;
    #[allow(unused)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let msg = format!("Type doesn't support sync unwiring: {}", type_name::<Self>());
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, msg))
    }
    #[inline]
    fn unwiring_vec<W: Unwire>(
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<Vec<Self>, std::io::Error>> + Send {
        async move {
            let len = u64::unwiring(wire).await? as usize;
            let mut data = Vec::with_capacity(len);
            for _ in 0..len {
                let t = Self::unwiring(wire).await?;
                data.push(t)
            }
            Ok(data)
        }
    }
    #[inline]
    fn unwiring_array<W: Unwire, const N: usize>(
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<[Self; N], std::io::Error>> + Send {
        async move {
            // todo
            let data = {
                let mut data: [MaybeUninit<Self>; N] = unsafe { MaybeUninit::uninit().assume_init() };
                for elem in &mut data[..] {
                    // todo improve this with safe code, as now it's not safe if unwiring exit
                    // make it similar to sync impl
                    let t = Self::unwiring(wire).await?;
                    elem.write(t);
                }
                unsafe { core::mem::transmute_copy::<_, [Self; N]>(&data) }
            };
            Ok(data)
        }
    }
    /// Return the length, including the prelength
    #[inline(always)]
    fn bytes_length<W: Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
    where
        W: Read,
    {
        let len = size_of::<Self>() as u64 * count;
        wire.advance_position(len)?;
        Ok(len)
    }

    #[inline(always)]
    fn sync_unwiring_array<W: Unwire, const N: usize>(wire: &mut W) -> Result<[Self; N], std::io::Error>
    where
        W: Read,
    {
        struct SafeInitArray<T, const N: usize> {
            array: Option<[MaybeUninit<T>; N]>,
            initialized_count: usize,
        }
        let data: [MaybeUninit<Self>; N] = unsafe { MaybeUninit::uninit().assume_init() };
        
        let mut v = SafeInitArray {
            array: data.into(),
            initialized_count: 0,
        };

        impl<T, const N: usize> Drop for SafeInitArray<T, N> {
            fn drop(&mut self) {
                unsafe {
                    if let Some(mut array) = self.array.take() {
                        for i in 0..self.initialized_count {
                            std::ptr::drop_in_place(array[i].as_mut_ptr());
                        }
                    }
                }
            }
        }

        unsafe {
            if let Some(d) = v.array.as_mut() {
                for i in d {
                    *i = MaybeUninit::new(Self::sync_unwiring(wire)?);
                    v.initialized_count += 1;
                }
                let data = Option::take(&mut v.array).ok_or(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Unable to unwrap fixed array",
                ))?;
                return Ok(MaybeUninit::array_assume_init(data));
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Unable to take fixed array",
                ));
            }
        }
    }
    #[inline(always)]
    fn sync_unwiring_vec<W: Unwire>(wire: &mut W) -> Result<Vec<Self>, std::io::Error>
    where
        W: Read,
    {
        let len = u64::sync_unwiring(wire)? as usize;
        struct SafeInitVec<T> {
            vec: Vec<MaybeUninit<T>>,
            initialized_count: usize,
        }

        let mut data: Vec<MaybeUninit<Self>> = Vec::new();
        data.reserve_exact(len);

        let mut v = SafeInitVec {
            vec: data,
            initialized_count: 0,
        };
        impl<T> Drop for SafeInitVec<T> {
            fn drop(&mut self) {
                unsafe {
                    // Only proceed if there are elements in the vector
                    if self.vec.len() > 0 {
                        // Drop each initialized element in place

                        for i in 0..self.initialized_count {
                            std::ptr::drop_in_place(self.vec[i].as_mut_ptr());
                        }
                        // Prevent automatic drop logic by setting length to 0
                        self.vec.set_len(0);
                    }
                }
            }
        }

        unsafe {
            v.vec.set_len(len);
            let d = v.vec.iter_mut();
            for i in d {
                *i = MaybeUninit::new(Self::sync_unwiring(wire)?);
                v.initialized_count += 1;
            }
            let data = std::mem::take(&mut v.vec);
            let initialized_vector: Vec<Self> = std::mem::transmute(data);
            Ok(initialized_vector)
        }
    }
}

impl<T: Unwiring + Wiring + 'static> Unwiring for tokio::sync::oneshot::Sender<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let mut w = wire.stream().await?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            let task = async move {
                tokio::select! {
                    _ = w.read_u8() => {

                    },
                    item = rx => {
                        if let Ok(item) = item {
                            w.wire(item).await.ok();
                        }
                    }
                }
            };
            tokio::spawn(task.boxed());
            Ok(tx)
        }
    }
}

impl<T: Unwiring + Wiring + 'static> Unwiring for tokio::sync::oneshot::Receiver<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let mut new = wire.stream().await?;
            let (mut tx, rx) = tokio::sync::oneshot::channel();
            let task = async move {
                tokio::select! {
                    _ = tx.closed() => {
                        new.shutdown().await.ok();
                    },
                    item = new.unwire() => {
                        if let Ok(item) = item {
                            tx.send(item).ok();
                        }
                    }
                }
            };
            tokio::spawn(task.boxed());
            Ok(rx)
        }
    }
}

impl<T: Unwiring + Wiring + 'static> Unwiring for tokio::sync::mpsc::UnboundedSender<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let w = wire.stream().await?;
            let (mut r, mut w) = w.split()?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let task = async move {
                while let Some(item) = rx.recv().await {
                    if let Err(_) = w.wire(item).await {
                        rx.close();
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
            Ok(tx)
        }
    }
}

impl<T: Unwiring + Wiring + 'static> Unwiring for tokio::sync::mpsc::UnboundedReceiver<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let w = wire.stream().await?;
            let (mut r, mut w) = w.split()?;
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let closed_handle = tx.clone();
            let task = async move {
                while let Ok(item) = r.unwire().await {
                    if let Err(_) = tx.send(item) {
                        break;
                    }
                }
            };
            let j = tokio::spawn(task.boxed());
            let detect_shutdown = async move {
                closed_handle.closed().await;
                w.shutdown().await.ok();
                j.abort();
            };
            tokio::spawn(detect_shutdown.boxed());
            Ok(rx)
        }
    }
}

impl<T: Unwiring + Wiring + 'static> Unwiring for tokio::sync::mpsc::Sender<T> {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let w = wire.stream().await?;
            let (mut r, mut w) = w.split()?;
            let buffer: usize = wire.bounded_buffer().into();
            let (tx, mut rx) = tokio::sync::mpsc::channel(buffer);
            let task = async move {
                while let Some(item) = rx.recv().await {
                    if let Err(_) = w.wire(item).await {
                        rx.close();
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
            Ok(tx)
        }
    }
}

impl<T: Unwiring + Wiring + 'static> Unwiring for tokio::sync::mpsc::Receiver<T> {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let w = wire.stream().await?;
            let (mut r, mut w) = w.split()?;
            let buffer: usize = wire.bounded_buffer().into();
            let (tx, rx) = tokio::sync::mpsc::channel(buffer);
            // so when unwiring
            let closed_handle = tx.clone();
            let task = async move {
                while let Ok(item) = r.unwire().await {
                    if let Err(_) = tx.send(item).await {
                        break;
                    }
                }
            };
            let j = tokio::spawn(task.boxed());
            let detect_shutdown = async move {
                closed_handle.closed().await;
                w.shutdown().await.ok();
                j.abort();
            };
            tokio::spawn(detect_shutdown.boxed());
            Ok(rx)
        }
    }
}

impl<T: Unwiring + 'static> Unwiring for tokio::sync::watch::Receiver<T> {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let mut w = wire.stream().await?;
            // unwire initial value
            let init = w.unwire().await?;
            // first message on stream is the T?
            let (mut r, w) = w.split()?;
            let (tx, rx) = tokio::sync::watch::channel(init);
            let mut closed_handle = tx.subscribe();
            let task = async move {
                while let Ok(item) = r.unwire().await {
                    if let Err(_) = tx.send(item) {
                        break;
                    }
                }
            };
            let j = tokio::spawn(task.boxed());
            let detect_shutdown = async move {
                if let Err(_) = closed_handle.wait_for(|_| false).await {
                    j.abort();
                    drop(w);
                }
            };
            tokio::spawn(detect_shutdown.boxed());
            Ok(rx)
        }
    }
}

impl<T: Wiring + Unwiring + 'static + Clone> Unwiring for tokio::sync::watch::Sender<T> {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let mut w = wire.stream().await?;
            // unwire initial value
            let init = w.unwire().await?;

            // first message on stream is the T?
            let (tx, rx) = tokio::sync::watch::channel(init);

            let (mut r, mut w) = w.split()?;

            let mut rx = tokio_stream::wrappers::WatchStream::new(rx);

            let task = async move {
                while let Some(v) = rx.next().await {
                    if let Err(_) = w.wire(v).await {
                        break;
                    }
                }
            };
            let j = tokio::spawn(task.boxed());
            let detect_shutdown = async move {
                // useless read, as remote not supposed to push to us anything, however it enables us to detects if
                // closed. in order to drop sender channel.
                r.read_u8().await.ok();
                j.abort();
            };
            tokio::spawn(detect_shutdown.boxed());
            Ok(tx)
        }
    }
}

impl Unwiring for () {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            match u8::unwiring(wire).await? {
                0 => Ok(()),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unexpected u8 data for ()",
                )),
            }
        }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        match u8::sync_unwiring(wire)? {
            0 => Ok(()),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Unexpected u8 unwiring data for ()",
            )),
        }
    }
    #[inline]
    fn bytes_length<W: Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
    where
        W: Read,
    {
        wire.advance_position(1 * count)?;
        Ok(1 * count)
    }
}

impl Unwiring for bool {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move { Ok(wire.read_u8().await? != 0) }
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let r = wire.sync_unwire_u8()?;
        Ok(r != 0)
    }
}

impl Unwiring for u8 {
    const FIXED_SIZE: usize = 1;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_u8()
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        wire.sync_unwire_u8()
    }
}

impl Unwiring for i8 {
    const FIXED_SIZE: usize = 1;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i8()
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        Ok(wire.sync_unwire_u8()? as i8)
    }
}

impl Unwiring for u16 {
    const FIXED_SIZE: usize = 2;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_u16()
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        wire.sync_unwire_u16()
    }
}

impl Unwiring for i16 {
    const FIXED_SIZE: usize = 2;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i16()
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        Ok(wire.sync_unwire_u16()? as i16)
    }
}

impl Unwiring for u32 {
    const FIXED_SIZE: usize = 4;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_u32()
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        wire.sync_unwire_u32()
    }
}

impl Unwiring for i32 {
    const FIXED_SIZE: usize = 4;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i32()
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        Ok(wire.sync_unwire_u32()? as i32)
    }
}

impl Unwiring for f32 {
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_f32()
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        wire.sync_unwire_f32()
    }
}

impl Unwiring for u64 {
    const FIXED_SIZE: usize = 8;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_u64()
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        wire.sync_unwire_u64()
    }
}

impl Unwiring for i64 {
    const FIXED_SIZE: usize = 8;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i64()
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        Ok(wire.sync_unwire_u64()? as i64)
    }
}

impl Unwiring for f64 {
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_f64()
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let mut buf = [0u8; 8];
        wire.read_exact(&mut buf)?;
        Ok(Self::from_be_bytes(buf))
    }
}

impl Unwiring for u128 {
    const FIXED_SIZE: usize = 16;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            let w = wire.read_u128().await?;
            Ok(w)
        }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let mut buf = [0u8; 16];
        wire.read_exact(&mut buf)?;

        Ok(Self::from_be_bytes(buf))
    }
}

impl Unwiring for i128 {
    const FIXED_SIZE: usize = 16;
    const MIXED: bool = false;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i128()
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let mut buf = [0u8; 16];
        wire.read_exact(&mut buf)?;
        Ok(Self::from_be_bytes(buf))
    }
}

impl Unwiring for String {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let mut dst = String::new();
            let len: u64 = wire.unwiring().await?;
            let mut reader = wire.take(len);
            reader.read_to_string(&mut dst).await?;
            Ok(dst)
        }
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        wire.sync_unwire_string()
    }
    #[inline(always)]
    fn bytes_length<W: Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
    where
        W: Read,
    {
        // count if string is inside vector, however to decode string length, we must compute the length now
        let mut total_bytes_len = size_of::<u64>() as u64 * count;
        for _ in 0..count {
            let len = wire.sync_unwire_u64()?;
            total_bytes_len += len;
            // advancing the position to allow next field reading from its start
            wire.advance_position(len)?;
        }
        Ok(total_bytes_len)
    }
}

impl Unwiring for Url {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let url = String::unwiring(wire).await?;
            let url = Url::from_str(&url).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Unable to unwire Url from String")
            })?;
            Ok(url)
        }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let url = String::sync_unwiring(wire)?;
        Self::from_str(&url).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

impl<T: Unwiring + 'static, const LEN: usize> Unwiring for [T; LEN] {
    const FIXED_SIZE: usize = T::FIXED_SIZE * LEN;
    const MIXED: bool = T::MIXED;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        T::unwiring_array::<_, LEN>(wire)
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        T::sync_unwiring_array::<_, LEN>(wire)
    }
    #[inline(always)]
    fn bytes_length<W: Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
    where
        W: Read,
    {
        let total_bytes_len = T::bytes_length(wire, LEN as u64 * count)?;
        Ok(total_bytes_len)
    }
}

impl<T: Unwiring + 'static> Unwiring for std::sync::Arc<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let v = wire.unwiring::<T>().await?;
            let arced: Self = v.into();
            Ok(arced)
        }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let v = T::sync_unwiring(wire)?;
        let arced: Self = v.into();
        Ok(arced)
    }
}

impl<T: Unwiring + 'static> Unwiring for Box<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let vec = wire.unwiring::<T>().await?;
            let boxx: Self = vec.into();
            Ok(boxx)
        }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let v = T::sync_unwiring(wire)?;
        let boxed: Self = v.into();
        Ok(boxed)
    }
}

impl<T: Unwiring + 'static> Unwiring for Box<[T]> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let vec = wire.unwiring::<Vec<T>>().await?;
            let boxx: Self = vec.into();
            Ok(boxx)
        }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let vec = T::sync_unwiring_vec(wire)?;
        let boxed = vec.into();
        Ok(boxed)
    }
    fn bytes_length<W: Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
    where
        W: Read,
    {
        T::bytes_length(wire, count)
    }
}

impl<T: Unwiring> Unwiring for Vec<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let vec = T::unwiring_vec(wire).await?;
            Ok(vec)
        }
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let vec = T::sync_unwiring_vec(wire)?;
        Ok(vec)
    }
    #[inline(always)]
    fn bytes_length<W: Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
    where
        W: Read,
    {
        // if all fixed, such as triangle or whatever, we set it to be tru
        let mut total_bytes_len = size_of::<u64>() as u64 * count;

        for _ in 0..count {
            let this_count = wire.sync_unwire_u64()?;
            // check if type is FIXED
            if !T::MIXED {
                let len = T::FIXED_SIZE as u64 * this_count;
                total_bytes_len += len;
                wire.advance_position(len)?;
            } else {
                total_bytes_len += T::bytes_length(wire, this_count)?;
            }
        }
        Ok(total_bytes_len)
    }
}

impl<T: Unwiring + Eq + PartialEq + std::hash::Hash> Unwiring for HashSet<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let mut len = u64::unwiring(wire).await?;
            let capacity = usize::try_from(len).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
            let mut set: HashSet<T> = HashSet::with_capacity(capacity);
            while len > 0 {
                len -= 1;
                let t = T::unwiring(wire).await?;
                set.insert(t);
            }
            Ok(set)
        }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let mut len = u64::sync_unwiring(wire)?;
        let capacity = usize::try_from(len).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
        let mut set: HashSet<T> = HashSet::with_capacity(capacity);
        while len > 0 {
            len -= 1;
            let t = T::sync_unwiring(wire)?;
            set.insert(t);
        }
        Ok(set)
    }
}

impl<K, V> Unwiring for HashMap<K, V>
where
    K: Unwiring + Eq + PartialEq + std::hash::Hash,
    V: Unwiring,
{
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let mut len = u64::unwiring(wire).await?;
            let capacity = usize::try_from(len).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
            let mut map: HashMap<K, V> = HashMap::with_capacity(capacity);
            while len > 0 {
                len -= 1;
                let k = K::unwiring(wire).await?;
                let v = V::unwiring(wire).await?;
                map.insert(k, v);
            }
            Ok(map)
        }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let mut len = u64::sync_unwiring(wire)?;
        let capacity = usize::try_from(len).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
        let mut map: HashMap<K, V> = HashMap::with_capacity(capacity);
        while len > 0 {
            len -= 1;
            let k = K::sync_unwiring(wire)?;
            let v = V::sync_unwiring(wire)?;
            map.insert(k, v);
        }
        Ok(map)
    }
}

impl<K, V> Unwiring for BTreeMap<K, V>
where
    K: Unwiring + Ord + std::hash::Hash,
    V: Unwiring,
{
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let mut len = u64::unwiring(wire).await?;
            let _capacity =
                usize::try_from(len).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
            let mut tree: BTreeMap<K, V> = BTreeMap::new();
            while len > 0 {
                len -= 1;
                let k = K::unwiring(wire).await?;
                let v = V::unwiring(wire).await?;
                tree.insert(k, v);
            }
            Ok(tree)
        }
    }
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let mut len = u64::sync_unwiring(wire)?;
        let _capacity = usize::try_from(len).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
        let mut tree: BTreeMap<K, V> = BTreeMap::new();
        while len > 0 {
            len -= 1;
            let k = K::sync_unwiring(wire)?;
            let v = V::sync_unwiring(wire)?;
            tree.insert(k, v);
        }
        Ok(tree)
    }
}

impl<T: Unwiring + Ord + std::hash::Hash> Unwiring for std::collections::BTreeSet<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let mut len = u64::unwiring(wire).await?;
            let _capacity =
                usize::try_from(len).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
            let mut set = Self::new();
            while len > 0 {
                len -= 1;
                let t = T::unwiring(wire).await?;
                set.insert(t);
            }
            Ok(set)
        }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        let mut len = u64::sync_unwiring(wire)?;
        let _capacity = usize::try_from(len).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
        let mut set = Self::new();
        while len > 0 {
            len -= 1;
            let t = T::sync_unwiring(wire)?;
            set.insert(t);
        }
        Ok(set)
    }
}

impl<T: Unwiring> Unwiring for Option<T> {
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            match u8::unwiring(wire).await? {
                0 => return Ok(None),
                1 => Ok(Some(T::unwiring(wire).await?)),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Unwiring {} unexpected variant", std::any::type_name::<Self>()),
                )),
            }
        }
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        match u8::sync_unwiring(wire)? {
            0 => return Ok(None),
            1 => Ok(Some(T::sync_unwiring(wire)?)),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unwiring {} unexpected variant", std::any::type_name::<Self>()),
            )),
        }
    }
    #[inline(always)]
    fn bytes_length<W: Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
    where
        W: Read,
    {
        // 1 byte for the variant
        let mut total_bytes_len = 1 * count;
        for _ in 0..count {
            let variant = wire.sync_unwire_u8()?;
            if variant == 1 {
                let len = T::bytes_length(wire, 1)?;
                total_bytes_len += len;
            }
        }
        Ok(total_bytes_len)
    }
}

impl<T: Unwiring, TT: Unwiring> Unwiring for (T, TT) {
    const FIXED_SIZE: usize = T::FIXED_SIZE + TT::FIXED_SIZE;
    const MIXED: bool = T::MIXED && TT::MIXED;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async { Ok((T::unwiring(wire).await?, TT::unwiring(wire).await?)) }
    }
    #[inline]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        Ok((T::sync_unwiring(wire)?, TT::sync_unwiring(wire)?))
    }
    #[inline(always)]
    fn bytes_length<W: Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
    where
        W: Read,
    {
        let mut total_bytes_len = 0;
        for _ in 0..count {
            total_bytes_len += T::bytes_length(wire, 1)?;
            total_bytes_len += TT::bytes_length(wire, 1)?;
        }
        Ok(total_bytes_len)
    }
}

impl<T: Unwiring, TT: Unwiring, T3: Unwiring> Unwiring for (T, TT, T3) {
    const FIXED_SIZE: usize = T::FIXED_SIZE + TT::FIXED_SIZE + T3::FIXED_SIZE;
    const MIXED: bool = T::MIXED && TT::MIXED && T3::MIXED;
    #[inline]
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            Ok((
                T::unwiring(wire).await?,
                TT::unwiring(wire).await?,
                T3::unwiring(wire).await?,
            ))
        }
    }
    #[inline(always)]
    fn sync_unwiring<W: Unwire>(wire: &mut W) -> Result<Self, std::io::Error>
    where
        W: Read,
    {
        Ok((
            T::sync_unwiring(wire)?,
            TT::sync_unwiring(wire)?,
            T3::sync_unwiring(wire)?,
        ))
    }
    #[inline(always)]
    fn bytes_length<W: Unwire>(wire: &mut W, count: u64) -> std::io::Result<u64>
    where
        W: Read,
    {
        let mut total_bytes_len = 0;
        for _ in 0..count {
            total_bytes_len += T::bytes_length(wire, 1)?;
            total_bytes_len += TT::bytes_length(wire, 1)?;
            total_bytes_len += T3::bytes_length(wire, 1)?;
        }
        Ok(total_bytes_len)
    }
}
