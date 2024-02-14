use tokio::{net::TcpListener, sync::oneshot};
use wiring::prelude::{ConnectInfo, TcpStreamConfig, Wire, WireConfig};

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
    while let Some(mut server_wire) = rx.recv().await {
        // decode the reply handle
        let reply = server_wire
            .unwire::<oneshot::Sender<String>>()
            .await
            .expect("Server wire to unwire oneshot reply");

        reply.send("hello world".to_string()).ok();
        break;
    }
    j.await.expect("Client task to complete");
}

async fn client(connect_info: ConnectInfo) {
    let mut client_wire = WireConfig::new(TcpStreamConfig)
        .connect(&connect_info)
        .await
        .expect("Wireconfig to connect");

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();

    client_wire.wire(tx).await.expect("Client wire to wire sender");
    assert!("hello world".to_string() == rx.await.expect("Client wire oneshot to recv msg"))
}
