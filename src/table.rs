//! A data source over one of the application's tables: rows counted, or a
//! column summed, by day or by a column's value.

use laterite_core::query::{bind_values, build, text_cast};
use laterite_core::sea_query::{
    self, Alias, Expr, Func, Order as SqlOrder, Query, SelectStatement, SimpleExpr,
};
use laterite_core::{sqlx, AnyRowExt, Db, DbBackend, Text};

use crate::{
    Dimension, FilterOp, Metric, OrderBy, ReportCx, ReportData, ReportError, ReportQuery, Row, Unit,
};

/// What a table metric computes.
#[derive(Clone, Debug)]
enum Measure {
    Count,
    Sum(String),
}

/// A data source over a table whose `date_column` holds UTC text timestamps
/// (`2026-11-17T09:30:00Z`) or dates, as Laterite stores time.
///
/// ```ignore
/// TableSource::new("acme.signups", t!("Sign-ups"), "signup", "created_at")
///     .dimension("city", t!("City"), "city")
///     .count("signups", t!("Sign-ups"))
///     .sum("amount", t!("Amount"), "amount_minor", Unit::Money("INR".into()))
/// ```
#[derive(Clone, Debug)]
pub struct TableSource {
    code: String,
    label: Text,
    permission: Option<String>,
    table: String,
    date_column: String,
    dimensions: Vec<Dimension>,
    metrics: Vec<(Metric, Measure)>,
    scope: Vec<(String, String)>,
}

impl TableSource {
    /// A source named `code` over `table`, dated by `date_column`.
    pub fn new(
        code: impl Into<String>,
        label: impl Into<Text>,
        table: impl Into<String>,
        date_column: impl Into<String>,
    ) -> Self {
        let date_column = date_column.into();
        Self {
            code: code.into(),
            label: label.into(),
            permission: None,
            table: table.into(),
            dimensions: vec![Dimension::date(date_column.clone())],
            date_column,
            metrics: Vec::new(),
            scope: Vec::new(),
        }
    }

    /// Sets the permission an operator needs to read it.
    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.permission = Some(permission.into());
        self
    }

    /// Breaks reports down by `column`'s value.
    pub fn dimension(
        mut self,
        code: impl Into<String>,
        label: impl Into<Text>,
        column: impl Into<String>,
    ) -> Self {
        self.dimensions.push(Dimension::new(code, label, column));
        self
    }

    /// Counts rows.
    pub fn count(mut self, code: impl Into<String>, label: impl Into<Text>) -> Self {
        let metric = Metric::new(code, label, "*");
        self.metrics.push((metric, Measure::Count));
        self
    }

    /// Sums `column`.
    pub fn sum(
        mut self,
        code: impl Into<String>,
        label: impl Into<Text>,
        column: impl Into<String>,
        unit: Unit,
    ) -> Self {
        let column = column.into();
        let metric = Metric::new(code, label, column.clone()).unit(unit);
        self.metrics.push((metric, Measure::Sum(column)));
        self
    }

    /// Only rows whose `column` equals `value`, always; each call adds a
    /// condition.
    pub fn only(mut self, column: impl Into<String>, value: impl Into<String>) -> Self {
        self.scope.push((column.into(), value.into()));
        self
    }

    fn column(&self, code: &str) -> Option<&Dimension> {
        self.dimensions.iter().find(|d| d.code == code)
    }

    fn as_text(backend: DbBackend, expr: SimpleExpr) -> SimpleExpr {
        expr.cast_as(Alias::new(text_cast(backend)))
    }

    /// The day a row falls on: the first ten characters of the date column.
    fn day(&self, backend: DbBackend) -> SimpleExpr {
        Func::cust(Alias::new("SUBSTR"))
            .arg(Self::as_text(
                backend,
                Expr::col(Alias::new(&self.date_column)).into(),
            ))
            .arg(1)
            .arg(10)
            .into()
    }

    /// The statement's FROM, period, scope and filters.
    fn base(&self, backend: DbBackend, query: &ReportQuery, key: &SimpleExpr) -> SelectStatement {
        let mut stmt = Query::select();
        stmt.from(Alias::new(&self.table))
            .and_where(Expr::expr(self.day(backend)).gte(query.from.to_string()))
            .and_where(Expr::expr(self.day(backend)).lte(query.to.to_string()));
        for (column, value) in &self.scope {
            stmt.and_where(Expr::col(Alias::new(column)).eq(value.as_str()));
        }
        let lower = || Expr::expr(Func::lower(key.clone()));
        for filter in &query.filters {
            let first = filter
                .values
                .first()
                .map(|v| v.to_lowercase())
                .unwrap_or_default();
            let cond = match filter.op {
                FilterOp::Equals => lower().eq(first),
                FilterOp::StartsWith => lower().like(format!("{}%", escape_like(&first))),
                FilterOp::Contains => lower().like(format!("%{}%", escape_like(&first))),
                FilterOp::OneOf => lower().is_in(filter.values.iter().map(|v| v.to_lowercase())),
            };
            stmt.and_where(cond);
        }
        if query.hide_empty {
            stmt.and_where(Expr::expr(key.clone()).is_not_null())
                .and_where(Expr::expr(key.clone()).ne(""));
        }
        stmt
    }

    fn measure(&self, code: &str) -> Option<SimpleExpr> {
        self.metrics
            .iter()
            .find(|(m, _)| m.code == code)
            .map(|(_, measure)| match measure {
                Measure::Count => Expr::cust("COUNT(*)"),
                Measure::Sum(column) => Func::sum(Expr::col(Alias::new(column))).into(),
            })
    }

    fn measures(&self, backend: DbBackend, query: &ReportQuery, stmt: &mut SelectStatement) {
        for (i, code) in query.metrics.iter().enumerate() {
            if let Some(value) = self.measure(code) {
                stmt.expr_as(Self::as_text(backend, value), Alias::new(format!("m{i}")));
            }
        }
    }

    /// The statements a query needs, rendered to SQL before anything is awaited
    /// (sea-query's builders cannot cross an await): the grouped rows, the
    /// count of groups for a page, and the totals.
    fn plan(&self, backend: DbBackend, query: &ReportQuery) -> Result<Plan, ReportError> {
        let dimension = self.column(&query.dimension).ok_or_else(|| {
            ReportError::Unsupported(format!("no dimension `{}`", query.dimension))
        })?;
        let key = if dimension.date {
            self.day(backend)
        } else {
            Self::as_text(backend, Expr::col(Alias::new(&dimension.field)).into())
        };
        let mut grouped = self.base(backend, query, &key);
        grouped.expr_as(key.clone(), Alias::new("k"));
        self.measures(backend, query, &mut grouped);
        // By the output name: Postgres compares expressions with their bound
        // parameters, so a repeated SUBSTR would not match the selected one.
        let named: SimpleExpr = Expr::col(Alias::new("k")).into();
        grouped.add_group_by([named.clone()]);
        let direction = |descending: bool| {
            if descending {
                SqlOrder::Desc
            } else {
                SqlOrder::Asc
            }
        };
        match &query.order {
            Some(crate::Order {
                by: OrderBy::Metric(code),
                descending,
            }) => {
                // Sort the number, not its text.
                if let Some(value) = self.measure(code) {
                    grouped.order_by_expr(value, direction(*descending));
                }
            }
            Some(crate::Order { descending, .. }) => {
                grouped.order_by_expr(named.clone(), direction(*descending));
            }
            None => {
                grouped.order_by_expr(named.clone(), SqlOrder::Asc);
            }
        }
        let mut count = None;
        if let Some(page) = query.page {
            let mut counted = self.base(backend, query, &key);
            counted.expr_as(
                Self::as_text(backend, Func::count_distinct(key.clone()).into()),
                Alias::new("m0"),
            );
            count = Some(build(backend, counted));
            grouped
                .limit(u64::from(page.per_page))
                .offset(page.offset());
        } else if let Some(limit) = query.limit {
            grouped.limit(u64::from(limit));
        }
        let totals = query.totals.then(|| {
            let mut whole = self.base(backend, query, &key);
            self.measures(backend, query, &mut whole);
            build(backend, whole)
        });
        Ok(Plan {
            grouped: build(backend, grouped),
            count,
            totals,
        })
    }

    async fn rows(
        db: &Db,
        (sql, values): (String, sea_query::Values),
        count: usize,
        key: bool,
    ) -> Result<Vec<Row>, ReportError> {
        let rows = bind_values(sqlx::query(&sql), values)
            .fetch_all(&db.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let values = (0..count)
                .map(|i| {
                    row.get_text_opt(&format!("m{i}"))
                        .ok()
                        .flatten()
                        .and_then(|v| v.trim().parse::<f64>().ok())
                })
                .collect();
            let key = if key {
                row.get_text_opt("k")?.unwrap_or_default()
            } else {
                String::new()
            };
            out.push(Row { key, values });
        }
        Ok(out)
    }
}

/// A query's statements, as SQL and values.
struct Plan {
    grouped: (String, sea_query::Values),
    count: Option<(String, sea_query::Values)>,
    totals: Option<(String, sea_query::Values)>,
}

/// Escapes `%`, `_` and `\` in a LIKE pattern.
fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[laterite_core::strata::async_trait]
impl crate::DataSource for TableSource {
    fn code(&self) -> &str {
        &self.code
    }
    fn label(&self) -> Text {
        self.label.clone()
    }
    fn permission(&self) -> Option<&str> {
        self.permission.as_deref()
    }
    fn dimensions(&self) -> Vec<Dimension> {
        self.dimensions.clone()
    }
    fn metrics(&self) -> Vec<Metric> {
        self.metrics.iter().map(|(m, _)| m.clone()).collect()
    }
    async fn fetch(
        &self,
        cx: &ReportCx<'_>,
        query: &ReportQuery,
    ) -> Result<ReportData, ReportError> {
        let plan = self.plan(cx.db.backend, query)?;
        let n = query.metrics.len();
        let total_rows = match plan.count {
            Some(sql) => Self::rows(cx.db, sql, 1, false)
                .await?
                .first()
                .and_then(|r| r.values[0])
                .map(|v| v as u64),
            None => None,
        };
        let rows = Self::rows(cx.db, plan.grouped, n, true).await?;
        let totals = match plan.totals {
            Some(sql) => Self::rows(cx.db, sql, n, false)
                .await?
                .pop()
                .map(|r| r.values)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        Ok(ReportData {
            rows,
            totals,
            total_rows,
            grouped: false,
        })
    }
}
