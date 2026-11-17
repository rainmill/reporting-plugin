//! Periods: the days a report covers, how the date is grouped within them, and
//! the period a report is compared with.

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, Months, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::{Aggregate, Row};

/// How the date dimension is grouped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// One row a day.
    #[default]
    Day,
    /// One row a week, weeks starting on Monday.
    Week,
    /// One row a calendar month.
    Month,
    /// One row a calendar quarter.
    Quarter,
    /// One row a calendar year.
    Year,
    /// One row for the whole period.
    Whole,
}

impl Group {
    /// The first day of the group holding `date`, within a period starting
    /// on `from`.
    pub fn start(self, date: NaiveDate, from: NaiveDate) -> NaiveDate {
        let first = match self {
            Group::Day => date,
            Group::Week => date - Duration::days(i64::from(date.weekday().num_days_from_monday())),
            Group::Month => day(date.year(), date.month(), 1),
            Group::Quarter => day(date.year(), (date.month0() / 3) * 3 + 1, 1),
            Group::Year => day(date.year(), 1, 1),
            Group::Whole => from,
        };
        first.max(from)
    }

    /// The first day of the group after the one starting on `start`.
    fn next(self, start: NaiveDate, to: NaiveDate) -> NaiveDate {
        let months = |n: u32| {
            let aligned = match self {
                Group::Month => day(start.year(), start.month(), 1),
                Group::Quarter => day(start.year(), (start.month0() / 3) * 3 + 1, 1),
                _ => day(start.year(), 1, 1),
            };
            aligned + Months::new(n)
        };
        match self {
            Group::Day => start + Duration::days(1),
            Group::Week => {
                start - Duration::days(i64::from(start.weekday().num_days_from_monday()))
                    + Duration::days(7)
            }
            Group::Month => months(1),
            Group::Quarter => months(3),
            Group::Year => months(12),
            Group::Whole => to + Duration::days(1),
        }
    }

    /// The first day of every group from `from` to `to`.
    pub fn starts(self, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        let mut out = Vec::new();
        let mut at = self.start(from, from);
        while at <= to {
            out.push(at);
            at = self.next(at, to);
        }
        out
    }

    /// The group that suits a period of `days`: days up to three months,
    /// weeks up to a year, months beyond.
    pub fn suits(days: i64) -> Self {
        match days {
            ..=92 => Group::Day,
            93..=366 => Group::Week,
            _ => Group::Month,
        }
    }
}

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap_or(NaiveDate::MIN)
}

/// A named span of days, counted back from today.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Today alone.
    Today,
    /// The last `n` days, today included.
    Days(u32),
    /// From the first of this month.
    ThisMonth,
    /// The whole of last month.
    LastMonth,
    /// From the first day of this quarter.
    ThisQuarter,
    /// From the first of January.
    ThisYear,
}

impl Preset {
    /// The presets a period bar offers, with their query codes.
    pub const ALL: &'static [(Preset, &'static str)] = &[
        (Preset::Today, "today"),
        (Preset::Days(7), "7d"),
        (Preset::Days(30), "30d"),
        (Preset::Days(90), "90d"),
        (Preset::ThisMonth, "month"),
        (Preset::LastMonth, "last_month"),
        (Preset::ThisQuarter, "quarter"),
        (Preset::ThisYear, "year"),
    ];

    /// The preset a query code names.
    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, c)| *c == code).map(|(p, _)| *p)
    }

    /// Its first and last day, counted from `today`.
    pub fn span(self, today: NaiveDate) -> (NaiveDate, NaiveDate) {
        match self {
            Preset::Today => (today, today),
            Preset::Days(n) => (today - Duration::days(i64::from(n.max(1)) - 1), today),
            Preset::ThisMonth => (Group::Month.start(today, NaiveDate::MIN), today),
            Preset::LastMonth => {
                let this = Group::Month.start(today, NaiveDate::MIN);
                let last = this - Duration::days(1);
                (Group::Month.start(last, NaiveDate::MIN), last)
            }
            Preset::ThisQuarter => (Group::Quarter.start(today, NaiveDate::MIN), today),
            Preset::ThisYear => (Group::Year.start(today, NaiveDate::MIN), today),
        }
    }
}

/// The period a report is compared with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compare {
    /// No comparison.
    #[default]
    Off,
    /// The same number of days just before.
    Previous,
    /// The same days a year earlier.
    LastYear,
}

impl Compare {
    /// The comparison's first and last day, or `None` when off.
    pub fn span(self, from: NaiveDate, to: NaiveDate) -> Option<(NaiveDate, NaiveDate)> {
        match self {
            Compare::Off => None,
            Compare::Previous => {
                let days = (to - from).num_days() + 1;
                let end = from - Duration::days(1);
                Some((end - Duration::days(days - 1), end))
            }
            Compare::LastYear => Some((year_back(from), year_back(to))),
        }
    }
}

/// The same day a year earlier; 29 February becomes the 28th.
fn year_back(date: NaiveDate) -> NaiveDate {
    date.checked_sub_months(Months::new(12)).unwrap_or(date)
}

/// Folds date rows into the query's groups unless the source already grouped
/// them, then gives every group in the period a row: summed metrics read zero
/// where nothing was recorded, the rest read `None`. Rows come back in date
/// order; a key that is not a date is dropped.
pub fn fill(
    rows: Vec<Row>,
    aggregates: &[Aggregate],
    from: NaiveDate,
    to: NaiveDate,
    group: Group,
    grouped: bool,
) -> Vec<Row> {
    // Per group start: a running total and a count per metric.
    let mut folded: BTreeMap<NaiveDate, Vec<(f64, u32)>> = BTreeMap::new();
    for row in rows {
        let Ok(date) = NaiveDate::parse_from_str(&row.key, "%Y-%m-%d") else {
            continue;
        };
        if date < from || date > to {
            continue;
        }
        let start = if grouped {
            date
        } else {
            group.start(date, from)
        };
        let slot = folded
            .entry(start)
            .or_insert_with(|| vec![(0.0, 0); aggregates.len()]);
        for (i, value) in row.values.iter().enumerate().take(aggregates.len()) {
            if let Some(v) = value {
                slot[i].0 += v;
                slot[i].1 += 1;
            }
        }
    }
    group
        .starts(from, to)
        .into_iter()
        .map(|start| {
            let slot = folded.remove(&start);
            let values = aggregates
                .iter()
                .enumerate()
                .map(
                    |(i, aggregate)| match (slot.as_ref().map(|s| s[i]), aggregate) {
                        (Some((_, 0)) | None, Aggregate::Sum | Aggregate::CountDistinct) => {
                            Some(0.0)
                        }
                        (Some((_, 0)) | None, Aggregate::Average) => None,
                        (Some((total, _)), Aggregate::Sum | Aggregate::CountDistinct) => {
                            Some(total)
                        }
                        (Some((total, n)), Aggregate::Average) => Some(total / f64::from(n)),
                    },
                )
                .collect();
            Row {
                key: start.format("%Y-%m-%d").to_string(),
                values,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn keys(dates: Vec<NaiveDate>) -> Vec<String> {
        dates.iter().map(|d| d.to_string()).collect()
    }

    #[test]
    fn groups_start_where_the_calendar_does_and_never_before_the_period() {
        let from = d("2026-02-18");
        assert_eq!(Group::Week.start(d("2026-02-22"), from), d("2026-02-18"));
        assert_eq!(Group::Week.start(d("2026-02-26"), from), d("2026-02-23"));
        assert_eq!(
            Group::Quarter.start(d("2026-05-14"), NaiveDate::MIN),
            d("2026-04-01")
        );
        assert_eq!(
            Group::Year.start(d("2026-05-14"), NaiveDate::MIN),
            d("2026-01-01")
        );
        assert_eq!(Group::Whole.start(d("2026-05-14"), from), from);
    }

    #[test]
    fn every_group_in_a_period_is_listed_once() {
        assert_eq!(
            keys(Group::Week.starts(d("2026-02-18"), d("2026-03-04"))),
            ["2026-02-18", "2026-02-23", "2026-03-02"]
        );
        assert_eq!(
            keys(Group::Month.starts(d("2026-01-31"), d("2026-04-01"))),
            ["2026-01-31", "2026-02-01", "2026-03-01", "2026-04-01"]
        );
        assert_eq!(
            keys(Group::Quarter.starts(d("2026-02-10"), d("2026-11-17"))),
            ["2026-02-10", "2026-04-01", "2026-07-01", "2026-10-01"]
        );
        assert_eq!(
            keys(Group::Whole.starts(d("2026-02-10"), d("2026-11-17"))),
            ["2026-02-10"]
        );
        assert_eq!(Group::Day.starts(d("2026-02-10"), d("2026-02-12")).len(), 3);
    }

    #[test]
    fn presets_count_back_from_today() {
        let today = d("2026-11-17");
        assert_eq!(Preset::Days(7).span(today), (d("2026-11-11"), today));
        assert_eq!(
            Preset::LastMonth.span(today),
            (d("2026-10-01"), d("2026-10-31"))
        );
        assert_eq!(Preset::ThisQuarter.span(today), (d("2026-10-01"), today));
        assert_eq!(Preset::ThisYear.span(today), (d("2026-01-01"), today));
        assert_eq!(Preset::parse("last_month"), Some(Preset::LastMonth));
    }

    #[test]
    fn a_comparison_is_the_days_before_or_the_year_before() {
        let (from, to) = (d("2026-11-11"), d("2026-11-17"));
        assert_eq!(
            Compare::Previous.span(from, to),
            Some((d("2026-11-04"), d("2026-11-10")))
        );
        assert_eq!(
            Compare::LastYear.span(from, to),
            Some((d("2025-11-11"), d("2025-11-17")))
        );
        assert_eq!(
            Compare::LastYear.span(d("2028-02-29"), d("2028-03-01")),
            Some((d("2027-02-28"), d("2027-03-01")))
        );
        assert_eq!(Compare::Off.span(from, to), None);
    }

    #[test]
    fn fill_folds_days_into_groups_and_gives_empty_groups_a_row() {
        let rows = vec![
            Row {
                key: "2026-03-02".into(),
                values: vec![Some(3.0), Some(0.5)],
            },
            Row {
                key: "2026-03-04".into(),
                values: vec![Some(2.0), Some(0.25)],
            },
            Row {
                key: "2026-03-17".into(),
                values: vec![Some(1.0), None],
            },
            Row {
                key: "(other)".into(),
                values: vec![Some(9.0), Some(1.0)],
            },
        ];
        let out = fill(
            rows,
            &[Aggregate::Sum, Aggregate::Average],
            d("2026-03-02"),
            d("2026-03-22"),
            Group::Week,
            false,
        );
        assert_eq!(out.len(), 3);
        assert_eq!(
            out[0],
            Row {
                key: "2026-03-02".into(),
                values: vec![Some(5.0), Some(0.375)]
            }
        );
        assert_eq!(
            out[1],
            Row {
                key: "2026-03-09".into(),
                values: vec![Some(0.0), None]
            }
        );
        assert_eq!(
            out[2],
            Row {
                key: "2026-03-16".into(),
                values: vec![Some(1.0), None]
            }
        );
    }

    #[test]
    fn fill_keeps_a_source_s_own_groups() {
        let rows = vec![Row {
            key: "2026-04-01".into(),
            values: vec![Some(7.0)],
        }];
        let out = fill(
            rows,
            &[Aggregate::CountDistinct],
            d("2026-02-10"),
            d("2026-06-30"),
            Group::Quarter,
            true,
        );
        assert_eq!(keys_of(&out), ["2026-02-10", "2026-04-01"]);
        assert_eq!(out[1].values, vec![Some(7.0)]);
    }

    fn keys_of(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.key.as_str()).collect()
    }
}
