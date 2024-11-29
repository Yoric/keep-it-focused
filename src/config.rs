use std::{collections::HashMap, fmt::Display, ops::Not, path::PathBuf, time::Duration};

use chrono::{Datelike, Timelike};
use globset::{Glob, GlobMatcher};
use lazy_regex::lazy_regex;
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
        impl<'d> Visitor<'d> for StrVisitor {
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
        use serde::de::Error;
        trace!("TimeOfDayParser");
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let (maybe_str, maybe_u64) = (value.as_str(), value.as_u64());
        let (h, m) = match (maybe_str, maybe_u64) {
            (_, Some(num)) => (num / 100, num % 100),
            (Some(source), _) => {
                trace!("TimeOfDayParser str {source}");
                if source.len() != 4 {
                    return Err(D::Error::invalid_length(source.len(), &"4"));
                }
                let re = lazy_regex!("([0-2][0-9])([0-5][0-9])");
                let Some(captures) = re.captures(source) else {
                    return Err(D::Error::invalid_value(
                        Unexpected::Str(source),
                        &"a time of day, e.g. \"1135\" (11:35 am) or \"1759\" (5:59pm)",
                    ));
                };
                let (_, [hh, mm]) = captures.extract();
                let Ok(hh) = hh.parse::<u64>() else {
                    return Err(D::Error::invalid_value(
                        Unexpected::Str(hh),
                        &"a number between 00 and 23",
                    ));
                };
                let Ok(mm) = mm.parse::<u64>() else {
                    return Err(D::Error::invalid_value(
                        Unexpected::Str(mm),
                        &"a number between 00 and 59",
                    ));
                };
                (hh, mm)
            }
            (None, None) => return Err(D::Error::invalid_value(
                Unexpected::Other(&format!("{value:?}")),
                &"an hour in military time",
            ))
        };
        trace!("TimeOfDayParser {h} {m}");
        match (h, m) {
            (24, 00) => {}
            _ if h > 23 => {
                return Err(D::Error::invalid_value(
                    Unexpected::Str(&format!("{h}")),
                    &"a number between 00 and 23",
                ));
            }
            _ if m> 59 => {
                return Err(D::Error::invalid_value(
                    Unexpected::Str(&format!("{m}")),
                    &"a number between 00 and 59",
                ));
            }
            _ => {}
        }
        trace!("TimeOfDayParser - success");
        Ok(TimeOfDay {
            hours: h as u8,
            minutes: m as u8,
        })
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

#[derive(Serialize, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Clone, Copy)]
pub struct DayOfWeek(u8);
impl DayOfWeek {
    pub fn now() -> Self {
        Self(chrono::Local::now().weekday().num_days_from_monday() as u8)
    }
    pub fn monday() -> Self {
        DayOfWeek(0)
    }
    pub fn tuesday() -> Self {
        DayOfWeek(1)
    }
    pub fn wednesday() -> Self {
        DayOfWeek(2)
    }
    pub fn thursday() -> Self {
        DayOfWeek(3)
    }
    pub fn friday() -> Self {
        DayOfWeek(4)
    }
    pub fn saturday() -> Self {
        DayOfWeek(5)
    }
    pub fn sunday() -> Self {
        DayOfWeek(6)
    }
}
impl Display for DayOfWeek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let day = match self.0 {
            0 => "monday",
            1 => "tuesday",
            2 => "wednesday",
            3 => "thursday",
            4 => "friday",
            5 => "saturday",
            6 => "sunday",
            other => panic!("invalid value for DayOfWeek: {}", other),
        };
        f.write_str(day)
    }
}
impl<'de> Deserialize<'de> for DayOfWeek {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct MyVisitor;
        impl<'d> Visitor<'d> for MyVisitor {
            type Value = DayOfWeek;

            fn visit_str<E: serde::de::Error>(self, source: &str) -> Result<Self::Value, E> {
                let prefix = &source.to_ascii_lowercase()[0..3];
                trace!("DayOfWeek - attempting to deserialize string {prefix}");
                let result = match prefix {
                    "0" | "mon" => Ok(DayOfWeek(0)),
                    "1" | "tue" => Ok(DayOfWeek(1)),
                    "2" | "wed" | "wen" => Ok(DayOfWeek(2)),
                    "3" | "thu" => Ok(DayOfWeek(3)),
                    "4" | "fri" => Ok(DayOfWeek(4)),
                    "5" | "sat" => Ok(DayOfWeek(5)),
                    "6" | "sun" => Ok(DayOfWeek(6)),
                    _ => Err(E::invalid_value(
                        Unexpected::Other(source),
                        &"day of week (either a number in [0, 6] or a named day",
                    )),
                };
                trace!("DayOfWeek - deserialized {prefix} to {result:?}");
                result
            }
            fn visit_u8<E: serde::de::Error>(self, source: u8) -> Result<Self::Value, E> {
                if source <= 6 {
                    trace!("DayOfWeek - attempting to deserialize number {source}");
                    Ok(DayOfWeek(source))
                } else {
                    Err(E::invalid_value(
                        Unexpected::Other(&format!("{}", source)),
                        &"day of week (either a number in [0, 6] or a named day",
                    ))
                }
            }
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                trace!("DayOfWeek - error");
                write!(formatter, "expecting either a numbered day of week (Monday = 0) or a named day of week (Monday/mon/...)")
            }
        }
        deserializer.deserialize_any(MyVisitor {})
    }
}



#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
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

#[derive(Deserialize, Serialize, Validate, Clone, PartialEq, Debug)]
pub struct ProcessConfig {
    /// The full path to the binary being watched.
    pub binary: Binary,

    // FIXME: Validate that there are no intersections between intervals.
    pub permitted: Vec<Interval>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DayConfigParser {
    Copy {
        like: DayOfWeek
    },
    Instructions {
        processes: Vec<ProcessConfig>,
    },
}

/*
impl<'de> Deserialize<'de> for DayConfigParser {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de> {          
             use serde::de::Error;

        struct MyVisitor;
        impl<'d> Visitor<'d> for MyVisitor {
            type Value = DayConfigParser;
        
            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
                where
                    A: serde::de::MapAccess<'d>, {
                let value = serde_yaml::Value::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                if let Some(processes) = value.get("processes") {
                    let processes = Vec::<ProcessConfig>::deserialize(processes)
                        .map_err(|err| A::Error::)?;
                }
                unimplemented!()
            }

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                todo!()
            }
        }
        deserializer.deserialize_map(MyVisitor{})
    }
}
 */

#[derive(Deserialize, Serialize, PartialEq, Debug)]
pub struct DayConfig {
    #[serde(default)]
    pub processes: Vec<ProcessConfig>,
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
                    Some(DayConfigParser::Copy { like: other}) => {
                        // Attempt to resolve.
                        let Some(d) = build_map.get(other) else { continue };
                        build_map.insert(day, DayConfig { processes: d.processes.clone() });
                    }
                    Some(DayConfigParser::Instructions { processes }) => {
                        build_map.insert(day, DayConfig { processes: processes.clone() });
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

    use crate::config::TimeOfDay;

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
        assert_eq!(mickey_monday.processes[0].binary.path, PathBuf::from("/bin/test"));
        assert_eq!(mickey_monday.processes[0].permitted.len(), 1);
        assert_eq!(mickey_monday.processes[0].permitted[0].start, TimeOfDay { hours: 9, minutes: 11});
        assert_eq!(mickey_monday, mickey_tuesday);
        assert_eq!(mickey_wed, mickey_tuesday);
        assert_eq!(mickey.0.len(), 3);
    }
}
