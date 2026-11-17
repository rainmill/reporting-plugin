//! Running a report: checking the query against the source, the cache, date
//! gaps and the comparison period.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::period::{fill, Compare};
use crate::{Aggregate, DataSource, ReportCx, ReportData, ReportError, ReportQuery};

/// A report and, when one was asked for, the same report over the comparison
/// period.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The period asked for.
    pub current: ReportData,
    /// The comparison period.
    pub previous: Option<ReportData>,
}

/// Answers `query` from `source`, and again over the `compare` period.
///
/// Unknown dimension or metric codes are refused before the source is asked.
/// Answers are kept for the source's [`DataSource::cache_for`] unless the query
/// is `fresh`. A date series comes back with every group of the period, in
/// date order.
pub async fn run(
    source: &dyn DataSource,
    cx: &ReportCx<'_>,
    query: &ReportQuery,
    compare: Compare,
) -> Result<Report, ReportError> {
    let (date, aggregates) = check(source, query)?;
    let current = answer(source, cx, query, date, &aggregates).await?;
    let previous = match compare.span(query.from, query.to) {
        Some((from, to)) => {
            let mut earlier = query.clone();
            earlier.from = from;
            earlier.to = to;
            Some(answer(source, cx, &earlier, date, &aggregates).await?)
        }
        None => None,
    };
    Ok(Report { current, previous })
}

/// Whether the dimension is the date, and each metric's aggregate.
fn check(
    source: &dyn DataSource,
    query: &ReportQuery,
) -> Result<(bool, Vec<Aggregate>), ReportError> {
    let unknown = |what, code: &str| ReportError::Unknown {
        source_code: source.code().to_string(),
        what,
        code: code.to_string(),
    };
    let dimension = source
        .dimensions()
        .into_iter()
        .find(|d| d.code == query.dimension)
        .or_else(|| source.other_dimension(&query.dimension))
        .ok_or_else(|| unknown("dimension", &query.dimension))?;
    let metrics = source.metrics();
    let aggregates = query
        .metrics
        .iter()
        .map(|code| {
            metrics
                .iter()
                .find(|m| &m.code == code)
                .map(|m| m.aggregate)
                .or_else(|| source.other_metric(code).map(|m| m.aggregate))
                .ok_or_else(|| unknown("metric", code))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if query.from > query.to {
        return Err(ReportError::Unsupported(
            "the period ends before it starts".into(),
        ));
    }
    Ok((dimension.date, aggregates))
}

async fn answer(
    source: &dyn DataSource,
    cx: &ReportCx<'_>,
    query: &ReportQuery,
    date: bool,
    aggregates: &[Aggregate],
) -> Result<ReportData, ReportError> {
    let mut data = cached(source, cx, query).await?;
    if date {
        let rows = std::mem::take(&mut data.rows);
        data.rows = fill(
            rows,
            aggregates,
            query.from,
            query.to,
            query.group,
            data.grouped,
        );
        data.grouped = true;
    }
    Ok(data)
}

async fn cached(
    source: &dyn DataSource,
    cx: &ReportCx<'_>,
    query: &ReportQuery,
) -> Result<ReportData, ReportError> {
    let Some(ttl) = source.cache_for().filter(|t| *t > Duration::ZERO) else {
        return source.fetch(cx, query).await;
    };
    let key = key(source.code(), query);
    if query.fresh {
        cx.cache.forget(&key).await;
    } else if let Some(hit) = cx.cache.get_json::<ReportData>(&key).await {
        return Ok(hit);
    }
    let data = source.fetch(cx, query).await?;
    // A failed write only means the next view asks again.
    let _ = cx.cache.put_json(&key, &data, &[], ttl).await;
    Ok(data)
}

/// The cache key: the source and a digest of the query, `fresh` aside.
fn key(source: &str, query: &ReportQuery) -> String {
    let json = laterite_core::serde_json::to_string(query).unwrap_or_default();
    let digest = Sha256::digest(json.as_bytes());
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("rainmill.reporting.{source}.{hex}")
}
