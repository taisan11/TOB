//! Transparent TCP proxy for the NetherNet signaling port used by current BDS.
//!
//! This intentionally forwards bytes without terminating TLS or interpreting HTTP.
//! After signaling, clients use the UDP ICE candidates advertised by BDS directly;
//! configure/firewall BDS's `server-udp-ports` range accordingly.

use anyhow::{Context, Result};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};

/// Accepts NetherNet signaling TCP connections and transparently relays each to BDS.
pub async fn run_nethernet_proxy(bind: SocketAddr, upstream: SocketAddr) -> Result<()> {
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind proxy signaling socket {bind}"))?;
    log::info!(
        "NetherNet TCP proxy listening on {} -> {}",
        listener.local_addr()?,
        upstream
    );

    loop {
        let (client, peer) = listener
            .accept()
            .await
            .context("accept signaling connection")?;
        tokio::spawn(async move {
            if let Err(error) = relay(client, upstream).await {
                log::debug!("signaling connection from {peer} ended: {error:#}");
            }
        });
    }
}

async fn relay(mut client: TcpStream, upstream: SocketAddr) -> Result<()> {
    let mut server = TcpStream::connect(upstream)
        .await
        .with_context(|| format!("connect BDS signaling port {upstream}"))?;
    let (to_server, to_client) = tokio::io::copy_bidirectional(&mut client, &mut server)
        .await
        .context("relay NetherNet signaling stream")?;
    log::trace!(
        "signaling stream closed: client->BDS {to_server} bytes, BDS->client {to_client} bytes"
    );
    Ok(())
}

/// Waits until BDS has started listening on its signaling port.
pub async fn wait_for_upstream(address: SocketAddr) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        loop {
            if TcpStream::connect(address).await.is_ok() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .with_context(|| format!("BDS did not start listening on {address} within 90 seconds"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tcp_proxy_relays_signaling_bytes_both_directions() {
        let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_address = backend.local_addr().unwrap();
        let backend_task = tokio::spawn(async move {
            let (mut socket, _) = backend.accept().await.unwrap();
            let mut request = [0; 4];
            socket.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"ping");
            socket.write_all(b"pong").await.unwrap();
        });

        let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_address = reservation.local_addr().unwrap();
        drop(reservation);
        let proxy_task = tokio::spawn(run_nethernet_proxy(proxy_address, backend_address));
        let mut client = loop {
            match TcpStream::connect(proxy_address).await {
                Ok(stream) => break stream,
                Err(_) => tokio::task::yield_now().await,
            }
        };
        client.write_all(b"ping").await.unwrap();
        let mut response = [0; 4];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");

        proxy_task.abort();
        backend_task.await.unwrap();
    }
}
