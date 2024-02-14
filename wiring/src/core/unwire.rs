use super::{wire::Wiring, ConnectConfig, SplitStream, WireId};
use futures::{FutureExt, StreamExt};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    num::NonZeroUsize,
    str::FromStr,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::tcp::OwnedReadHalf,
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
}

impl Unwire for tokio::net::TcpStream {
    type Stream = Self;
}

impl Unwire for OwnedReadHalf {
    type Stream = tokio::net::TcpStream;
}

impl<T: Send + Sync + AsyncRead + Unpin, C> Unwire for WireStream<T, C>
where
    C: ConnectConfig,
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
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send;
}

impl<T: Unwiring + Wiring + 'static> Unwiring for tokio::sync::oneshot::Sender<T> {
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
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            match u8::unwiring(wire).await? {
                1 => Ok(()),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unexpected u8 data for ()",
                )),
            }
        }
    }
}

impl Unwiring for u8 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_u8()
    }
}

impl Unwiring for i8 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i8()
    }
}

impl Unwiring for u16 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_u16()
    }
}

impl Unwiring for i16 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i16()
    }
}

impl Unwiring for u32 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_u32()
    }
}

impl Unwiring for i32 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i32()
    }
}

impl Unwiring for u64 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_u64()
    }
}

impl Unwiring for i64 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i64()
    }
}

impl Unwiring for u128 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_u128()
    }
}

impl Unwiring for i128 {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        wire.read_i128()
    }
}

impl Unwiring for String {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let mut dst = String::new();
            let len: u64 = wire.unwiring().await?;
            let mut reader = wire.take(len);
            reader.read_to_string(&mut dst).await?;
            Ok(dst)
        }
    }
}

impl Unwiring for Url {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let url = String::unwiring(wire).await?;
            let url = Url::from_str(&url).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Unable to unwire Url from String")
            })?;
            Ok(url)
        }
    }
}

impl<T: Unwiring> Unwiring for Vec<T> {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            let mut len = u64::unwiring(wire).await?;
            let capacity = usize::try_from(len).map_err(|e| std::io::Error::new(std::io::ErrorKind::OutOfMemory, e))?;
            let mut vec: Vec<T> = Vec::with_capacity(capacity);
            while len > 0 {
                len -= 1;
                let t = T::unwiring(wire).await?;
                vec.push(t);
            }
            Ok(vec)
        }
    }
}

impl<T: Unwiring + Eq + PartialEq + std::hash::Hash> Unwiring for HashSet<T> {
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
}

impl<K, V> Unwiring for HashMap<K, V>
where
    K: Unwiring + Eq + PartialEq + std::hash::Hash,
    V: Unwiring,
{
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
}

impl<K, V> Unwiring for BTreeMap<K, V>
where
    K: Unwiring + Ord + std::hash::Hash,
    V: Unwiring,
{
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
}

impl<T: Unwiring + Ord + std::hash::Hash> Unwiring for std::collections::BTreeSet<T> {
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
}

impl<T: Unwiring> Unwiring for Option<T> {
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
}

impl<T: Unwiring, TT: Unwiring> Unwiring for (T, TT) {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async { Ok((T::unwiring(wire).await?, TT::unwiring(wire).await?)) }
    }
}
