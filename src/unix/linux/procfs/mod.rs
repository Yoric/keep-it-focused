use std::net::SocketAddr;

use anyhow::{anyhow, Context};
use log::debug;
use procfs::process::FDTarget;

use crate::unix::uid_resolver::Uid;

/// Find the user owning a peer currently opened locally.
pub fn find_peer_owner(peer: SocketAddr) -> Result<Uid, anyhow::Error> {
    let mut inode_local = None;
    let tcp = procfs::net::tcp()
        .unwrap_or_default()
        .into_iter()
        .chain(procfs::net::tcp6().unwrap_or_default());
    for entry in tcp {
        if entry.local_address == peer {
            inode_local = Some(entry.inode);
            break;
        }
    }
    let Some(inode_local) = inode_local else {
        return Err(anyhow!("Failed to find local inode"));
    };

    // Find the process owning this inode.
    let processes = procfs::process::all_processes().context("Could not access /proc")?;
    let mut owner = None;
    for process in processes {
        let Ok(process) = process else { continue };
        let Ok(exe) = process.exe() else { continue };
        let Ok(fds) = process.fd() else { continue };
        for fd in fds {
            let Ok(fd) = fd else { continue };
            if let FDTarget::Socket(inode) = fd.target {
                if inode_local == inode {
                    debug!("found process {} for local inode, with owner {:?}", exe.display(), process.exe());
                    let Ok(uid) = process.uid() else { continue };
                    owner = Some(uid);
                    break;
                }
            }
        }
    }
    match owner {
        Some(owner) => Ok(Uid(owner)),
        None => Err(anyhow!("No owner found")) 
    }
}