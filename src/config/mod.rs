pub mod manager;

use core::fmt;
use std::{collections::HashMap, fmt::Display, hash::Hash, path::PathBuf};

use crate::types::{DayOfWeek, Domain, Interval, Username};
use anyhow::anyhow;
use globset::{Glob, GlobMatcher};
use log::trace;
use serde::{
    de::{Unexpected, Visitor},
    Deserialize, Serialize,
};

/// The absolute path to a binary (may be a glob).
#[derive(Clone)]
pub struct Binary {
    pub path: PathBuf,
    pub matcher: GlobMatcher,
}
impl Binary {
    pub fn try_new(path: &str) -> Result<Self, anyhow::Error> {
        let glob = Glob::new(path).map_err(|_| anyhow!("invalid glob {path}"))?;

        Ok(Binary {
            path: PathBuf::from(path),
            matcher: glob.compile_matcher(),
        })
    }
}
impl fmt::Debug for Binary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.path.fmt(f)
    }
}
impl Hash for Binary {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state)
    }
}
impl PartialEq for Binary {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}
impl Eq for Binary {}

impl<'de> Deserialize<'de> for Binary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrVisitor;
        impl Visitor<'_> for StrVisitor {
            type Value = Binary;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "expected a glob string")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                trace!("Binary <- {v}");
                let path = PathBuf::from(v);
                let glob = Glob::new(v).map_err(|err| {
                    E::invalid_value(Unexpected::Other(&format!("{}", err)), &"glob string")
                })?;
                let matcher = glob.compile_matcher();
                trace!("Binary -> {path:?}");
                Ok(Binary { path, matcher })
            }
        }
        deserializer.deserialize_str(StrVisitor)
    }
}
impl Serialize for Binary {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.path.to_string_lossy().as_ref())
    }
}

impl Display for Binary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.path)
    }
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
pub struct ProcessFilter {
    /// The full path to the binary being watched.
    pub binary: Binary,

    /// Intervals during which the binary is permitted.
    ///
    /// If empty, the binary is never permitted.
    #[serde(default)]
    pub permitted: Vec<Interval>,

    /// Intervals during which the binary is forbidden.
    ///
    /// This are subtracted from `permitted`. If empty,
    /// the binary is permitted exactly during the
    /// intervals specified by `permitted`.
    #[serde(default)]
    pub forbidden: Vec<Interval>,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
pub struct WebFilter {
    pub domain: Domain,

    /// Intervals during which the domain is permitted.
    ///
    /// If empty, the domain is never permitted.
    #[serde(default)]
    pub permitted: Vec<Interval>,

    /// Intervals during which the domain is forbidden.
    ///
    /// This are subtracted from `permitted`. If empty,
    /// the domain is permitted exactly during the
    /// intervals specified by `permitted`.
    #[serde(default)]
    pub forbidden: Vec<Interval>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum DayConfig {
    Copy {
        /// Copy the configuration of another day of the week.
        like: DayOfWeek,
    },
    Instructions {
        /// Block certain processes during given time periods.
        #[serde(default)]
        processes: Vec<ProcessFilter>,

        /// Block certain IPs during given time periods.
        ///
        /// Note: This doesn't work with e.g. youtube.com, as they
        /// load-balance between millions of IPs.
        #[serde(default)]
        ip: Vec<WebFilter>,

        /// Block certain domains during given time periods.
        ///
        /// Note: This requires the companion browser extension.
        #[serde(default)]
        web: Vec<WebFilter>,
    },
}
impl Default for DayConfig {
    fn default() -> Self {
        DayConfig::Instructions {
            processes: vec![],
            ip: vec![],
            web: vec![],
        }
    }
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Default)]
pub struct ResolvedDayConfig {
    /// Block certain processes during given time periods.
    pub processes: Vec<ProcessFilter>,

    /// Block certain IPs during given time periods.
    ///
    /// Note: This doesn't work with e.g. youtube.com, as they
    /// load-balance between millions of IPs.
    pub ip: Vec<WebFilter>,

    /// Block certain domains during given time periods.
    ///
    /// Note: This requires the companion browser extension.
    pub web: Vec<WebFilter>,
}

#[derive(Deserialize, Serialize, Default, Debug)]
pub struct Week(pub HashMap<DayOfWeek, DayConfig>);

/// The contents of /etc/keep-it-focused.yaml, covering the entire week.
#[derive(Deserialize, Serialize, Default, Debug)]
pub struct Config {
    #[serde(default)]
    pub users: HashMap<Username, Week>,
}

/// The contents of a patch file, valid only for one day.
#[derive(Deserialize, Serialize, Default, Debug)]
pub struct Extension {
    pub users: HashMap<Username, ResolvedDayConfig>,
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use crate::{
        config::{DayConfig, ResolvedDayConfig},
        types::{TimeOfDay, Username},
    };

    use super::{Config, DayOfWeek};

    #[test]
    fn test_config_syntax_v2() {
        let sample = r#"
            users:
                mickey:
                    monday:
                        processes:
                            - binary: /bin/test
                              permitted:
                                - start: 0911
                                  end: 0923
                    tuesday:
                        like: monday
                    WEDanythinggoes:
                        like: monday
                mouse:
                    monday:
                        processes:                        
                            - binary: /**/snap/test/**
                              user: duck
                              forbidden: []
                              permitted:
                                - start: 0000
                                  end:   0001
                                - start: 0002
                                  end:   0003
        "#;
        let mut config: Config = serde_yaml::from_str(sample).expect("invalid config");
        let mut mickey = config
            .users
            .remove(&Username("mickey".to_string()))
            .expect("missing user mickey");
        let mickey_monday = mickey.0.remove(&DayOfWeek::monday()).unwrap();
        let mickey_monday = match mickey_monday {
            DayConfig::Instructions { web, processes, ip } => {
                ResolvedDayConfig { web, processes, ip }
            }
            _ => panic!(),
        };
        let mickey_tuesday = mickey.0.remove(&DayOfWeek::tuesday()).unwrap();
        let mickey_tuesday = match mickey_tuesday {
            DayConfig::Instructions { web, processes, ip } => {
                ResolvedDayConfig { web, processes, ip }
            }
            _ => panic!(),
        };
        let mickey_wed = mickey.0.remove(&DayOfWeek::wednesday()).unwrap();
        let mickey_wed = match mickey_wed {
            DayConfig::Instructions { web, processes, ip } => {
                ResolvedDayConfig { web, processes, ip }
            }
            _ => panic!(),
        };
        assert_eq!(mickey_monday.processes.len(), 1);
        assert_eq!(
            mickey_monday.processes[0].binary.path,
            PathBuf::from("/bin/test")
        );
        assert_eq!(mickey_monday.processes[0].permitted.len(), 1);
        assert_eq!(
            mickey_monday.processes[0].permitted[0].start,
            TimeOfDay {
                hours: 9,
                minutes: 11
            }
        );
        assert_eq!(mickey_monday, mickey_tuesday);
        assert_eq!(mickey_wed, mickey_tuesday);
        assert_eq!(mickey.0.len(), 3);
    }
}
