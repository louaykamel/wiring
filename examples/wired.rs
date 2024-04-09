// NOTE: this example is kinda similar to nested example, except here we use the same wire for multiple requests
// while nested spawn stream for channel. this make wired perfect for connection and resource management.

use tokio::{
    net::TcpListener,
    sync::{mpsc::unbounded_channel, oneshot},
};
use wiring::prelude::{ConnectInfo, TcpStreamConfig, Unwiring, WireConfig, Wiring};

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

    // for bi communication the client can also create a channel to handle events from server, but in our example
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

#[derive(Debug, Wiring, Unwiring)]
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
