use std::{collections::HashMap, fmt::Display, ops::Not, path::PathBuf};

use crate::types::{DayOfWeek, Interval};
use globset::{Glob, GlobMatcher};
use log::trace;
use serde::{
    de::{Unexpected, Visitor},
    Deserialize, Serialize,
};
use validator::Validate;

/// The absolute path to a binary (may be a glob).
#[derive(Debug, Clone)]
pub struct Binary {
    pub path: PathBuf,
    pub matcher: GlobMatcher,
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

#[derive(Deserialize, Serialize, Validate, Clone, PartialEq, Debug)]
pub struct ProcessConfig {
    /// The full path to the binary being watched.
    pub binary: Binary,

    // FIXME: Validate that there are no intersections between intervals.
    pub permitted: Vec<Interval>,
}

#[derive(Deserialize, Serialize, Validate, Clone, PartialEq, Debug)]
pub struct WebConfig {
    pub domain: String,
    pub permitted: Vec<Interval>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DayConfigParser {
    Copy {
        like: DayOfWeek,
    },
    Instructions {
        #[serde(default)]
        processes: Vec<ProcessConfig>,
        #[serde(default)]
        web: Vec<WebConfig>,
    },
}

#[derive(Deserialize, Serialize, PartialEq, Debug)]
pub struct DayConfig {
    #[serde(default)]
    pub processes: Vec<ProcessConfig>,
    pub web: Vec<WebConfig>,
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
                                web: d.web.clone(),
                            },
                        );
                    }
                    Some(DayConfigParser::Instructions { processes, web }) => {
                        build_map.insert(
                            day,
                            DayConfig {
                                processes: processes.clone(),
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

#[derive(Deserialize, Serialize, Validate)]
pub struct Config {
    #[serde(default)]
    pub users: HashMap<String /*username*/, Week>,
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
