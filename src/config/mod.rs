pub mod manager;
pub mod instruction;
use core::fmt;
use std::{
    collections::{hash_map::Entry, HashMap}, fmt::Display, hash::Hash, path::PathBuf, time::Duration
};

use crate::{config::instruction::Template, types::{DayOfWeek, Domain, Interval, Username}};
use anyhow::{anyhow,};
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

    /// AN instruction to run once the binary is forbidden.
    #[serde(default)]
    pub then: Option<Template>,
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
pub struct Week(HashMap<DayOfWeek, DayConfig>);

impl Week {
    pub fn entry(&mut self, day: DayOfWeek) -> Entry<'_, DayOfWeek, DayConfig> {
        self.0.entry(day)
    }
    pub fn resolve(&self, day: DayOfWeek) -> Option<ResolvedDayConfig> {
        let mut visit = day;
        let mut visited = [false; 7];
        while let Some(config) = self.0.get(&visit) {
            visited[visit.index()] = true;
            match config {
                DayConfig::Instructions { processes, ip, web } => {
                    return Some(ResolvedDayConfig {
                        processes: processes.clone(),
                        ip: ip.clone(),
                        web: web.clone(),
                    })
                }
                DayConfig::Copy { like } if visited[like.index()] => {
                    // There's a cycle!
                    break;
                }
                DayConfig::Copy { like } => {
                    visit = *like;
                }
            }
        }
        None
    }
}

/// The contents of /etc/keep-it-focused.yaml, covering the entire week.
#[derive(Deserialize, Serialize, Default, Debug)]
pub struct Config {
    #[serde(default)]
    pub users: HashMap<Username, Week>,

    #[serde(default="Config::default_interval")]
    pub interval: Duration,
}
impl Config {
    fn default_interval() -> Duration {
        Duration::from_secs(15)
    }
}

/// The contents of a patch file, valid only for one day.
#[derive(Deserialize, Serialize, Default, Debug)]
pub struct Extension {
    pub users: HashMap<Username, ResolvedDayConfig>,
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use crate::types::{TimeOfDay, Username};

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
                    thur:
                        processes: []
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
        let mickey = config
            .users
            .remove(&Username("mickey".to_string()))
            .expect("missing user mickey");
        let mickey_monday = mickey
            .resolve(DayOfWeek::monday())
            .expect("Could not resolve monday");
        let mickey_tuesday = mickey
            .resolve(DayOfWeek::tuesday())
            .expect("Could not resolve tuesday");
        let mickey_wed = mickey
            .resolve(DayOfWeek::wednesday())
            .expect("Could not resolve wednesday");
        let mickey_thur = mickey
            .resolve(DayOfWeek::thursday())
            .expect("Could not resolve thursday");
        let _mickey_fri = mickey
            .resolve(DayOfWeek::friday())
            .map(|_| panic!("We should not have any content for friday"));
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
        assert!(mickey_monday != mickey_thur);
        assert_eq!(mickey.0.len(), 4);
    }
}
