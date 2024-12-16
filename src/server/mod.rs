use std::{
    collections::HashMap,
    io::Write,
    net::{TcpListener, TcpStream},
    ops::Not,
    sync::RwLock,
};

use anyhow::{anyhow, Context};

#[allow(unused)]
use log::{debug, info, trace, warn};

#[cfg(target_family="unix")]
use crate::unix::uid_resolver::Uid;
#[cfg(target_os="linux")]
use crate::unix::linux::procfs::find_peer_owner;

/// The pre-serialized data to serve.
///
/// We take the path of "almost static HTTP server", as it makes
/// for a simpler data model.
pub type Data = HashMap<Uid, String>;

pub struct Server {
    /// The pre-serialized data to serve.
    data: RwLock<Data>,

    /// The port on which we serve.
    port: u16,
}
impl Server {
    pub fn new(data: Data, port: u16) -> Self {
        Server {
            data: RwLock::new(data),
            port,
        }
    }

    /// Start serving.
    ///
    /// Once serving is setup, this method will never return, except in case
    /// of uncatchable error.
    pub fn serve_blocking(&self) -> Result<(), anyhow::Error> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.port))
            .with_context(|| format!("Failed to acquire port {}", self.port))?;
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    warn!("stream acquisition error {}", err);
                    continue;
                }
            };
            if let Err(err) = self.handle_stream(stream) {
                warn!("stream handling error {}", err);
                continue;
            }
        }
        Ok(())
    }

    /// Replace the pre-serialized data.
    pub fn update_data(&self, data: Data) -> Result<(), anyhow::Error> {
        let mut lock = self
            .data
            .write()
            .map_err(|_| anyhow!("failed to acquire lock"))?;
        *lock = data;
        Ok(())
    }

    /// Respond to a HTTP request.
    fn handle_stream(&self, mut stream: TcpStream) -> Result<(), anyhow::Error> {
        let peer = stream
            .peer_addr()
            .context("Stream doesn't have an address")?;

        // Don't answer requests from other hosts.
        if peer.ip().is_loopback().not() {
            let response = "HTTP/1.1 403 FORBIDDEN\r\n\r\n";
            if let Err(err) = stream.write_all(response.as_bytes()) {
                warn!("error responding with FORBIDDEN {}", err);
            }
            return Err(anyhow!("this is not a request from localhost: {}", peer));
        }
        // Find out which process sent this request.
        info!("received request from port: {}", peer.port());

        // Find the inode for this port.
        let owner = find_peer_owner(peer)?;

        let contents = self
            .data
            .read()
            .map_err(|_| anyhow!("couldn't acquire rwlock"))?
            .get(&owner)
            .cloned()
            .unwrap_or_else(|| "{}".to_string());
        let length = contents.len();
        let response =
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {length}\r\n\r\n{contents}");
        debug!("response {}", response);
        stream
            .write_all(response.as_bytes())
            .context("Failed to respond with OK")?;

        debug!("responded");
        stream.flush().context("Failed to flush")
    }
}
