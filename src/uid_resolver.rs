use std::collections::HashMap;
use uucore::entries::{Locate, Passwd};

use anyhow::Context;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Uid(pub u32);

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
            .with_context(|| format!("could not find information for user {name}"))?;
        let uid = Uid(passwd.uid);
        self.username_to_uid.insert(name.to_string(), uid);
        Ok(uid)
    }
}
