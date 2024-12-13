use std::{
    fmt::Display,
    ops::Not,
    time::{Duration, SystemTime},
};

use anyhow::anyhow;
use chrono::{DateTime, Datelike, Local, Timelike};
use derive_more::derive::{AsRef, Deref, Display};
use lazy_regex::lazy_regex;
#[allow(unused)]
use log::{debug, trace};
use serde::{
    de::{Unexpected, Visitor},
    Deserialize, Serialize,
};
use typed_builder::TypedBuilder;

/// A time of day.
#[derive(PartialEq, Eq, Debug, Clone, Copy, TypedBuilder)]
pub struct TimeOfDay {
    pub hours: u8,
    #[builder(default = 0)]
    pub minutes: u8,
}

impl TimeOfDay {
    pub fn as_iptables_arg(&self) -> String {
        format!("{:02}:{:02}", self.hours, self.minutes)
    }
    pub fn as_minutes(&self) -> u16 {
        self.minutes as u16 + self.hours as u16 * 60
    }
    pub fn from_minutes(minutes: u16) -> Self {
        let mm = (minutes % 60) as u8;
        let hh = ((minutes / 60) % 23) as u8;
        Self {
            hours: hh,
            minutes: mm,
        }
    }
    pub fn now() -> TimeOfDay {
        let now = Local::now();
        now.into()
    }
    pub const START: TimeOfDay = DAY_BEGINS;
    pub const END: TimeOfDay = DAY_ENDS;
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

impl TimeOfDay {
    pub fn parse(source: &str) -> Result<Self, anyhow::Error> {
        let re = lazy_regex!("([0-2][0-9]):?([0-5][0-9])");
        let Some(captures) = re.captures(source) else {
            return Err(anyhow!(
                "invalid time of day, expecting e.g. \"1135\" (11:35 am) or \"1759\" (5:59pm)"
            ));
        };
        let (_, [hh, mm]) = captures.extract();
        let Ok(hh) = hh.parse::<u64>() else {
            return Err(anyhow!("hours should be a valid number"));
        };
        let Ok(mm) = mm.parse::<u64>() else {
            return Err(anyhow!("minutes should be a valid number"));
        };
        match (hh, mm) {
            (24, 00) => Ok(DAY_ENDS),
            (0..=23, 00..=59) => Ok(TimeOfDay {
                hours: hh as u8,
                minutes: mm as u8,
            }),
            (0..=23, _) => Err(anyhow!(
                "invalid minutes {mm}, expected a number in [0, 59]"
            )),
            _ => Err(anyhow!("invalid hours {hh}, expected a number in [0, 23]")),
        }
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        // untagged enum parsers are really bad for error messages, so we use an intermediate
        // yaml parser
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let (maybe_str, maybe_u64) = (value.as_str(), value.as_u64());
        let (h, m) = match (maybe_str, maybe_u64) {
            (_, Some(num)) => (num / 100, num % 100),
            (Some(source), _) => {
                if source.len() != 4 {
                    return Err(D::Error::invalid_length(source.len(), &"4"));
                }
                let re = lazy_regex!("([0-2][0-9]):?([0-5][0-9])");
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

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Clone, Copy)]
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
    pub fn parse(source: &str) -> Result<Self, anyhow::Error> {
        let day = match source[0..3].to_ascii_lowercase().as_str() {
            "mon" | "0" => Self::monday(),
            "tue" | "1" => Self::tuesday(),
            "wed" | "2" => Self::wednesday(),
            "thu" | "3" => Self::thursday(),
            "fri" | "4" => Self::friday(),
            "sat" | "5" => Self::saturday(),
            "sun" | "6" => Self::sunday(),
            _ => {
                return Err(anyhow!(
                    "invalid day '{source}', expected one of mon, tue, wed, thu, fri, sat, sun"
                ))
            }
        };
        Ok(day)
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
impl Serialize for DayOfWeek {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(&format!("{}", self))
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
    /// Return the length of an interval, in minutes.
    pub fn len(&self) -> u16 {
        self.end.as_minutes() - self.start.as_minutes()
    }
    pub fn intersects(&self, other: &Self) -> bool {
        if self.start <= other.start && self.end >= other.start {
            return true;
        }
        if self.start <= other.end && self.end >= other.end {
            return true;
        }
        false
    }
    pub fn merge(&self, other: &Self) -> Option<Self> {
        if self.intersects(other).not() {
            return None;
        }
        Some(Interval {
            start: TimeOfDay::min(self.start, other.start),
            end: TimeOfDay::max(self.end, other.end),
        })
    }
    pub fn is_empty(&self) -> bool {
        assert!(self.start <= self.end);
        self.start < self.end
    }
    fn default_start() -> TimeOfDay {
        DAY_BEGINS
    }
    fn default_end() -> TimeOfDay {
        DAY_ENDS
    }
    pub fn subtract(self, other: Interval) -> IntervalSubtraction {
        match () {
            // `self` included in `other`.
            _ if self.start >= other.start && self.end <= other.end => IntervalSubtraction::Empty,
            // Intervals do not overlap.
            _ if self.start >= other.end => IntervalSubtraction::MissLeft(self),
            _ if self.end <= other.start => IntervalSubtraction::MissRight(self),
            // Intervals overlap but `other` is not included in `self`
            _ if self.start >= other.start && self.end > other.end => {
                IntervalSubtraction::HitLeft(Interval {
                    start: other.end,
                    end: self.end,
                })
            }
            _ if self.start <= other.start && self.end < other.end => {
                IntervalSubtraction::HitLeft(Interval {
                    start: self.start,
                    end: other.start,
                })
            }
            // `other` included in `self`
            _ => IntervalSubtraction::HitCenter(
                Interval {
                    start: self.start,
                    end: other.start,
                },
                Interval {
                    start: other.end,
                    end: self.end,
                },
            ),
        }
    }
}

/// The result of computing A - B on intervals
pub enum IntervalSubtraction {
    /// No overlap, B.start < B.end <= A.start.
    MissLeft(Interval),

    /// Overlap, B.start <= A.start < B.end < A.end.
    HitLeft(Interval),

    HitCenter(Interval, Interval),

    /// Overlap, A.start <= B.start < A.end < B.end.
    HitRight(Interval),

    /// No overlap, B strictly after A
    MissRight(Interval),

    /// A included in B.
    Empty,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AcceptedInterval(pub Interval);
impl AcceptedInterval {
    /// Simplify a bunch of accepted intervals.
    pub fn simplify(mut intervals: Vec<AcceptedInterval>) -> Vec<AcceptedInterval> {
        intervals.sort_by_key(|interval| interval.0.start);
        let mut normalized: Vec<AcceptedInterval> = vec![];
        for interval in intervals {
            if let Some(latest) = normalized.last_mut() {
                if let Some(merged) = latest.0.merge(&interval.0) {
                    latest.0 = merged;
                    continue;
                }
            }
            // Otherwise, append interval
            normalized.push(interval.clone());
        }
        normalized
    }

    /// ```
    /// use keep_it_focused::types::*;
    /// let accepted = vec![AcceptedInterval(Interval { start: TimeOfDay::START, end: TimeOfDay::END})];
    /// let rejected = vec![RejectedInterval(Interval { start: TimeOfDay { hours: 12, minutes: 0}, end: TimeOfDay { hours: 12, minutes: 5} })];
    ///
    /// let difference = AcceptedInterval::subtract(accepted, rejected);
    /// assert_eq!(difference, vec![
    ///     AcceptedInterval(Interval { start: TimeOfDay::START, end: TimeOfDay { hours: 12, minutes: 0} }),
    ///     AcceptedInterval(Interval { start: TimeOfDay { hours: 12, minutes: 5}, end: TimeOfDay::END }),
    /// ])
    /// ```
    pub fn subtract(
        accepted: Vec<AcceptedInterval>,
        rejected: Vec<RejectedInterval>,
    ) -> Vec<AcceptedInterval> {
        if rejected.is_empty() {
            return accepted;
        }
        let mut accepted = Self::simplify(accepted).into_iter().peekable();
        let mut rejected = RejectedInterval::simplify(rejected).into_iter().peekable();
        let mut committed = vec![];
        while let Some(acc) = accepted.peek_mut() {
            // Loop invariant: at each step, either
            // - we consume `acc`; or
            // - we consume `rej`; or
            // - `acc` grows strictly smaller.
            let Some(rej) = rejected.peek().cloned() else {
                // We're done, copy whatever's left.
                break;
            };
            match acc.clone().0.subtract(rej.0) {
                IntervalSubtraction::Empty => {
                    // `acc` was fully consumed, but `rej` may have intersections with further accepted intervals,
                    accepted.next().unwrap();
                }
                IntervalSubtraction::MissLeft(_) => {
                    // Since `accepted` is sorted, `rejected` won't intersect with any further accepted interval.
                    rejected.next().unwrap();
                    // However, `acc` may still intersect with further rejected intervals.
                }
                IntervalSubtraction::HitLeft(difference) => {
                    // Since `accepted` is sorted, `rejected` won't intersect with any further accepted interval.
                    rejected.next().unwrap();
                    // However, `difference` may still intersect with further rejected intervals.
                    *acc = AcceptedInterval(difference);
                }
                IntervalSubtraction::HitCenter(left, right) => {
                    // Since `accepted` has no intersection between its intervals, `rejected` won't intersect with
                    // any further accepted interval.
                    rejected.next().unwrap();
                    // Since `rejected` is sorted, any further rejected interval will not intersect with `left`.
                    committed.push(AcceptedInterval(left));
                    *acc = AcceptedInterval(right);
                }
                IntervalSubtraction::HitRight(difference) => {
                    // We may still have intersections between `difference` and any further rejected interval.
                    // However, `difference` is strictly smaller than `acc`.
                    assert!(difference.len() < acc.0.len());
                    *acc = AcceptedInterval(difference);
                }
                IntervalSubtraction::MissRight(unchanged) => {
                    // Since `rejected` is sorted, `acc` won't intersect with any further rejected interval.
                    accepted.next().unwrap();
                    committed.push(AcceptedInterval(unchanged));
                }
            }
        }
        // Copy everything else
        committed.extend(accepted);
        committed
    }
}

/// A difference between two unions of intervals.
#[derive(Default)]
pub struct IntervalsDiff {
    pub accepted: Vec<AcceptedInterval>,
    pub rejected: Vec<RejectedInterval>,
}
impl IntervalsDiff {
    pub fn compute_accepted_intervals(from: Vec<IntervalsDiff>) -> Vec<AcceptedInterval> {
        // Successively add `accepted`, reject `rejected`.
        let mut accepted = vec![];
        for diff in from {
            accepted.extend(diff.accepted);
            accepted = AcceptedInterval::subtract(accepted, diff.rejected);
        }
        accepted
    }
    pub fn compute_rejected_intervals(from: Vec<IntervalsDiff>) -> Vec<RejectedInterval> {
        RejectedInterval::complement(Self::compute_accepted_intervals(from))
    }
}

/// From a list of intervals within a day, return the list of complementary intervals,
///
/// ```
/// use keep_it_focused::types::*;
/// let complement = RejectedInterval::complement(vec![
///   AcceptedInterval(Interval { // This interval represents 12:15-13:37
///     start: TimeOfDay { hours: 12, minutes: 15 },
///     end: TimeOfDay  { hours: 13, minutes: 37 },
///   })
/// ]);
/// assert_eq!(complement, vec![
///    RejectedInterval(Interval { // 00:00-12:15
///       start: TimeOfDay { hours: 0, minutes: 0 },
///       end: TimeOfDay { hours: 12, minutes: 15 },
///    }),
///    RejectedInterval(Interval { // 13:37-24:00
///       start: TimeOfDay { hours: 13, minutes: 37 },
///       end: TimeOfDay { hours: 24, minutes: 00 },
///    })
/// ]);
/// ```
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RejectedInterval(pub Interval);
impl RejectedInterval {
    /// Simplify a bunch of accepted intervals.
    pub fn simplify(mut intervals: Vec<RejectedInterval>) -> Vec<RejectedInterval> {
        intervals.sort_by_key(|interval| interval.0.start);
        let mut normalized: Vec<RejectedInterval> = vec![];
        for interval in intervals {
            if let Some(latest) = normalized.last_mut() {
                if let Some(merged) = latest.0.merge(&interval.0) {
                    latest.0 = merged;
                    continue;
                }
            }
            // Otherwise, append interval
            normalized.push(interval.clone());
        }
        normalized
    }

    /// Complement a bunch of accepted intervals into rejected intervals.
    pub fn complement(intervals: Vec<AcceptedInterval>) -> Vec<RejectedInterval> {
        let accepted = AcceptedInterval::simplify(intervals);

        // Obtain the intervals during which use is forbidden.
        let mut complement = Vec::new();
        if accepted.is_empty() {
            // Trivial case: nothing is permitted, so reject the entire day.
            complement.push(RejectedInterval(Interval {
                start: TimeOfDay {
                    hours: 0,
                    minutes: 0,
                },
                end: TimeOfDay {
                    hours: 24,
                    minutes: 0,
                },
            }));
        } else {
            let mut latest_in = DAY_BEGINS;
            for interval in accepted {
                if interval.0.start > latest_in {
                    // Nothing is permitted between `latest_in` and `interval.0.start`,
                    // so that's a new forbidden segment.
                    complement.push(RejectedInterval(Interval {
                        start: latest_in,
                        end: interval.0.start,
                    }));
                }
                latest_in = interval.0.end;
            }
            if latest_in < DAY_ENDS {
                complement.push(RejectedInterval(Interval {
                    start: latest_in,
                    end: DAY_ENDS,
                }));
            }
        }
        complement
    }
}

#[cfg(test)]
mod test {
    use itertools::Itertools;

    use crate::types::*;
    #[test]
    fn test_interval_sub() {
        let diffs = vec![
            IntervalsDiff {
                accepted: (1..10)
                    .map(|hh| {
                        AcceptedInterval(Interval {
                            start: TimeOfDay {
                                hours: hh,
                                minutes: 0,
                            },
                            end: TimeOfDay {
                                hours: hh,
                                minutes: 10,
                            },
                        })
                    })
                    .collect_vec(),
                rejected: vec![
                    // This removes 00:00 -> 00:10 and part of 01:00 -> 01:10
                    RejectedInterval(Interval {
                        start: TimeOfDay {
                            hours: 0,
                            minutes: 0,
                        },
                        end: TimeOfDay {
                            hours: 1,
                            minutes: 9,
                        },
                    }),
                    // This doesn't intersect with anything
                    RejectedInterval(Interval {
                        start: TimeOfDay {
                            hours: 1,
                            minutes: 15,
                        },
                        end: TimeOfDay {
                            hours: 1,
                            minutes: 20,
                        },
                    }),
                    RejectedInterval(Interval {
                        start: TimeOfDay {
                            hours: 3,
                            minutes: 0,
                        },
                        end: TimeOfDay {
                            hours: 3,
                            minutes: 1,
                        },
                    }),
                ],
            },
            IntervalsDiff {
                accepted: vec![AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 23,
                        minutes: 0,
                    },
                    end: TimeOfDay::END,
                })],
                rejected: vec![
                    RejectedInterval(Interval {
                        start: TimeOfDay {
                            hours: 8,
                            minutes: 59,
                        },
                        end: TimeOfDay {
                            hours: 9,
                            minutes: 9,
                        },
                    }),
                    RejectedInterval(Interval {
                        start: TimeOfDay {
                            hours: 7,
                            minutes: 1,
                        },
                        end: TimeOfDay {
                            hours: 7,
                            minutes: 11,
                        },
                    }),
                    RejectedInterval(Interval {
                        start: TimeOfDay {
                            hours: 4,
                            minutes: 50,
                        },
                        end: TimeOfDay {
                            hours: 6,
                            minutes: 11,
                        },
                    }),
                    RejectedInterval(Interval {
                        start: TimeOfDay {
                            hours: 4,
                            minutes: 5,
                        },
                        end: TimeOfDay {
                            hours: 4,
                            minutes: 7,
                        },
                    }),
                ],
            },
        ];
        let result = IntervalsDiff::compute_accepted_intervals(diffs);
        assert_eq!(
            result,
            vec![
                AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 1,
                        minutes: 9
                    },
                    end: TimeOfDay {
                        hours: 1,
                        minutes: 10
                    }
                }),
                AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 2,
                        minutes: 0
                    },
                    end: TimeOfDay {
                        hours: 2,
                        minutes: 10
                    }
                }),
                AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 3,
                        minutes: 1
                    },
                    end: TimeOfDay {
                        hours: 3,
                        minutes: 10
                    }
                }),
                AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 4,
                        minutes: 0
                    },
                    end: TimeOfDay {
                        hours: 4,
                        minutes: 5
                    }
                }),
                AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 4,
                        minutes: 7
                    },
                    end: TimeOfDay {
                        hours: 4,
                        minutes: 10
                    }
                }),
                AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 7,
                        minutes: 0
                    },
                    end: TimeOfDay {
                        hours: 7,
                        minutes: 1
                    }
                }),
                AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 8,
                        minutes: 0
                    },
                    end: TimeOfDay {
                        hours: 8,
                        minutes: 10
                    }
                }),
                AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 9,
                        minutes: 9
                    },
                    end: TimeOfDay {
                        hours: 9,
                        minutes: 10
                    }
                }),
                AcceptedInterval(Interval {
                    start: TimeOfDay {
                        hours: 23,
                        minutes: 0
                    },
                    end: TimeOfDay {
                        hours: 24,
                        minutes: 0
                    }
                }),
            ]
        )
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, Deserialize, Serialize, AsRef, Deref, Display)]
pub struct Username(pub String);

#[derive(Clone, Debug, Hash, PartialEq, Eq, Deserialize, Serialize, AsRef, Deref, Display)]
pub struct Domain(pub String);

pub fn is_today(date: SystemTime) -> bool {
    let latest_update_chrono = DateTime::<Local>::from(date);
    let today = Local::now();
    today.num_days_from_ce() == latest_update_chrono.num_days_from_ce()
}
