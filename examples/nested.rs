use tokio::{
    net::TcpListener,
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
};
use wiring::prelude::{BufStreamConfig, ConnectInfo, TcpStreamConfig, WireChannel, WireConfig};

#[tokio::main]
async fn main() {
    use wiring::prelude::*;
    // This the listener loop which you will get your tcpstream or websocket, etc.
    let connect_info = ConnectInfo::TcpStream("127.0.0.1:9999".to_string());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let config = BufStreamConfig::new(TcpStreamConfig);
    let h = WireListener::new(tx, config, connect_info.clone()).run();
    let tcp_listener = TcpListener::bind("127.0.0.1:9999").await.expect("Tcplistener to bind");
    let listener = async move {
        while let Ok((stream, _)) = tcp_listener.accept().await {
            // If set to true, then the wire is returned to here, if it was not an initial wire.
            h.wire::<false>(stream).await.ok();
        }
    };
    tokio::spawn(listener);
    let j = tokio::spawn(client(connect_info));
    while let Some(server_wire) = rx.recv().await {
        // decode the reply handle
        let WireChannel::<UnboundedSender<String>, UnboundedReceiver<ClientRequest>> { sender, mut receiver } =
            server_wire.into().await.expect("Expect to create channel");

        // The sender can be used to push events to client (UI, etc), while receiver
        sender.send("This is push event(s) from server".to_string()).ok();

        while let Some(req) = receiver.recv().await {
            match req {
                ClientRequest::EchoMessage { reply, request } => {
                    println!("Server received echomessage request with reply handle");
                    reply.send(request).ok();
                }
                ClientRequest::EchoNumber { reply, request } => {
                    reply.send(request).ok();
                }
            }
            // the receiver will exit the loop once the client drop the sender half
        }
        break;
    }
    j.await.expect("Client task to complete");
}

async fn client(connect_info: ConnectInfo) {
    let config = BufStreamConfig::new(TcpStreamConfig);
    let client_wire = WireConfig::new(config)
        .connect(&connect_info)
        .await
        .expect("Wireconfig to connect");

    let WireChannel::<UnboundedSender<ClientRequest>, UnboundedReceiver<String>> { sender, mut receiver } =
        client_wire.into().await.expect("Expect to create channel");

    // send first request;
    let (reply, rx) = oneshot::channel();
    sender
        .send(ClientRequest::EchoMessage {
            reply,
            request: "This is sync request over oneshot channel".to_string(),
        })
        .ok();
    let replied = rx.await.expect("Expected server to reply");
    println!("Server replied to echomessage: {}", replied);
    // the receiver is used to receive push events from server
    let response = receiver
        .recv()
        .await
        .expect("Client expected to receive push events from remote");
    println!("Client received push event: {}", response);
}

pub enum ClientRequest {
    EchoMessage {
        reply: oneshot::Sender<String>,
        request: String,
    },
    EchoNumber {
        reply: oneshot::Sender<i32>,
        request: i32,
    },
}

// This is temporary, till we impl a macro to derive.
impl wiring::prelude::Wiring for ClientRequest {
    fn wiring<W: wiring::prelude::Wire>(
        self,
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<(), std::io::Error>> + Send {
        async move {
            match self {
                Self::EchoMessage { reply, request } => {
                    0u8.wiring(wire).await?;
                    reply.wiring(wire).await?;
                    request.wiring(wire).await?;
                }
                Self::EchoNumber { reply, request } => {
                    1u8.wiring(wire).await?;
                    reply.wiring(wire).await?;
                    request.wiring(wire).await?;
                }
            }
            Ok(())
        }
    }
}

// This is temporary, till we impl a macro to derive.
impl wiring::prelude::Unwiring for ClientRequest {
    fn unwiring<W: wiring::prelude::Unwire>(
        wire: &mut W,
    ) -> impl std::future::Future<Output = Result<Self, std::io::Error>> + Send {
        async move {
            match u8::unwiring(wire).await? {
                0 => Ok(Self::EchoMessage {
                    reply: wire.unwiring().await?,
                    request: wire.unwiring().await?,
                }),
                1 => Ok(Self::EchoNumber {
                    reply: wire.unwiring().await?,
                    request: wire.unwiring().await?,
                }),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unexpected clientrequest u8 variant",
                )),
            }
        }
    }
}
