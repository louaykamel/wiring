use tokio::sync::oneshot;

use self::wire::{HandleWire, WireConfig, WireInfo};

use super::*;
/// Listener server manages the state of incoming and outgoing wires
pub struct WireListener<T: HandleWire<C>, C: ConnectConfig> {
    /// Connect config like it holds tlsconfig and such if needed to connect to remote
    connect_config: C,
    /// Local connect config, to be passed to remote
    connect_info: ConnectInfo,
    wire_handler: T,
}

#[allow(dead_code)]
impl<T, C> WireListener<T, C>
where
    T: HandleWire<C> + Sync + Send + 'static,
    C: ConnectConfig,
{
    /// Create new Wirelistener with provided wire_handler and local connect info
    pub fn new(wire_handler: T, connect_config: C, connect_info: ConnectInfo) -> Self {
        Self {
            wire_handler,
            connect_config,
            connect_info,
        }
    }
    /// Run the wirelistener and return Wireconfig handle
    pub fn run(mut self) -> WireConfig<C, UnboundedSender<WireListenerEvent<C>>> {
        let (handle, mut inbox) = tokio::sync::mpsc::unbounded_channel::<WireListenerEvent<C>>();
        let r = handle.clone();
        let config = self.connect_config.clone();
        let x = async move {
            let mut wires = HashMap::new();
            // #todo have fn to set options on streams.
            while let Some(event) = inbox.recv().await {
                match event {
                    WireListenerEvent::OutgoingWire {
                        stream,
                        forward_info,
                        reply,
                    } => {
                        let mut wire = WireStream::new(stream);
                        // create wire id and  access key
                        let mut wire_id = rand::random();
                        let access_key: u128 = rand::random();
                        while wires.contains_key(&wire_id) {
                            wire_id = rand::random();
                        }
                        let wire_info = WireInfo::new(wire_id, access_key, self.connect_info.clone());

                        if let Err(e) = Some(&wire_info).wiring(&mut wire).await {
                            reply.send(Err(e)).ok();
                            continue;
                        } else {
                            if let Err(e) = wire.wire(forward_info).await {
                                reply.send(Err(e)).ok();
                                continue;
                            };
                        }
                        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                        let local = Local::new(wire_info, rx);
                        let wire = wire.with_local(local);
                        // NOTE peer info are set at stream fn level.
                        // store tx with access to push to it.
                        wires.insert(wire_id, (access_key, tx.clone()));
                        // handle the new wire. by pushing it to the listener?
                        let handle = handle.clone();
                        let signal_wire_drop = async move {
                            tx.closed().await;
                            handle.send(WireListenerEvent::DropWire(wire_id)).ok();
                        };
                        tokio::spawn(signal_wire_drop.boxed());
                        reply.send(Ok(wire)).ok();
                    }
                    WireListenerEvent::Incomingwire {
                        stream,
                        remote_info,
                        forward_info,
                        reply,
                    } => {
                        let mut wire = WireStream::new(stream);
                        let mut wire_id = rand::random();
                        let access_key = rand::random();
                        while wires.contains_key(&wire_id) {
                            wire_id = rand::random();
                        }
                        let wire_info = WireInfo::new(wire_id, access_key, self.connect_info.clone());

                        wire.wire(&wire_info).await.ok();
                        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                        let local = Local::new(wire_info, rx);

                        wire = wire.with_local(local);

                        if let Some(remote_info) = remote_info {
                            // we need localhandle to establish further bi-connections with remote,
                            wire = wire.with_peer(Peer::new(
                                remote_info,
                                Some(handle.clone()),
                                self.connect_config.clone(),
                            ));
                        }
                        if let Some((wire_id, access_key)) = forward_info {
                            if let Some((key, sender)) = wires.get(&wire_id) {
                                if *key == access_key {
                                    let r = sender.send(wire);
                                    let r = r
                                        .map(|_| None)
                                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e));
                                    if let Some(reply) = reply {
                                        reply.send(r).ok();
                                    }
                                } else {
                                    if let Some(reply) = reply {
                                        reply
                                            .send(Err(std::io::Error::new(
                                                std::io::ErrorKind::PermissionDenied,
                                                "Invalid access key",
                                            )))
                                            .ok();
                                    }
                                }
                            } else {
                                if let Some(reply) = reply {
                                    reply
                                        .send(Err(std::io::Error::new(
                                            std::io::ErrorKind::NotFound,
                                            "Wire target not found",
                                        )))
                                        .ok();
                                }
                            }
                        } else {
                            // store tx with access to push to it.
                            wires.insert(wire_id, (access_key, tx.clone()));
                            // todo maybe instead of handling wire with trait, we reply.
                            if let Some(reply) = reply {
                                reply.send(Ok(Some(wire))).ok();
                            } else {
                                if let Err(_) = self.wire_handler.handle_wire(wire) {
                                    inbox.close();
                                };
                            }
                        }
                        // handle the new wire. by pushing it to the listener?
                        let handle = handle.clone();
                        let signal_wire_drop = async move {
                            tx.closed().await;
                            handle.send(WireListenerEvent::DropWire(wire_id)).ok();
                        };
                        tokio::spawn(signal_wire_drop.boxed());
                    }
                    WireListenerEvent::DropWire(wire_id) => {
                        wires.remove(&wire_id);
                    }
                    WireListenerEvent::Shutdown => {
                        inbox.close();
                    }
                }
            }
        };
        tokio::spawn(x.boxed());
        WireConfig::with_handle(config, r)
    }
}

#[allow(dead_code)]
pub enum WireListenerEvent<C: ConnectConfig> {
    /// Incoming wire from remote peers
    Incomingwire {
        stream: C::Stream,
        /// The remote wire info. is pushed if they enable bi conn
        remote_info: Option<WireInfo>,
        /// Foward this wire to nested wire.
        forward_info: Option<(WireId, u128)>,
        /// Reply handle to skip handle wire
        reply: Option<oneshot::Sender<Result<Option<WireStream<C::Stream, C>>, std::io::Error>>>,
    },
    /// Outgoing wires to remote peers
    OutgoingWire {
        stream: C::Stream,
        forward_info: Option<(WireId, u128)>,
        reply: oneshot::Sender<Result<WireStream<C::Stream, C>, std::io::Error>>,
    },
    /// Dropped wire
    DropWire(WireId),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum ConnectInfo {
    /// Connect using string, used by tcpstream.
    TcpStream(String),
    /// Url used by websocket server through http/s or through tls
    Url(url::Url),
}

#[derive(Debug)]
pub struct Peer<C: ConnectConfig> {
    pub(crate) wire_info: WireInfo,
    pub(crate) local_handle: Option<tokio::sync::mpsc::UnboundedSender<WireListenerEvent<C>>>,
    pub(crate) connect_config: C,
}

impl<C: ConnectConfig> Peer<C> {
    pub fn new(
        wire_info: WireInfo,
        local_handle: Option<tokio::sync::mpsc::UnboundedSender<WireListenerEvent<C>>>,
        connect_config: C,
    ) -> Self {
        Self {
            wire_info,
            local_handle,
            connect_config,
        }
    }
}

#[derive(Debug)]
pub struct Local<C: ConnectConfig> {
    /// The local wire info
    pub(crate) wire_info: WireInfo,
    /// Incoming wires from remote, as we enabled them to push wires
    pub(crate) incoming: tokio::sync::mpsc::UnboundedReceiver<WireStream<C::Stream, C>>,
}

impl<C: ConnectConfig> Local<C> {
    pub fn new(wire_info: WireInfo, incoming: tokio::sync::mpsc::UnboundedReceiver<WireStream<C::Stream, C>>) -> Self {
        Self { wire_info, incoming }
    }
}

impl Wiring for ConnectInfo {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            match self {
                Self::TcpStream(string) => {
                    string.wiring(wire).await?;
                }
                Self::Url(url) => {
                    url.wiring(wire).await?;
                }
            }
            Ok(())
        }
    }
}

impl<'a> Wiring for &'a ConnectInfo {
    fn wiring<W: Wire>(self, wire: &mut W) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            match self {
                ConnectInfo::TcpStream(string) => {
                    0u8.wiring(wire).await?;
                    string.wiring(wire).await?;
                }
                ConnectInfo::Url(url) => {
                    1u8.wiring(wire).await?;
                    url.wiring(wire).await?;
                }
            }
            Ok(())
        }
    }
}

impl super::unwire::Unwiring for ConnectInfo {
    fn unwiring<W: Unwire>(wire: &mut W) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async {
            Ok(match u8::unwiring(wire).await? {
                0 => Self::TcpStream(wire.unwiring().await?),
                1 => Self::Url(wire.unwiring().await?),
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Unexpected Connect info variant",
                    ))
                }
            })
        }
    }
}
