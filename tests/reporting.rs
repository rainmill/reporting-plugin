//! The engine against a stand-in source: checks, cache, date fill and the
//! comparison period.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::NaiveDate;
use laterite_core::testing::connect_test;
use laterite_core::{t, Cache, Crypt, Text};
use rainmill_reporting::{
    run, Aggregate, Compare, DataSource, Dimension, Group, Metric, ReportCx, ReportData,
    ReportError, ReportQuery, Row,
};

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// Answers one row a day with the day of the month as views, counting calls.
struct Stand {
    calls: AtomicUsize,
    cache: Option<Duration>,
}

#[laterite_core::strata::async_trait]
impl DataSource for Stand {
    fn code(&self) -> &str {
        "test.stand"
    }
    fn label(&self) -> Text {
        t!("Stand")
    }
    fn dimensions(&self) -> Vec<Dimension> {
        vec![
            Dimension::date("day"),
            Dimension::new("page", t!("Page"), "path"),
        ]
    }
    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("views", t!("Views"), "views"),
            Metric::new("rate", t!("Rate"), "rate").aggregate(Aggregate::Average),
        ]
    }
    fn cache_for(&self) -> Option<Duration> {
        self.cache
    }
    async fn fetch(
        &self,
        _cx: &ReportCx<'_>,
        query: &ReportQuery,
    ) -> Result<ReportData, ReportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if query.dimension == "page" {
            return Ok(ReportData {
                rows: vec![Row {
                    key: "/".into(),
                    values: vec![Some(4.0)],
                }],
                ..Default::default()
            });
        }
        let mut rows = Vec::new();
        let mut day = query.from;
        while day <= query.to {
            // Skip the 5th, so a gap shows.
            if day.format("%d").to_string() != "05" {
                let n: f64 = day.format("%d").to_string().parse().unwrap();
                rows.push(Row {
                    key: day.to_string(),
                    values: query
                        .metrics
                        .iter()
                        .map(|m| Some(if m == "views" { n } else { 0.5 }))
                        .collect(),
                });
            }
            day = day.succ_opt().unwrap();
        }
        Ok(ReportData {
            rows,
            ..Default::default()
        })
    }
}

fn stand(cache: Option<Duration>) -> Stand {
    Stand {
        calls: AtomicUsize::new(0),
        cache,
    }
}

#[tokio::test]
async fn a_date_report_has_every_day_and_its_comparison() {
    let (db, _guard) = connect_test(&[]).await;
    let crypt = Crypt::ephemeral();
    let cache = Cache::local();
    let cx = ReportCx {
        db: &db,
        crypt: &crypt,
        cache: &cache,
    };
    let source = stand(None);
    let query = ReportQuery::new("date", &["views", "rate"], d("2026-03-03"), d("2026-03-07"));
    let report = run(&source, &cx, &query, Compare::Previous).await.unwrap();
    let days: Vec<_> = report.current.rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(
        days,
        [
            "2026-03-03",
            "2026-03-04",
            "2026-03-05",
            "2026-03-06",
            "2026-03-07"
        ]
    );
    assert_eq!(
        report.current.rows[2].values,
        vec![Some(0.0), None],
        "the missing day"
    );
    let earlier = report.previous.unwrap();
    assert_eq!(earlier.rows.first().unwrap().key, "2026-02-26");
    assert_eq!(earlier.rows.len(), 5);

    let mut weekly = query.clone();
    weekly.group = Group::Week;
    let report = run(&source, &cx, &weekly, Compare::Off).await.unwrap();
    assert_eq!(report.current.rows.len(), 1);
    assert_eq!(
        report.current.rows[0].values,
        vec![Some(3.0 + 4.0 + 6.0 + 7.0), Some(0.5)]
    );
    assert!(report.previous.is_none());
}

#[tokio::test]
async fn unknown_codes_are_refused_before_the_source_is_asked() {
    let (db, _guard) = connect_test(&[]).await;
    let crypt = Crypt::ephemeral();
    let cache = Cache::local();
    let cx = ReportCx {
        db: &db,
        crypt: &crypt,
        cache: &cache,
    };
    let source = stand(None);
    let query = ReportQuery::new("date", &["clicks"], d("2026-03-01"), d("2026-03-02"));
    let err = run(&source, &cx, &query, Compare::Off).await.unwrap_err();
    assert_eq!(err.to_string(), "test.stand has no metric `clicks`");
    let query = ReportQuery::new("city", &["views"], d("2026-03-01"), d("2026-03-02"));
    assert!(matches!(
        run(&source, &cx, &query, Compare::Off).await,
        Err(ReportError::Unknown {
            what: "dimension",
            ..
        })
    ));
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn answers_are_cached_for_the_source_s_time_unless_fresh() {
    let (db, _guard) = connect_test(&[]).await;
    let crypt = Crypt::ephemeral();
    let cache = Cache::local();
    let cx = ReportCx {
        db: &db,
        crypt: &crypt,
        cache: &cache,
    };
    let source = stand(Some(Duration::from_secs(600)));
    let mut query = ReportQuery::new("page", &["views"], d("2026-03-01"), d("2026-03-31"));
    let first = run(&source, &cx, &query, Compare::Off).await.unwrap();
    let again = run(&source, &cx, &query, Compare::Off).await.unwrap();
    assert_eq!(first, again);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    query.fresh = true;
    run(&source, &cx, &query, Compare::Off).await.unwrap();
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);

    let uncached = stand(None);
    query.fresh = false;
    run(&uncached, &cx, &query, Compare::Off).await.unwrap();
    run(&uncached, &cx, &query, Compare::Off).await.unwrap();
    assert_eq!(uncached.calls.load(Ordering::SeqCst), 2);
}

mod table {
    use super::d;
    use laterite_core::query::{bind_values, build};
    use laterite_core::sea_query::{Alias, Query};
    use laterite_core::strata::*;
    use laterite_core::testing::connect_test;
    use laterite_core::{sqlx, t, Cache, Crypt, MigrationSet};
    use rainmill_reporting::{
        run, Compare, FilterOp, Group, Order, OrderBy, ReportCx, ReportQuery, TableSource, Unit,
    };

    struct CreateVisits;

    #[async_trait(?Send)]
    impl Migration for CreateVisits {
        fn name(&self) -> &str {
            "0001_create_visits"
        }
        async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
            s.exec(
                Table::create()
                    .table(Alias::new("rp_visits"))
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("created_at")).text().not_null())
                    .col(ColumnDef::new(Alias::new("city")).text())
                    .col(ColumnDef::new(Alias::new("kind")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("amount"))
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
        }
    }

    fn source() -> TableSource {
        TableSource::new("test.visits", t!("Visits"), "rp_visits", "created_at")
            .dimension("city", t!("City"), "city")
            .count("visits", t!("Visits"))
            .sum("amount", t!("Amount"), "amount", Unit::Money("INR".into()))
    }

    #[tokio::test]
    async fn counts_and_sums_by_day_and_by_column() {
        let sets = vec![MigrationSet::new(
            "test.visits",
            vec![Box::new(CreateVisits)],
        )];
        let (db, _guard) = connect_test(&sets).await;
        for (at, city, kind, amount) in [
            ("2026-03-02T09:00:00Z", Some("Bengaluru"), "guest", 100),
            ("2026-03-02T21:30:00Z", Some("Bengaluru"), "member", 250),
            ("2026-03-04T10:00:00Z", Some("Mumbai"), "guest", 50),
            ("2026-03-04T11:00:00Z", None, "guest", 10),
            ("2026-02-27T10:00:00Z", Some("Mumbai"), "guest", 999),
        ] {
            let (sql, values) = build(
                db.backend,
                Query::insert()
                    .into_table(Alias::new("rp_visits"))
                    .columns(["created_at", "city", "kind", "amount"].map(Alias::new))
                    .values_panic([at.into(), city.into(), kind.into(), amount.into()])
                    .to_owned(),
            );
            bind_values(sqlx::query(&sql), values)
                .execute(&db.pool)
                .await
                .unwrap();
        }
        let crypt = Crypt::ephemeral();
        let cache = Cache::local();
        let cx = ReportCx {
            db: &db,
            crypt: &crypt,
            cache: &cache,
        };
        let source = source();

        let mut q = ReportQuery::new(
            "date",
            &["visits", "amount"],
            d("2026-03-01"),
            d("2026-03-04"),
        );
        q.totals = true;
        let report = run(&source, &cx, &q, Compare::Off).await.unwrap().current;
        let days: Vec<(&str, Vec<Option<f64>>)> = report
            .rows
            .iter()
            .map(|r| (r.key.as_str(), r.values.clone()))
            .collect();
        assert_eq!(
            days,
            vec![
                ("2026-03-01", vec![Some(0.0), Some(0.0)]),
                ("2026-03-02", vec![Some(2.0), Some(350.0)]),
                ("2026-03-03", vec![Some(0.0), Some(0.0)]),
                ("2026-03-04", vec![Some(2.0), Some(60.0)]),
            ]
        );
        assert_eq!(report.totals, vec![Some(4.0), Some(410.0)]);

        q.group = Group::Whole;
        let whole = run(&source, &cx, &q, Compare::Previous).await.unwrap();
        assert_eq!(whole.current.rows[0].values, vec![Some(4.0), Some(410.0)]);
        assert_eq!(
            whole.previous.unwrap().rows[0].values,
            vec![Some(1.0), Some(999.0)]
        );

        let mut by_city = ReportQuery::new("city", &["amount"], d("2026-03-01"), d("2026-03-31"));
        by_city.order = Some(Order {
            by: OrderBy::Metric("amount".into()),
            descending: true,
        });
        by_city.hide_empty = true;
        let rows = run(&source, &cx, &by_city, Compare::Off)
            .await
            .unwrap()
            .current
            .rows;
        let cities: Vec<(&str, Option<f64>)> =
            rows.iter().map(|r| (r.key.as_str(), r.values[0])).collect();
        assert_eq!(
            cities,
            vec![("Bengaluru", Some(350.0)), ("Mumbai", Some(50.0))]
        );

        by_city.filters = vec![rainmill_reporting::Filter {
            op: FilterOp::StartsWith,
            values: vec!["mum".into()],
        }];
        by_city.page = Some(rainmill_reporting::Page {
            per_page: 1,
            page: 1,
        });
        let page = run(&source, &cx, &by_city, Compare::Off)
            .await
            .unwrap()
            .current;
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].key, "Mumbai");
        assert_eq!(page.total_rows, Some(1));

        let scoped = source.only("kind", "member");
        let q = ReportQuery::new("city", &["visits"], d("2026-03-01"), d("2026-03-31"));
        let rows = run(&scoped, &cx, &q, Compare::Off)
            .await
            .unwrap()
            .current
            .rows;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].values, vec![Some(1.0)]);
    }
}

mod layouts {
    use laterite_core::testing::connect_test;
    use rainmill_reporting::dashboard::Definition;
    use rainmill_reporting::layout::{self, Layout, Placed, EVERYONE};

    const YAML: &str = r#"
reports:
  head: { row: 1, type: section, title: Sales }
  orders: { row: 2, width: 5, type: indicator, metric: orders, data_source: acme.orders }
  revenue: { row: 2, width: 5, type: indicator, metric: revenue, data_source: acme.orders }
  trend: { row: 3, type: chart, chart: line, data_source: acme.orders, metrics: [orders] }
"#;

    fn codes(d: &Definition) -> Vec<Vec<(String, u8)>> {
        d.rows()
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|(c, s)| (c.clone(), s.width.unwrap_or(20)))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn an_arrangement_is_kept_to_what_the_dashboard_has() {
        let definition = Definition::parse(YAML).unwrap();
        let sent = r#"{"rows":[[{"code":"trend","width":12},{"code":"orders","width":1},{"code":"ghost","width":5}],
            [{"code":"trend","width":20}],[{"code":"revenue","width":15},{"code":"head","width":20}]],"hidden":["orders","nope"]}"#;
        let layout = Layout::checked(sent, &definition).unwrap();
        assert_eq!(
            layout.rows,
            vec![
                vec![
                    Placed {
                        code: "trend".into(),
                        width: 12
                    },
                    Placed {
                        code: "orders".into(),
                        width: 3
                    }
                ],
                vec![Placed {
                    code: "revenue".into(),
                    width: 15
                }],
                vec![Placed {
                    code: "head".into(),
                    width: 20
                }],
            ],
            "unknown and repeated widgets go, a width stays in bounds, an overfull row splits"
        );
        assert!(layout.hidden.is_empty(), "a placed widget is not hidden");
        assert!(Layout::checked("not json", &definition).is_err());
    }

    #[test]
    fn an_arrangement_moves_resizes_and_hides_and_keeps_newer_widgets() {
        let definition = Definition::parse(YAML).unwrap();
        let layout = Layout {
            rows: vec![vec![
                Placed {
                    code: "trend".into(),
                    width: 14,
                },
                Placed {
                    code: "orders".into(),
                    width: 6,
                },
            ]],
            hidden: vec!["head".into()],
        };
        assert_eq!(
            codes(&definition.arranged(&layout)),
            vec![
                vec![("trend".to_string(), 14), ("orders".to_string(), 6)],
                vec![("revenue".to_string(), 5)],
            ],
            "revenue, which the arrangement does not mention, follows in its own row"
        );
    }

    #[tokio::test]
    async fn a_person_sees_their_own_arrangement_then_the_shared_one() {
        let (db, _guard) = connect_test(&[rainmill_reporting::module().migrations()]).await;
        let now = "2026-11-18T10:00:00Z";
        let one = |code: &str| Layout {
            rows: vec![vec![Placed {
                code: code.into(),
                width: 20,
            }]],
            hidden: vec![],
        };
        assert_eq!(
            layout::of(&db, "acme.sales", 7).await.unwrap(),
            (None, false)
        );
        layout::save(&db, "acme.sales", EVERYONE, &one("trend"), now)
            .await
            .unwrap();
        assert_eq!(
            layout::of(&db, "acme.sales", 7).await.unwrap(),
            (Some(one("trend")), false)
        );
        layout::save(&db, "acme.sales", 7, &one("orders"), now)
            .await
            .unwrap();
        layout::save(&db, "acme.sales", 7, &one("revenue"), now)
            .await
            .unwrap();
        assert_eq!(
            layout::of(&db, "acme.sales", 7).await.unwrap(),
            (Some(one("revenue")), true)
        );
        assert_eq!(
            layout::of(&db, "acme.sales", 8).await.unwrap(),
            (Some(one("trend")), false)
        );
        assert_eq!(
            layout::of(&db, "acme.other", 7).await.unwrap(),
            (None, false)
        );
        layout::forget(&db, "acme.sales", 7).await.unwrap();
        assert_eq!(
            layout::of(&db, "acme.sales", 7).await.unwrap(),
            (Some(one("trend")), false)
        );
    }
}
