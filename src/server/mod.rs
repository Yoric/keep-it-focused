use std::{
    collections::HashMap,
    ops::Not, sync::Arc, time::Duration,
};

use anyhow::{anyhow, Context};

use lazy_regex::lazy_regex;
#[allow(unused)]
use log::{debug, info, trace, warn};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, net::{TcpListener, TcpStream}, sync::{Notify, RwLock}};

#[cfg(target_os = "linux")]
use crate::unix::linux::procfs::find_peer_owner;
#[cfg(target_family = "unix")]
use crate::unix::uid_resolver::Uid;

const WAIT_TIMEOUT_SEC: u64 = 3600;

/// The pre-serialized data to serve.
///
/// We take the path of "almost static HTTP server", as it makes
/// for a simpler data model.
pub struct Data {
    store: RwLock<HashMap<Uid, String>>,
    notify: Notify,
}
impl Data {
    pub fn new(data: HashMap<Uid, String>) -> Self {
        Self {
            store: RwLock::new(data),
            notify: Notify::new(),
        }
    }
}

pub struct Server {
    /// The pre-serialized data to serve.
    data: Arc<Data>,

    /// The port on which we serve.
    port: u16,
}
impl Server {
    pub fn new(data: HashMap<Uid, String>, port: u16) -> Self {
        Server {
            data: Arc::new(Data::new(data)),
            port,
        }
    }

    /// Start serving.
    ///
    /// Once serving is setup, this method will never return, except in case
    /// of uncatchable error.
    pub async fn serve_blocking(&self) -> Result<(), anyhow::Error> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.port))
            .await
            .with_context(|| format!("Failed to acquire port {}", self.port))?;
        while let Ok((stream, _)) = listener.accept().await {
            let data = self.data.clone();
            tokio::spawn(async move {
                // We're responding slowly, by design, so we want each stream to run in its own task.
                if let Err(err) = Self::handle_stream(stream, data.as_ref()).await {
                    warn!("stream handling error {}", err);
            }});
        }
        Ok(())
    }

    /// Replace the pre-serialized data.
    pub async fn update_data(&self, data: HashMap<Uid, String>) -> Result<(), anyhow::Error> {
        {
            let mut lock = self
                .data
                .store
                .write()
                .await;
            *lock = data;
        }
        self.data.notify.notify_waiters();
        Ok(())
    }

    /// Respond to a HTTP request.
    async fn handle_stream(mut stream: TcpStream, data: &Data) -> Result<(), anyhow::Error> {
        let peer = stream
            .peer_addr()
            .context("Stream doesn't have an address")?;

        // Don't answer requests from other hosts.
        if peer.ip().is_loopback().not() {
            let response = "HTTP/1.1 403 FORBIDDEN\r\n\r\n";
            if let Err(err) = stream.write_all(response.as_bytes()).await {
                warn!("error responding with FORBIDDEN {}", err);
            }
            return Err(anyhow!("this is not a request from localhost: {}", peer));
        }
        // Find out which process sent this request.
        info!("received request from port: {}", peer.port());

        // Find the inode for this port.
        let owner = find_peer_owner(peer)?;

        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        let get_re = lazy_regex!("GET (.*) HTTP.*");
        let url = loop {
            debug!("reading to string");
            line.clear();
            tokio::select!{
                result = reader.read_line(&mut line) => {
                    result?;
                    let Some(captures) = get_re.captures(&line) else {
                        // Wait for next line.
                        continue;
                    };
                    break captures[1].to_string();
                }
            _ = tokio::time::sleep(Duration::from_secs(WAIT_TIMEOUT_SEC)) => {
                    return Err(anyhow!("timeout exceeded, giving up on connection"));
                }
            }
        };

        let expect_immediate_result = url == "/immediate";

        // Unless we need an immediate result, wait for an update.
        if expect_immediate_result {
            debug!("immediate response requested, responding");
        } else {
            tokio::select! {
                _ = data.notify.notified() => {
                    debug!("data was modified, responding");
                }
                _ = tokio::time::sleep(Duration::from_secs(WAIT_TIMEOUT_SEC)) => {
                    debug!("timeout exceeded, responding");
                }
            };    
        }

        // Respond with the latest version.
        let contents = data
            .store
            .read()
            .await
            .get(&owner)
            .cloned()
            .unwrap_or_else(|| "{}".to_string());
        let length = contents.len();
        let response =
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {length}\r\n\r\n{contents}");
        debug!("response {}", response);
        stream
            .write_all(response.as_bytes())
            .await
            .context("Failed to respond with OK")?;

        debug!("responded");
        stream.flush().await.context("Failed to flush")
    }
}
