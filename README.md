# Rainmill Reporting

Report dashboards for a [Laterite](https://laterite.rs) application.

- **Data sources**: a source answers one question shape (a dimension, some metrics, a period, filters, a sort and a limit), and any widget draws from any source.
- **Widgets**: section, indicator (with the change against a comparison period and a goal), line, bar, stacked bar, horizontal bar, doughnut, pie, percentage, table (with totals and in-cell bars) and notice, drawn on the server with no script.
- **Dashboards**: written in YAML on a 20-column grid, under a Reports item in the admin menu.
- **Table source**: counts rows or sums a column of any table, by day or by a column's value, with no code beyond declaring it.

## Requirements

Laterite 0.11.

## Installation

```toml
[dependencies]
rainmill-reporting = "0.1"
```

Add `rainmill_reporting::module()` to the application's modules.

## A table source

```rust
use rainmill_reporting::{DataSourceReg, TableSource, Unit};

registry.add(DataSourceReg::new(Arc::new(
    TableSource::new("acme.orders", t!("Orders"), "orders", "created_at")
        .permission("acme.orders.view")
        .dimension("city", t!("City"), "city")
        .count("orders", t!("Orders"))
        .sum("revenue", t!("Revenue"), "total_minor", Unit::Money("INR".into()))
        .only("status", "paid"),
)));
```

| Method | Adds |
| --- | --- |
| `dimension(code, label, column)` | A breakdown by the column's value |
| `count(code, label)` | Rows counted |
| `sum(code, label, column, unit)` | The column summed |
| `only(column, value)` | A condition every row meets; each call adds one |
| `permission(name)` | The permission an operator needs to read it |

The date column holds UTC text timestamps or dates.

## A dashboard

```rust
registry.add(
    rainmill_reporting::DashboardReg::new("acme.sales", t!("Sales"), include_str!("../dashboards/sales.yaml"))
        .icon("chart-bar")
        .permission("acme.orders.view"),
);
```

```yaml
reports:
  heading:
    row: 1
    type: section
    title: Sales
    show_period: true
  orders:
    row: 2
    width: 5
    type: indicator
    icon: shopping-cart
    metric: orders
    goal: 500
    data_source: acme.orders
  trend:
    row: 2
    width: 15
    type: chart
    chart: line
    data_source: acme.orders
    metrics:
      - metric: revenue
        totals: true
  cities:
    row: 3
    type: table
    dimension: city
    limit: 10
    data_source: acme.orders
    metrics:
      - metric: orders
        bar: true
        totals: true
      - revenue
```

| Key | Description |
| --- | --- |
| `type` | `section`, `indicator`, `chart`, `table`, `percentage` or `notice` |
| `row`, `width` | The row, and the columns of 20 it spans (all of them when left out) |
| `title` | The heading |
| `data_source` | The source's code |
| `dimension` | The dimension's code; `date` when left out |
| `metric` | The metric of an indicator or percentage |
| `metrics` | The metrics of a chart or table: a code, or `metric` with `colour`, `totals` and `bar` |
| `chart` | `line`, `bar`, `stacked_bar`, `horizontal_bar`, `doughnut` or `pie` |
| `group` | `day`, `week`, `month`, `quarter`, `year` or `whole`; suited to the period when left out |
| `limit`, `sort`, `order` | The most rows, `dimension` or a metric code, `asc` or `desc` |
| `filters` | `op` (`equals`, `starts_with`, `contains`, `one_of`) with `value` or `values` |
| `hide_empty` | Leave out empty and unset values |
| `goal`, `icon`, `link`, `lower_is_better` | An indicator's target, icon, link, and the direction that is good |
| `show_period` | A section shows the period |
| `text`, `tone` | A notice's text and `info`, `success`, `warning` or `danger` |

The period bar offers today, 7, 30 and 90 days, this month, last month, this quarter, this year and a custom range, with a comparison against the previous period or the same period last year. Refresh asks every source again.

## Arrange a dashboard

**Edit dashboard** (permission `rainmill.reporting.manage`) turns the page into its editor:

| Action | How |
| --- | --- |
| Move a widget | Drag it onto another widget, to its left or right half; onto the strip at the bottom for a new row |
| Resize | Drag its right edge; it snaps to the 20 columns, from 3 to what its row has free |
| Remove | The button at its top right, shown on hover |
| Apply | Saves the layout as your own |
| Apply for everyone | Saves it as the layout everyone without their own sees |
| Reset to the shared layout | Drops your own |
| Cancel | Leaves the page as it was |

A widget moves into a row only when its width fits there. A widget a plugin adds to the dashboard later appears after the arranged ones.

## A source of your own

```rust
#[async_trait]
impl DataSource for Visits {
    fn code(&self) -> &str { "acme.visits" }
    fn label(&self) -> Text { t!("Visits") }
    fn dimensions(&self) -> Vec<Dimension> { vec![Dimension::date("day")] }
    fn metrics(&self) -> Vec<Metric> { vec![Metric::new("visits", t!("Visits"), "visits")] }
    fn cache_for(&self) -> Option<Duration> { Some(Duration::from_secs(600)) }
    async fn fetch(&self, cx: &ReportCx<'_>, q: &ReportQuery) -> Result<ReportData, ReportError> {
        // Rows of (key, values in the query's metric order); the engine fills empty days.
        todo!()
    }
}
```

The page opens with every widget's frame, and each widget with a data source then fetches itself from `/reports/<dashboard>/widgets/<widget>`. A source that fails with `ReportError::Source` shows its reason inside its own widgets; the rest of the dashboard draws.

A widget sits directly under the one above it in its columns, whatever the heights beside it.

## License

MIT or Apache-2.0, at your option.
