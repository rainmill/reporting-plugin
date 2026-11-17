//! Dashboards: rows of report widgets on a 20-column grid, read from YAML, each
//! widget asking a data source over the page's period.

use std::collections::HashMap;

use chrono::{NaiveDate, Utc};
use laterite_admin::html::Markup;
use laterite_admin::routes::RouteCtx;
use laterite_admin::Shell;
use laterite_core::Text;
use serde::Deserialize;

use crate::draw::{self, escape, Axis, Figure, Ring, Shown};
use crate::period::{Compare, Group, Preset};
use crate::{
    run, DataSource, DataSourceReg, Filter, FilterOp, Order, OrderBy, Report, ReportCx,
    ReportError, ReportQuery, Unit,
};

/// The grid's columns.
pub const COLUMNS: u8 = 20;

/// A dashboard a module ships, written in YAML.
#[derive(Clone)]
pub struct DashboardReg {
    /// `vendor.name`, unique across modules; the dashboard's address.
    pub code: &'static str,
    /// Its name.
    pub name: Text,
    /// A framework icon name.
    pub icon: &'static str,
    /// The permission an operator needs, beside `rainmill.reporting.view`.
    pub permission: Option<&'static str>,
    /// The definition: a `reports:` map of widgets.
    pub yaml: &'static str,
}

impl DashboardReg {
    /// A dashboard named `name` at `code`, defined by `yaml`.
    pub fn new(code: &'static str, name: impl Into<Text>, yaml: &'static str) -> Self {
        Self {
            code,
            name: name.into(),
            icon: "chart-bar",
            permission: None,
            yaml,
        }
    }

    /// Sets its icon.
    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = icon;
        self
    }

    /// Sets the permission it needs.
    pub fn permission(mut self, permission: &'static str) -> Self {
        self.permission = Some(permission);
        self
    }
}

/// A dashboard's definition.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    /// The widgets, by code, placed by their `row` and in written order.
    #[serde(default)]
    pub reports: Vec<(String, WidgetSpec)>,
}

impl Definition {
    /// Reads a definition. Widgets keep the order they are written in.
    pub fn parse(yaml: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            #[serde(default)]
            reports: laterite_core::serde_json::Map<String, laterite_core::serde_json::Value>,
        }
        let file: File = serde_saphyr::from_str(yaml).map_err(|e| e.to_string())?;
        let mut reports = Vec::new();
        for (code, value) in file.reports {
            let spec: WidgetSpec = laterite_core::serde_json::from_value(value)
                .map_err(|e| format!("report `{code}`: {e}"))?;
            spec.check().map_err(|e| format!("report `{code}`: {e}"))?;
            reports.push((code, spec));
        }
        Ok(Self { reports })
    }

    /// The widgets in rows, rows in order.
    pub fn rows(&self) -> Vec<Vec<&(String, WidgetSpec)>> {
        let mut rows: Vec<(u32, Vec<&(String, WidgetSpec)>)> = Vec::new();
        for item in &self.reports {
            let row = item.1.row.unwrap_or(u32::MAX);
            match rows.iter_mut().find(|(r, _)| *r == row) {
                Some((_, items)) => items.push(item),
                None => rows.push((row, vec![item])),
            }
        }
        rows.sort_by_key(|(r, _)| *r);
        rows.into_iter().map(|(_, items)| items).collect()
    }
}

/// What a widget draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A heading across the row.
    Section,
    /// One figure.
    Indicator,
    /// A chart.
    Chart,
    /// A table.
    Table,
    /// Each value as a share of the whole.
    Percentage,
    /// A paragraph of text.
    Notice,
}

/// How a chart widget draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    /// Lines over the date.
    #[default]
    Line,
    /// Bars side by side.
    Bar,
    /// Bars stacked into one.
    StackedBar,
    /// A bar per value, across.
    HorizontalBar,
    /// A ring with the total in the middle.
    Doughnut,
    /// A full disc.
    Pie,
}

/// A metric a widget shows, with its options.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum MetricSpec {
    /// Just the metric's code.
    Code(String),
    /// The metric with options.
    Full {
        /// The metric's code.
        metric: String,
        /// Its colour, `#rrggbb`.
        #[serde(default)]
        colour: Option<String>,
        /// Show its total in the legend or the footer.
        #[serde(default)]
        totals: bool,
        /// Draw a bar under each table cell.
        #[serde(default)]
        bar: bool,
    },
}

impl MetricSpec {
    fn code(&self) -> &str {
        match self {
            Self::Code(c) | Self::Full { metric: c, .. } => c,
        }
    }
}

/// A filter as written: one operation and a value or values.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterSpec {
    /// The comparison.
    pub op: FilterOp,
    /// The value.
    #[serde(default)]
    pub value: Option<String>,
    /// The values, for `one_of`.
    #[serde(default)]
    pub values: Vec<String>,
}

/// One widget as written.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetSpec {
    /// What it draws.
    #[serde(rename = "type")]
    pub kind: Kind,
    /// Its row; rows sort by number.
    #[serde(default)]
    pub row: Option<u32>,
    /// Columns of the 20 it spans.
    #[serde(default)]
    pub width: Option<u8>,
    /// Its heading.
    #[serde(default)]
    pub title: Option<String>,
    /// The data source's code.
    #[serde(default)]
    pub data_source: Option<String>,
    /// The dimension's code; `date` when not given.
    #[serde(default)]
    pub dimension: Option<String>,
    /// The metric, for an indicator or a percentage.
    #[serde(default)]
    pub metric: Option<String>,
    /// The metrics, for a chart or a table.
    #[serde(default)]
    pub metrics: Vec<MetricSpec>,
    /// How a chart draws.
    #[serde(default)]
    pub chart: ChartKind,
    /// The most rows.
    #[serde(default)]
    pub limit: Option<u32>,
    /// `dimension` or a metric code to sort by.
    #[serde(default)]
    pub sort: Option<String>,
    /// `asc` or `desc`.
    #[serde(default)]
    pub order: Option<String>,
    /// How dates group; suited to the period when not given.
    #[serde(default)]
    pub group: Option<Group>,
    /// Leave out empty and unset values.
    #[serde(default)]
    pub hide_empty: bool,
    /// Conditions on the dimension.
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    /// A target for an indicator.
    #[serde(default)]
    pub goal: Option<f64>,
    /// An icon for an indicator.
    #[serde(default)]
    pub icon: Option<String>,
    /// Where an indicator links.
    #[serde(default)]
    pub link: Option<String>,
    /// A falling value is good.
    #[serde(default)]
    pub lower_is_better: bool,
    /// Show the period beside a section's title.
    #[serde(default)]
    pub show_period: bool,
    /// A notice's text.
    #[serde(default)]
    pub text: Option<String>,
    /// A notice's tone.
    #[serde(default)]
    pub tone: Option<String>,
}

impl WidgetSpec {
    fn check(&self) -> Result<(), String> {
        let needs_source = !matches!(self.kind, Kind::Section | Kind::Notice);
        if needs_source && self.data_source.is_none() {
            return Err("needs a data_source".into());
        }
        match self.kind {
            Kind::Indicator | Kind::Percentage if self.metric.is_none() => {
                Err("needs a metric".into())
            }
            Kind::Chart | Kind::Table if self.metrics.is_empty() => Err("needs metrics".into()),
            Kind::Notice if self.text.is_none() => Err("needs text".into()),
            _ => match self.width {
                Some(w) if w == 0 || w > COLUMNS => Err(format!("width must be 1 to {COLUMNS}")),
                _ => Ok(()),
            },
        }
    }

    fn metric_codes(&self) -> Vec<&str> {
        match self.kind {
            Kind::Indicator | Kind::Percentage => self.metric.as_deref().into_iter().collect(),
            _ => self.metrics.iter().map(MetricSpec::code).collect(),
        }
    }
}

/// The period a dashboard shows, read from its query string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// The preset's code, or `custom`.
    pub range: String,
    /// The first day.
    pub from: NaiveDate,
    /// The last day.
    pub to: NaiveDate,
    /// The comparison.
    pub compare: Compare,
    /// Skip the cache.
    pub fresh: bool,
}

impl View {
    /// The view a query asks for: the last 30 days unless it says otherwise.
    pub fn from_query(query: &HashMap<String, String>, today: NaiveDate) -> Self {
        let date = |key: &str| {
            query
                .get(key)
                .and_then(|v| NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d").ok())
        };
        let compare = match query.get("compare").map(String::as_str) {
            Some("previous") => Compare::Previous,
            Some("year") => Compare::LastYear,
            _ => Compare::Off,
        };
        let fresh = query.get("refresh").is_some_and(|v| v == "1");
        if let (Some(from), Some(to)) = (date("from"), date("to")) {
            if from <= to && (to - from).num_days() <= 1830 {
                return Self {
                    range: "custom".into(),
                    from,
                    to: to.min(today),
                    compare,
                    fresh,
                };
            }
        }
        let code = query.get("range").map(String::as_str).unwrap_or("30d");
        let preset = Preset::parse(code).unwrap_or(Preset::Days(30));
        let code = Preset::ALL
            .iter()
            .find(|(p, _)| *p == preset)
            .map_or("30d", |(_, c)| c);
        let (from, to) = preset.span(today);
        Self {
            range: code.into(),
            from,
            to,
            compare,
            fresh,
        }
    }

    fn query_string(&self, compare: Compare) -> String {
        let compare = match compare {
            Compare::Off => "",
            Compare::Previous => "&compare=previous",
            Compare::LastYear => "&compare=year",
        };
        if self.range == "custom" {
            format!("?from={}&to={}{compare}", self.from, self.to)
        } else {
            format!("?range={}{compare}", self.range)
        }
    }

    /// The days shown, as text.
    pub fn label(&self) -> String {
        if self.from == self.to {
            self.from.format("%-d %b %Y").to_string()
        } else {
            format!(
                "{} – {}",
                self.from.format("%-d %b %Y"),
                self.to.format("%-d %b %Y")
            )
        }
    }
}

/// The bar above a dashboard: presets, a custom range, the comparison and
/// Refresh.
fn period_bar(shell: &Shell, view: &View) -> String {
    let presets: String = Preset::ALL
        .iter()
        .map(|(_, code)| {
            let label = match *code {
                "today" => shell.t("Today"),
                "7d" => shell.t("7 days"),
                "30d" => shell.t("30 days"),
                "90d" => shell.t("90 days"),
                "month" => shell.t("This month"),
                "last_month" => shell.t("Last month"),
                "quarter" => shell.t("This quarter"),
                _ => shell.t("This year"),
            };
            let compare = match view.compare {
                Compare::Off => "",
                Compare::Previous => "&compare=previous",
                Compare::LastYear => "&compare=year",
            };
            format!(
                r#"<a class="lat-period__preset{}" href="?range={code}{compare}">{}</a>"#,
                if view.range == *code { " is-on" } else { "" },
                escape(&label)
            )
        })
        .collect();
    let option = |value: &str, label: String, on: bool| {
        format!(
            r#"<option value="{value}"{}>{}</option>"#,
            if on { " selected" } else { "" },
            escape(&label)
        )
    };
    format!(
        r#"<div class="lat-period"><div class="lat-period__presets">{presets}</div><form class="lat-period__custom" method="get"><input class="lat-input" type="date" name="from" value="{from}" aria-label="{from_l}"><span>–</span><input class="lat-input" type="date" name="to" value="{to}" aria-label="{to_l}"><select class="lat-input" name="compare" aria-label="{cmp_l}">{off}{prev}{year}</select><button class="lat-btn lat-btn--secondary lat-btn--sm" type="submit">{apply}</button><a class="lat-btn lat-btn--ghost lat-btn--sm" href="{refresh}&refresh=1">{refresh_l}</a></form></div>"#,
        from = view.from,
        to = view.to,
        from_l = escape(&shell.t("From")),
        to_l = escape(&shell.t("To")),
        cmp_l = escape(&shell.t("Compare with")),
        off = option("", shell.t("No comparison"), view.compare == Compare::Off),
        prev = option(
            "previous",
            shell.t("Previous period"),
            view.compare == Compare::Previous
        ),
        year = option(
            "year",
            shell.t("Same period last year"),
            view.compare == Compare::LastYear
        ),
        apply = escape(&shell.t("Apply")),
        refresh = escape(&view.query_string(view.compare)),
        refresh_l = escape(&shell.t("Refresh")),
    )
}

/// Renders a dashboard's widgets over `view` for the signed-in operator.
pub async fn render(ctx: &RouteCtx, shell: &Shell, definition: &Definition, view: &View) -> Markup {
    let sources: Vec<&DataSourceReg> = ctx.contributions::<DataSourceReg>();
    let cx = ReportCx {
        db: ctx.db(),
        crypt: ctx.crypt(),
        cache: ctx.cache(),
    };
    let mut body = String::from(r#"<div class="rp-grid">"#);
    // A source that fails says why once, above the widgets.
    let mut failures: Vec<String> = Vec::new();
    for row in definition.rows() {
        for (code, spec) in row {
            let width = spec.width.unwrap_or(COLUMNS).min(COLUMNS);
            let source = spec.data_source.as_deref().map(|code| {
                sources
                    .iter()
                    .find(|r| r.source.code() == code)
                    .map(|r| r.source.as_ref())
            });
            let inner = match source {
                Some(None) => draw::failed(&shell.t("This widget's data source is not installed.")),
                Some(Some(s)) if s.permission().is_some_and(|p| !shell.allows(p)) => continue,
                Some(Some(s)) => match widget(shell, &cx, s, spec, view).await {
                    Ok(m) => m,
                    Err(ReportError::Source(reason)) => {
                        let line = format!("{}: {reason}", shell.tt(&s.label()));
                        if !failures.contains(&line) {
                            failures.push(line);
                        }
                        draw::unavailable(&shell.t("Not available."))
                    }
                    Err(e) => draw::failed(&e.to_string()),
                },
                None => static_widget(spec, view),
            };
            let title = match (spec.kind, &spec.title) {
                (Kind::Section | Kind::Indicator, _) | (_, None) => String::new(),
                (_, Some(t)) => format!(r#"<h3 class="rp-widget__title">{}</h3>"#, escape(t)),
            };
            let frame = if spec.kind == Kind::Section {
                "rp-widget rp-widget--bare"
            } else {
                "rp-widget"
            };
            body.push_str(&format!(
                r#"<section class="{frame}" id="report-{}" style="--span:{width}">{title}{inner}</section>"#,
                escape(code)
            ));
        }
    }
    body.push_str("</div>");
    let banner: String = failures
        .iter()
        .map(|f| draw::notice(f, "danger").into_string())
        .collect();
    let banner = if banner.is_empty() {
        banner
    } else {
        format!(r#"<div class="rp-failures">{banner}</div>"#)
    };
    Markup::raw_unchecked(format!("{}{banner}{body}", period_bar(shell, view)))
}

fn static_widget(spec: &WidgetSpec, view: &View) -> Markup {
    match spec.kind {
        Kind::Section => draw::section(
            spec.title.as_deref().unwrap_or_default(),
            spec.show_period.then(|| view.label()).as_deref(),
        ),
        _ => draw::notice(
            spec.text.as_deref().unwrap_or_default(),
            spec.tone.as_deref().unwrap_or("info"),
        ),
    }
}

/// The query a widget asks its source.
pub fn query_of(spec: &WidgetSpec, view: &View) -> ReportQuery {
    let metrics = spec.metric_codes();
    let dimension = spec.dimension.clone().unwrap_or_else(|| "date".into());
    let mut q = ReportQuery::new(dimension, &metrics, view.from, view.to);
    q.group = spec.group.unwrap_or_else(|| {
        if spec.kind == Kind::Indicator || spec.kind == Kind::Percentage {
            Group::Whole
        } else {
            Group::suits((view.to - view.from).num_days() + 1)
        }
    });
    q.limit = spec.limit;
    q.hide_empty = spec.hide_empty;
    q.totals = true;
    q.fresh = view.fresh;
    q.filters = spec
        .filters
        .iter()
        .map(|f| Filter {
            op: f.op,
            values: if f.values.is_empty() {
                f.value.clone().into_iter().collect()
            } else {
                f.values.clone()
            },
        })
        .collect();
    q.order = spec.sort.as_ref().map(|by| Order {
        by: if by == "dimension" {
            OrderBy::Dimension
        } else {
            OrderBy::Metric(by.clone())
        },
        descending: spec.order.as_deref() != Some("asc"),
    });
    if q.order.is_none() && q.dimension != "date" {
        if let Some(first) = metrics.first() {
            q.order = Some(Order {
                by: OrderBy::Metric(first.to_string()),
                descending: true,
            });
        }
    }
    q
}

async fn widget(
    shell: &Shell,
    cx: &ReportCx<'_>,
    source: &dyn DataSource,
    spec: &WidgetSpec,
    view: &View,
) -> Result<Markup, ReportError> {
    let query = query_of(spec, view);
    let compare = if spec.kind == Kind::Indicator {
        view.compare
    } else {
        Compare::Off
    };
    let report = match run(source, cx, &query, compare).await {
        Ok(r) => r,
        Err(e) => return Err(e),
    };
    let metrics = source.metrics();
    let dimensions = source.dimensions();
    let shown: Vec<Shown> = query
        .metrics
        .iter()
        .enumerate()
        .map(|(i, code)| {
            let other = source.other_metric(code);
            let meta = metrics.iter().find(|m| &m.code == code).or(other.as_ref());
            let opts = spec.metrics.get(i);
            let (colour, bar) = match opts {
                Some(MetricSpec::Full { colour, bar, .. }) => (colour.clone(), *bar),
                _ => (None, false),
            };
            Shown {
                label: meta.map_or_else(|| code.clone(), |m| shell.tt(&m.label)),
                unit: meta.map_or(Unit::Number, |m| m.unit.clone()),
                colour,
                relative: bar,
            }
        })
        .collect();
    let date = dimensions
        .iter()
        .any(|d| d.code == query.dimension && d.date);
    let other_dimension = source.other_dimension(&query.dimension);
    let labels: Vec<String> = report
        .current
        .rows
        .iter()
        .map(|r| {
            if date {
                draw::date_label(&r.key, query.group, query.to)
            } else if r.key.is_empty() {
                shell.t("(not set)")
            } else {
                r.key.clone()
            }
        })
        .collect();
    let column = |i: usize| -> Vec<Option<f64>> {
        report
            .current
            .rows
            .iter()
            .map(|r| r.values.get(i).copied().flatten())
            .collect()
    };
    let total = |i: usize| total_of(&report, i);
    Ok(match spec.kind {
        Kind::Indicator => {
            let figure = Figure {
                label: spec.title.as_deref().unwrap_or(&shown[0].label),
                unit: &shown[0].unit,
                value: total(0),
                previous: report.previous.as_ref().and_then(|p| sum_or_total(p, 0)),
                goal: spec.goal,
                icon: spec.icon.as_deref(),
                link: spec.link.as_deref(),
                lower_is_better: spec.lower_is_better,
            };
            draw::indicator(shell, &figure)
        }
        Kind::Percentage => draw::shares(shell, &labels, &shown[0], &column(0)),
        Kind::Table => {
            let rows: Vec<(String, Vec<Option<f64>>)> = labels
                .iter()
                .cloned()
                .zip(report.current.rows.iter().map(|r| r.values.clone()))
                .collect();
            let wants_totals = spec
                .metrics
                .iter()
                .any(|m| matches!(m, MetricSpec::Full { totals: true, .. }));
            let totals: Vec<Option<f64>> = (0..shown.len()).map(total).collect();
            let dimension = dimensions
                .iter()
                .chain(other_dimension.iter())
                .find(|d| d.code == query.dimension)
                .map_or_else(|| query.dimension.clone(), |d| shell.tt(&d.label));
            draw::table(
                shell,
                &dimension,
                &shown,
                &rows,
                wants_totals.then_some(totals.as_slice()),
            )
        }
        Kind::Chart => match spec.chart {
            ChartKind::Doughnut | ChartKind::Pie => {
                let ring = if spec.chart == ChartKind::Pie {
                    Ring::Pie
                } else {
                    Ring::Doughnut
                };
                draw::ring(shell, ring, &labels, &shown[0], &column(0), total(0))
            }
            ChartKind::HorizontalBar => draw::bars(shell, &labels, &shown[0], &column(0)),
            kind => {
                let axis = match kind {
                    ChartKind::Bar => Axis::Bar,
                    ChartKind::StackedBar => Axis::StackedBar,
                    _ => Axis::Line,
                };
                let totals: Vec<Option<f64>> = spec
                    .metrics
                    .iter()
                    .enumerate()
                    .map(|(i, m)| {
                        matches!(m, MetricSpec::Full { totals: true, .. })
                            .then(|| total(i))
                            .flatten()
                    })
                    .collect();
                let series: Vec<(Shown, Vec<Option<f64>>)> = shown
                    .into_iter()
                    .enumerate()
                    .map(|(i, s)| (s, column(i)))
                    .collect();
                draw::axis(shell, axis, &labels, &series, &totals)
            }
        },
        Kind::Section | Kind::Notice => static_widget(spec, view),
    })
}

/// A metric's total: the source's own when it gave one, else the rows' sum.
fn total_of(report: &Report, i: usize) -> Option<f64> {
    sum_or_total(&report.current, i)
}

fn sum_or_total(data: &crate::ReportData, i: usize) -> Option<f64> {
    if let Some(t) = data.totals.get(i).copied().flatten() {
        return Some(t);
    }
    let values: Vec<f64> = data
        .rows
        .iter()
        .filter_map(|r| r.values.get(i).copied().flatten())
        .collect();
    (!values.is_empty()).then(|| values.iter().sum())
}

/// Today, for counting presets back.
pub fn today() -> NaiveDate {
    Utc::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = r##"
reports:
  title:
    row: 1
    type: section
    title: Traffic
    show_period: true
  views:
    row: 2
    width: 5
    type: indicator
    metric: views
    data_source: test.stand
    goal: 1000
  pages:
    row: 3
    type: table
    dimension: page
    data_source: test.stand
    limit: 10
    metrics:
      - metric: views
        colour: "#84cc16"
        totals: true
        bar: true
  traffic:
    row: 2
    width: 15
    type: chart
    chart: line
    data_source: test.stand
    metrics: [views]
"##;

    #[test]
    fn a_definition_keeps_its_order_and_groups_rows() {
        let def = Definition::parse(YAML).unwrap();
        let rows: Vec<Vec<&str>> = def
            .rows()
            .iter()
            .map(|r| r.iter().map(|(c, _)| c.as_str()).collect())
            .collect();
        assert_eq!(
            rows,
            vec![vec!["title"], vec!["views", "traffic"], vec!["pages"]]
        );
    }

    #[test]
    fn a_widget_missing_what_it_needs_is_refused() {
        let err = Definition::parse("reports:\n  x:\n    type: indicator\n    data_source: a\n")
            .unwrap_err();
        assert_eq!(err, "report `x`: needs a metric");
        let err =
            Definition::parse("reports:\n  x:\n    type: chart\n    metrics: [a]\n").unwrap_err();
        assert_eq!(err, "report `x`: needs a data_source");
        assert!(Definition::parse(
            "reports:\n  x:\n    type: notice\n    text: hi\n    colour: red\n"
        )
        .is_err());
    }

    #[test]
    fn a_view_reads_presets_custom_days_and_the_comparison() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
        let q = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let v = View::from_query(&q(&[]), today);
        assert_eq!(
            (v.range.as_str(), v.from.to_string()),
            ("30d", "2026-10-19".into())
        );
        let v = View::from_query(&q(&[("range", "month"), ("compare", "year")]), today);
        assert_eq!(
            (v.from.to_string(), v.compare),
            ("2026-11-01".into(), Compare::LastYear)
        );
        let v = View::from_query(&q(&[("from", "2026-09-01"), ("to", "2026-12-31")]), today);
        assert_eq!((v.range.as_str(), v.to), ("custom", today));
        let v = View::from_query(&q(&[("range", "nonsense")]), today);
        assert_eq!(v.range, "30d");
    }

    #[test]
    fn a_widget_asks_a_whole_period_for_one_figure_and_ranks_a_breakdown() {
        let def = Definition::parse(YAML).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
        let view = View::from_query(&HashMap::new(), today);
        let views = &def.reports.iter().find(|(c, _)| c == "views").unwrap().1;
        assert_eq!(query_of(views, &view).group, Group::Whole);
        let pages = &def.reports.iter().find(|(c, _)| c == "pages").unwrap().1;
        let q = query_of(pages, &view);
        assert_eq!(q.limit, Some(10));
        assert_eq!(
            q.order,
            Some(Order {
                by: OrderBy::Metric("views".into()),
                descending: true
            })
        );
    }
}
