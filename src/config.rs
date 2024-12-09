use core::fmt;
use std::{collections::HashMap, fmt::Display, ops::Not, path::PathBuf};

use crate::types::{DayOfWeek, Interval};
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
pub struct ProcessConfig {
    /// The full path to the binary being watched.
    pub binary: Binary,
    pub permitted: Vec<Interval>,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
pub struct WebFilter {
    pub domain: String,
    pub permitted: Vec<Interval>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DayConfigParser {
    Copy {
        /// Copy the configuration of another day of the week.
        like: DayOfWeek,
    },
    Instructions {
        /// Block certain processes during given time periods.
        #[serde(default)]
        processes: Vec<ProcessConfig>,

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

#[derive(Deserialize, Serialize, PartialEq, Debug, Default)]
pub struct DayConfig {
    #[serde(default, skip_serializing_if="Vec::is_empty")]
    pub processes: Vec<ProcessConfig>,

    #[serde(default, skip_serializing_if="Vec::is_empty")]
    pub ip: Vec<WebFilter>,

    #[serde(default, skip_serializing_if="Vec::is_empty")]
    pub web: Vec<WebFilter>,
}

#[derive(Serialize)]
pub struct Week(pub HashMap<DayOfWeek, DayConfig>);

impl<'de> Deserialize<'de> for Week {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error, Unexpected};
        trace!("attempting to parse week");
        let mut parse_map = HashMap::<DayOfWeek, DayConfigParser>::deserialize(deserializer)?;
        let mut build_map = HashMap::<DayOfWeek, DayConfig>::new();

        trace!("attempting to normalize week");
        // Let's be a bit hackish here. As there are exactly 7 per week, we need at most 7 steps to flatten any reference.
        for _ in 0..7 {
            for day in [
                DayOfWeek::monday(),
                DayOfWeek::tuesday(),
                DayOfWeek::wednesday(),
                DayOfWeek::thursday(),
                DayOfWeek::friday(),
                DayOfWeek::saturday(),
                DayOfWeek::sunday(),
            ] {
                match parse_map.get(&day) {
                    None => continue,
                    Some(DayConfigParser::Copy { like: other }) => {
                        // Attempt to resolve.
                        let Some(d) = build_map.get(other) else {
                            continue;
                        };
                        build_map.insert(
                            day,
                            DayConfig {
                                processes: d.processes.clone(),
                                ip: d.ip.clone(),
                                web: d.web.clone(),
                            },
                        );
                    }
                    Some(DayConfigParser::Instructions { processes, ip, web }) => {
                        build_map.insert(
                            day,
                            DayConfig {
                                processes: processes.clone(),
                                ip: ip.clone(),
                                web: web.clone(),
                            },
                        );
                    }
                }
                parse_map.remove(&day);
            }
        }
        if parse_map.is_empty().not() {
            return Err(D::Error::invalid_value(
                Unexpected::Other("cycle within day definitions"),
                &"a DAG of day definitions",
            ));
        }
        Ok(Week(build_map))
    }
}

/// The contents of /etc/keep-it-focused.yaml, covering the entire week.
#[derive(Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub users: HashMap<String /*username*/, Week>,
}

/// The contents of a patch file, valid only for one day.
#[derive(Deserialize, Serialize, Default)]
pub struct Extension {
    pub users: HashMap<String, DayConfig>,
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use crate::types::TimeOfDay;

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
                              permitted:
                                - start: 0000
                                  end:   0001
                                - start: 0002
                                  end:   0003
        "#;
        let config: Config = serde_yaml::from_str(sample).expect("invalid config");
        let mickey = config.users.get("mickey").expect("missing user mickey");
        let mickey_monday = mickey.0.get(&DayOfWeek::monday()).unwrap();
        let mickey_tuesday = mickey.0.get(&DayOfWeek::tuesday()).unwrap();
        let mickey_wed = mickey.0.get(&DayOfWeek::wednesday()).unwrap();
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
