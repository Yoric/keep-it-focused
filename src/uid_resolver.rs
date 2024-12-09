use std::collections::HashMap;
use log::debug;
use uucore::entries::{uid2usr, Locate, Passwd};

use anyhow::{anyhow, Context};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Uid(pub u32);
impl Uid {
    pub fn is_root(&self) -> bool {
        self.0 == 0
    }
    pub fn me() -> Uid {
        Uid(unsafe { libc::getuid() })
    }
    pub fn name(&self) -> Result<String, anyhow::Error> {
        uid2usr(self.0).with_context(|| anyhow!("cannot find user {}", self.0))
    }
}

pub struct Resolver {
    username_to_uid: HashMap<String, Uid>,
}

impl Default for Resolver {
    fn default() -> Self {
        Resolver::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        Resolver {
            username_to_uid: HashMap::new(),
        }
    }
    pub fn resolve(&mut self, name: &str) -> Result<Uid, anyhow::Error> {
        if let Some(uid) = self.username_to_uid.get(name) {
            return Ok(*uid);
        }
        let passwd = Passwd::locate(name)
            .with_context(|| format!("Could not find information for user {name}"))?;
        let uid = Uid(passwd.uid);
        self.username_to_uid.insert(name.to_string(), uid);
        debug!("resolved user {name} => {}", uid.0);
        Ok(uid)
    }
}
