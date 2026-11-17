//! The data source contract: what a source offers and the one question shape
//! it answers.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use laterite_core::{Cache, Crypt, Db, Text};
use serde::{Deserialize, Serialize};

/// How values of a metric combine across rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregate {
    /// Added together: views, sessions, sign-ups.
    Sum,
    /// Counted once however often they recur: users. Only the source can
    /// combine these, so a source offering one groups dates itself.
    CountDistinct,
    /// Averaged: rates and durations. Combined by the engine as the mean of
    /// the points it folds, so a source that can should group dates itself.
    Average,
}

/// How a metric's value reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// A count or a plain number.
    Number,
    /// A fraction shown as a percentage: `0.25` reads 25%.
    Percent,
    /// Seconds shown as minutes and seconds.
    Duration,
    /// An amount in minor units of the ISO currency.
    Money(String),
}

/// Something a report is broken down by.
#[derive(Clone, Debug)]
pub struct Dimension {
    /// Our code for it, `page_path`.
    pub code: String,
    /// Its name for an operator.
    pub label: Text,
    /// The source's own name for it: a column, or the provider's field.
    pub field: String,
    /// Whether it is the date.
    pub date: bool,
}

impl Dimension {
    /// A dimension named `code`, read from the source's `field`.
    pub fn new(code: impl Into<String>, label: impl Into<Text>, field: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            label: label.into(),
            field: field.into(),
            date: false,
        }
    }

    /// The date dimension every source offers, read from `field`.
    pub fn date(field: impl Into<String>) -> Self {
        Self {
            code: "date".into(),
            label: laterite_core::t!("Date"),
            field: field.into(),
            date: true,
        }
    }
}

/// Something a report counts or measures.
#[derive(Clone, Debug)]
pub struct Metric {
    /// Our code for it, `views`.
    pub code: String,
    /// Its name for an operator.
    pub label: Text,
    /// The source's own name for it.
    pub field: String,
    /// How its values combine.
    pub aggregate: Aggregate,
    /// How its value reads.
    pub unit: Unit,
}

impl Metric {
    /// A summed number named `code`, read from the source's `field`.
    pub fn new(code: impl Into<String>, label: impl Into<Text>, field: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            label: label.into(),
            field: field.into(),
            aggregate: Aggregate::Sum,
            unit: Unit::Number,
        }
    }

    /// Sets how its values combine.
    pub fn aggregate(mut self, aggregate: Aggregate) -> Self {
        self.aggregate = aggregate;
        self
    }

    /// Sets how its value reads.
    pub fn unit(mut self, unit: Unit) -> Self {
        self.unit = unit;
        self
    }
}

/// How a filter compares the dimension's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    /// The value is the first given one.
    Equals,
    /// The value begins with the first given one.
    StartsWith,
    /// The value contains the first given one.
    Contains,
    /// The value is one of those given.
    OneOf,
}

/// A condition on the dimension's value. Values compare as text, ignoring case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// The comparison.
    pub op: FilterOp,
    /// The value, or the values for [`FilterOp::OneOf`].
    pub values: Vec<String>,
}

/// What rows are sorted by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderBy {
    /// The dimension's value; dates sort as dates.
    Dimension,
    /// A metric among those asked for, by code.
    Metric(String),
}

/// A sort.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    /// What is sorted by.
    pub by: OrderBy,
    /// Largest first.
    pub descending: bool,
}

/// One page of rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// Rows on a page.
    pub per_page: u32,
    /// The page, from 1.
    pub page: u32,
}

impl Page {
    /// Rows skipped before this page.
    pub fn offset(&self) -> u64 {
        u64::from(self.page.max(1) - 1) * u64::from(self.per_page)
    }
}

/// The question a source answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportQuery {
    /// The dimension's code.
    pub dimension: String,
    /// The metrics' codes, in the order the row values come back.
    pub metrics: Vec<String>,
    /// The first day, included.
    pub from: NaiveDate,
    /// The last day, included.
    pub to: NaiveDate,
    /// How the date is grouped; ignored for other dimensions.
    pub group: crate::Group,
    /// Conditions on the dimension, all of which must hold.
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// The sort; the source's own when `None`.
    #[serde(default)]
    pub order: Option<Order>,
    /// The most rows wanted.
    #[serde(default)]
    pub limit: Option<u32>,
    /// One page of rows, with the total count; wins over `limit`.
    #[serde(default)]
    pub page: Option<Page>,
    /// Leave out rows whose dimension is empty or not set.
    #[serde(default)]
    pub hide_empty: bool,
    /// Ask the source for totals of every metric.
    #[serde(default)]
    pub totals: bool,
    /// Skip the cache and ask the source again.
    #[serde(default, skip_serializing)]
    pub fresh: bool,
}

impl ReportQuery {
    /// A query for `metrics` by `dimension` over the given days.
    pub fn new(
        dimension: impl Into<String>,
        metrics: &[&str],
        from: NaiveDate,
        to: NaiveDate,
    ) -> Self {
        Self {
            dimension: dimension.into(),
            metrics: metrics.iter().map(|m| m.to_string()).collect(),
            from,
            to,
            group: crate::Group::Day,
            filters: Vec::new(),
            order: None,
            limit: None,
            page: None,
            hide_empty: false,
            totals: false,
            fresh: false,
        }
    }
}

/// One row: the dimension's value and a value per metric asked for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// The dimension's value. For the date, the first day of the group as
    /// `YYYY-MM-DD`.
    pub key: String,
    /// A value per metric, in the query's order; `None` when the source had
    /// none.
    pub values: Vec<Option<f64>>,
}

/// What a source answers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReportData {
    /// The rows.
    pub rows: Vec<Row>,
    /// A total per metric when the query asked for totals.
    #[serde(default)]
    pub totals: Vec<Option<f64>>,
    /// Every matching row, when the query asked for a page.
    #[serde(default)]
    pub total_rows: Option<u64>,
    /// Set when the source already grouped the date as asked, so the engine
    /// only fills gaps.
    #[serde(default)]
    pub grouped: bool,
}

/// Why a report could not be made.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    /// The query names a dimension or metric the source does not offer.
    #[error("{source_code} has no {what} `{code}`")]
    Unknown {
        /// The source asked.
        source_code: String,
        /// `dimension` or `metric`.
        what: &'static str,
        /// The code asked for.
        code: String,
    },
    /// The source cannot answer a query of this shape.
    #[error("{0}")]
    Unsupported(String),
    /// The source failed: a provider's message, a refused credential.
    #[error("{0}")]
    Source(String),
    /// The database failed.
    #[error("the database failed: {0}")]
    Store(#[from] laterite_core::sqlx::Error),
}

/// What a source reads from while answering.
pub struct ReportCx<'a> {
    /// The application's database.
    pub db: &'a Db,
    /// The application's keys, for a credential in the vault.
    pub crypt: &'a Crypt,
    /// The shared cache.
    pub cache: &'a Cache,
}

/// A source of reports.
#[laterite_core::strata::async_trait]
pub trait DataSource: Send + Sync {
    /// `vendor.name`, unique across modules.
    fn code(&self) -> &str;
    /// Its name for an operator.
    fn label(&self) -> Text;
    /// The permission an operator needs to read it; `None` lets every operator.
    fn permission(&self) -> Option<&str> {
        None
    }
    /// What reports break down by; includes the date.
    fn dimensions(&self) -> Vec<Dimension>;
    /// What reports count or measure.
    fn metrics(&self) -> Vec<Metric>;
    /// A dimension outside [`dimensions`](Self::dimensions) that the source
    /// still accepts by code, such as a provider's own field typed by name.
    fn other_dimension(&self, _code: &str) -> Option<Dimension> {
        None
    }
    /// A metric outside [`metrics`](Self::metrics) that the source still
    /// accepts by code.
    fn other_metric(&self, _code: &str) -> Option<Metric> {
        None
    }
    /// How long an answer is kept; `None` asks every time.
    fn cache_for(&self) -> Option<Duration> {
        None
    }
    /// Answers `query`, whose dimension and metrics the engine has checked.
    async fn fetch(
        &self,
        cx: &ReportCx<'_>,
        query: &ReportQuery,
    ) -> Result<ReportData, ReportError>;
}

/// A data source contributed through the registry.
#[derive(Clone)]
pub struct DataSourceReg {
    /// The source.
    pub source: Arc<dyn DataSource>,
}

impl DataSourceReg {
    /// Registers `source`.
    pub fn new(source: Arc<dyn DataSource>) -> Self {
        Self { source }
    }
}
