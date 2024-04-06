use tokio::{
    net::TcpListener,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};
use wiring::prelude::{ConnectInfo, TcpStreamConfig, WireChannel, WireConfig};

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
        // decode the reply handle
        let WireChannel::<UnboundedSender<i32>, UnboundedReceiver<String>> { sender, mut receiver } =
            server_wire.into().await.expect("Expect to create channel");
        let request = receiver.recv().await.expect("Expected client to send message");
        println!("Server received: <- {}", request);
        sender.send(7).ok();
        break;
    }
    j.await.expect("Client task to complete");
}

async fn client(connect_info: ConnectInfo) {
    let client_wire = WireConfig::new(TcpStreamConfig)
        .connect(&connect_info)
        .await
        .expect("Wireconfig to connect");

    let WireChannel::<UnboundedSender<String>, UnboundedReceiver<i32>> { sender, mut receiver } =
        client_wire.into().await.expect("Expect to create channel");
    sender.send("Hello server, echo 7i32".to_string()).ok();
    let response = receiver
        .recv()
        .await
        .expect("Client expected to receive response from remote");

    assert!(response == 7i32);
    println!("Client received: <- {}", response);
}
