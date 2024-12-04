use std::{collections::HashMap, io::Write, net::{TcpListener, TcpStream}, ops::Not, sync::RwLock};

use anyhow::{anyhow, Context};
use log::{debug, info, trace, warn};
use procfs::process::FDTarget;

use crate::uid_resolver::Uid;

pub type Data = HashMap<Uid, String>;

pub struct Server {
    data: RwLock<Data>,
    port: u16,
}
impl Server {
    pub fn new(data: Data, port: u16) -> Self {
        debug!("serving data {:?}", data);
        Server {
            data: RwLock::new(data),
            port,
        }
    }
    pub fn serve_blocking(&self) -> Result<(), anyhow::Error> {
        let listener = TcpListener::bind(format!("127.0.0.1:{}", self.port))
            .context("failed to acquire port 7878")?;
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
    pub fn update_data(&self, data: Data) -> Result<(), anyhow::Error> {
        let mut lock = self.data.write()
            .map_err(|_| anyhow!("failed to acquire lock"))?;
        debug!("updating data {:?}", data);
        *lock = data;
        Ok(())
    }

    fn handle_stream(&self, mut stream: TcpStream) -> Result<(), anyhow::Error> {
        let peer = stream.peer_addr()
            .context("stream doesn't have an address")?;

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
        let mut inode_local = None;
        let tcp = procfs::net::tcp()
            .unwrap_or_default()
            .into_iter()
            .chain(procfs::net::tcp6()
                .unwrap_or_default());
        for entry in tcp {
            if entry.local_address == peer {
                inode_local = Some(entry.inode);
                break
            }
        }
        let Some(inode_local) = inode_local else { return Err(anyhow!("failed to find local inode")) };

        // Find the process owning this inode.
        let processes = procfs::process::all_processes()
            .context("could not access /proc")?;
        let mut owner = None;
        for process in processes {
            let Ok(process) = process else { continue };
            let Ok(exe) = process.exe() else { continue };
            let Ok(fds) = process.fd() else { continue };
            for fd in fds {
                let Ok(fd) = fd else { continue };
                if let FDTarget::Socket(inode) = fd.target {
                    if inode_local == inode {
                        debug!("found process {} for local inode", exe.display());
                        let Ok(uid) = process.uid() else { continue };
                        debug!("found owner {} for local inode", uid);
                        owner = Some(uid);
                        break
                    }
                }
            }
        }
        let Some(owner) = owner else { return Err(anyhow!("failed to find owner"))};

        let contents = self.data.read()
            .map_err(|_| anyhow!("couldn't acquire rwlock"))?
            .get(&Uid(owner))
            .cloned()
            .unwrap_or_else(|| "{}".to_string());
        let length = contents.len();
        let response =
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {length}\r\n\r\n{contents}");
        debug!("response {}", response);
        stream.write_all(response.as_bytes())
            .context("failed to respond with OK")?;

        debug!("responded");
        stream.flush()
            .context("failed to flush")
    }
}