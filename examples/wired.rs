// NOTE: this example is kinda similar to nested example, except here we use the same wire for multiple requests
// while nested spawn stream for channel. this make wired perfect for connection and resource management.

use tokio::{
    net::TcpListener,
    sync::{mpsc::unbounded_channel, oneshot},
};
use wiring::prelude::{ConnectInfo, TcpStreamConfig, WireConfig};

#[tokio::main]
async fn main() {
    use wiring::prelude::*;
    // This the listener loop which you will get your tcpstream or websocket, etc.
    let connect_info = ConnectInfo::TcpStream("127.0.0.1:9999".to_string());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let h = WireListener::new(tx, TcpStreamConfig, connect_info.clone()).run();
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
        // create the channel to receive events from client, with any type the client is expecting to use.
        let (tx, mut rx) = unbounded_channel::<ClientRequest>();

        // wired enables us to receives channels from client over one wire.
        let _wired: WiredHandle<()> = server_wire.wired(tx).expect("To convert serverwire to wired");

        // await on loop to accept requests from client
        while let Some(client_request) = rx.recv().await {
            match client_request {
                ClientRequest::EchoMessage { reply, request: _ } => {
                    reply.send("2024".to_string()).ok();
                }
                ClientRequest::EchoNumber { reply, request } => {
                    reply.send(request).ok();
                }
            }
        }
        break;
    }
    j.await.expect("Client task to complete");
}

async fn client(connect_info: ConnectInfo) {
    let client_wire = WireConfig::new(TcpStreamConfig)
        .connect(&connect_info)
        .await
        .expect("Wireconfig to connect");

    // for bi communicate the client can also create a channel to handle events from server, but in our example
    // client only sends requests therefore client
    // the use noop as handle event
    let wired = client_wire.wired(()).expect("To convert clientwire to wired");

    // a request/response cycle example
    {
        let (reply, rx) = oneshot::channel();
        let client_request = ClientRequest::EchoMessage {
            reply,
            request: "Hello server, reply to me what year is it".to_string(),
        };
        println!("client sent request to server");
        wired.send(client_request).ok();

        let response = rx.await;
        println!("client received response: {:?}", response);
    }
}

#[derive(Debug)]
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
