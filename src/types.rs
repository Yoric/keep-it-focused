use std::{fmt::Display, time::Duration};

use chrono::{Datelike, Timelike};
use lazy_regex::lazy_regex;
use log::trace;
use serde::{
    de::{Unexpected, Visitor},
    Deserialize, Serialize,
};

/// A time of day.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub struct TimeOfDay {
    pub hours: u8,
    pub minutes: u8,
}
impl TimeOfDay {
    pub fn as_iptables_arg(&self) -> String {
        format!("{:02}:{:02}", self.hours, self.minutes)
    }
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

pub const DAY_BEGINS: TimeOfDay = TimeOfDay {
    hours: 0,
    minutes: 0,
};
pub const DAY_ENDS: TimeOfDay = TimeOfDay {
    hours: 24,
    minutes: 0,
};

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        trace!("TimeOfDayParser");
        // untagged enum parsers are really bad for error messages, so we use an intermediate
        // yaml parser
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
            (None, None) => {
                return Err(D::Error::invalid_value(
                    Unexpected::Other(&format!("{value:?}")),
                    &"an hour in military time",
                ))
            }
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
            _ if m > 59 => {
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

impl<'de> Deserialize<'de> for DayOfWeek {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct MyVisitor;
        impl Visitor<'_> for MyVisitor {
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
    pub fn as_iptables_arg(&self) -> &'static str {
        match self.0 {
            0 => "Mo",
            1 => "Tu",
            2 => "We",
            3 => "Th",
            4 => "Fr",
            5 => "Sa",
            6 => "Su",
            _ => unreachable!(),
        }
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
        DAY_BEGINS
    }
    fn default_end() -> TimeOfDay {
        DAY_ENDS
    }
}

pub fn resolve_intervals(mut intervals: Vec<Interval>) -> Vec<Interval> {
    intervals.sort_by_key(|interval| interval.start);
    let mut normalized: Vec<Interval> = vec![];
    for interval in intervals {
        if let Some(latest) = normalized.last_mut() {
            // Merge two contiguous intervals.
            if latest.end >= interval.start {
                latest.end = interval.end;
                continue;
            }
        }
        normalized.push(interval);
    }
    normalized
}

/// From a list of intervals within a day, return the list of complementary intervals,
///
/// ```
/// use keep_it_focused::types::*;
/// let complement = complement_intervals(vec![
///   Interval { // This interval represents 12:15-13:37
///     start: TimeOfDay { hours: 12, minutes: 15 },
///     end: TimeOfDay  { hours: 13, minutes: 37 },
///   }
/// ]);
/// assert_eq!(complement, vec![
///    Interval { // 00:00-12:15
///       start: TimeOfDay { hours: 0, minutes: 0 },
///       end: TimeOfDay { hours: 12, minutes: 15 },
///    },
///    Interval { // 13:37-24:00
///       start: TimeOfDay { hours: 13, minutes: 37 },
///       end: TimeOfDay { hours: 24, minutes: 00 },
///    }
/// ]);
/// ```
pub fn complement_intervals(intervals: Vec<Interval>) -> Vec<Interval> {
    let normalized = resolve_intervals(intervals);

    // Obtain the intervals during which use is forbidden.
    let mut complement = Vec::new();
    if normalized.is_empty() {
        complement.push(Interval {
            start: TimeOfDay {
                hours: 0,
                minutes: 0,
            },
            end: TimeOfDay {
                hours: 24,
                minutes: 0,
            },
        });
    } else {
        let mut latest_in = DAY_BEGINS;
        for interval in normalized {
            if interval.start > latest_in {
                complement.push(Interval {
                    start: latest_in,
                    end: interval.start,
                });
            }
            latest_in = interval.end;
        }
        if latest_in < DAY_ENDS {
            complement.push(Interval {
                start: latest_in,
                end: DAY_ENDS,
            });
        }
    }
    complement
}
