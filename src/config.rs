use std::{collections::HashSet, fmt::Display, path::PathBuf, time::Duration};

use chrono::Timelike;
use globset::{Glob, GlobMatcher};
use lazy_regex::lazy_regex;
use serde::{
    de::{Unexpected, Visitor},
    Deserialize, Serialize,
};
use validator::{Validate, ValidationError};

/// The absolute path to a binary (may be a glob).
#[derive(Debug)]
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
        impl<'d> Visitor<'d> for StrVisitor {
            type Value = Binary;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "expected a glob string")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let path = PathBuf::from(v);
                let glob = Glob::new(v).map_err(|err| {
                    E::invalid_value(Unexpected::Other(&format!("{}", err)), &"glob string")
                })?;
                let matcher = glob.compile_matcher();
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

/// A time of day.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub struct TimeOfDay {
    pub hours: u8,
    pub minutes: u8,
}
impl From<TimeOfDay> for std::time::Duration {
    fn from(t: TimeOfDay) -> std::time::Duration {
        std::time::Duration::new(t.hours as u64 * 3_600 + t.minutes as u64 * 60, 0)
    }
}
impl<Tz: chrono::TimeZone> From<chrono::DateTime<Tz>> for TimeOfDay {
    fn from(value: chrono::DateTime<Tz>) -> Self {
        TimeOfDay {
            hours: value.hour() as u8,
            minutes: value.minute() as u8,
        }
    }
}
impl PartialOrd for TimeOfDay {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimeOfDay {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.hours
            .cmp(&other.hours)
            .then_with(|| self.minutes.cmp(&other.minutes))
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrVisitor;
        impl<'d> Visitor<'d> for StrVisitor {
            type Value = TimeOfDay;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a time of day in format HHMM, e.g. \"1135\" (11:35 am) or \"1759\" (5:59pm)",
                )
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v.len() != 4 {
                    return Err(E::invalid_length(v.len(), &"4"));
                }
                let re = lazy_regex!("([0-2][0-9])([0-5][0-9])");
                let Some(captures) = re.captures(v) else {
                    return Err(E::invalid_value(
                        Unexpected::Str(v),
                        &"a time of day, e.g. \"1135\" (11:35 am) or \"1759\" (5:59pm)",
                    ));
                };
                let (_, [hh, mm]) = captures.extract();
                let Ok(h) = hh.parse::<u8>() else {
                    return Err(E::invalid_value(
                        Unexpected::Str(hh),
                        &"a number between 00 and 23",
                    ));
                };
                let Ok(m) = mm.parse::<u8>() else {
                    return Err(E::invalid_value(
                        Unexpected::Str(mm),
                        &"a number between 00 and 59",
                    ));
                };
                // Validate.
                match (h, m) {
                    (24, 00) => {}
                    _ if h > 23 => {
                        return Err(E::invalid_value(
                            Unexpected::Str(hh),
                            &"a number between 00 and 23",
                        ));
                    }
                    _ if m > 59 => {
                        return Err(E::invalid_value(
                            Unexpected::Str(mm),
                            &"a number between 00 and 59",
                        ));
                    }
                    _ => {}
                }

                Ok(TimeOfDay {
                    hours: h,
                    minutes: m,
                })
            }
        }
        deserializer.deserialize_str(StrVisitor)
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&format!("{:02}{:02}", self.hours, self.minutes))
    }
}

#[derive(Deserialize, Serialize)]
pub struct Interval {
    #[serde(default = "Interval::default_start")]
    pub start: TimeOfDay,

    #[serde(default = "Interval::default_end")]
    pub end: TimeOfDay,
}
impl Interval {
    pub fn remaining(&self, time: TimeOfDay) -> Option<std::time::Duration> {
        if self.start > time || self.end < time {
            return None;
        }
        let end: Duration = self.end.into();
        let time: Duration = time.into();
        Some(end - time)
    }
    fn default_start() -> TimeOfDay {
        TimeOfDay {
            hours: 0,
            minutes: 0,
        }
    }
    fn default_end() -> TimeOfDay {
        TimeOfDay {
            hours: 24,
            minutes: 0,
        }
    }
}

#[derive(Deserialize, Serialize, Validate)]
pub struct Watch {
    /// The username.
    pub user: String,

    /// The full path to the binary being watched.
    pub binary: Binary,

    // FIXME: Validate that there are no intersections between intervals.
    pub permitted: Vec<Interval>,
}

#[derive(Deserialize, Serialize, Validate)]
pub struct Config {
    /// The list of processes to watch and kill.
    #[validate(custom(function=Config::validate_distinct_binaries))]
    pub watch: Vec<Watch>,
}
impl Config {
    fn validate_distinct_binaries(watch: &[Watch]) -> Result<(), ValidationError> {
        let mut set = HashSet::new();
        for watch in watch {
            if !set.insert(&watch.binary.path) {
                let mut error = ValidationError::new("duplicate binary");
                error.add_param("binary".into(), &watch.binary);
                return Err(error);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::config::TimeOfDay;

    use super::Config;

    #[test]
    fn test_config_good() {
        let sample = r#"
        watch:
            - binary: /bin/test
              user: donald
              permitted:
                - start: 0911
                  end: 0923
            - binary: /bin/test2
              user: duck
              permitted:
                - start: 0000
                  end:   0001
                - start: 0002
                  end:   0003
        "#;
        let config: Config = serde_yaml::from_str(sample).expect("invalid config");
        assert_eq!(config.watch.len(), 2);
        assert_eq!(config.watch[0].binary.to_string(), "/bin/test");
        assert_eq!(config.watch[1].binary.to_string(), "/bin/test2");
        assert_eq!(
            config.watch[1].permitted[1].start,
            TimeOfDay {
                hours: 0,
                minutes: 2
            }
        );
    }

    #[test]
    fn test_config_bad_interval() {
        let sample = r#"
        watch:
            - binary: /bin/test
              permitted:
                - start: 0911
                  end: 0923
            - binary: /bin/test2
              permitted:
                - start: 0000
                  end:   0001
                - start: 0002
                  end:   2403
        "#;
        assert!(serde_yaml::from_str::<Config>(sample).is_err());
    }
}
