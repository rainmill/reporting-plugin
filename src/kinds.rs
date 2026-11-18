//! The report widgets as kinds a board places: indicator, chart, table,
//! percentage, section and notice, each with the settings Configure edits.

use std::sync::Arc;

use laterite_admin::board::{BoardCx, Kind as BoardKind, KindReg, COLUMNS};
use laterite_admin::form::{FormField, Span};
use laterite_admin::html::Markup;
use laterite_admin::Shell;
use laterite_core::serde_json::{self, json, Map, Value};
use laterite_core::{t, Registry, Text};

use crate::dashboard::{self, Kind, View, WidgetSpec};
use crate::draw;
use crate::source::{DataSource, DataSourceReg, ReportCx, ReportError};

/// The prefix every report widget's kind carries.
pub const PREFIX: &str = "rainmill.reporting";

/// The board kind's code for a report widget type.
pub fn code_of(kind: Kind) -> &'static str {
    match kind {
        Kind::Section => "rainmill.reporting.section",
        Kind::Indicator => "rainmill.reporting.indicator",
        Kind::Chart => "rainmill.reporting.chart",
        Kind::Table => "rainmill.reporting.table",
        Kind::Percentage => "rainmill.reporting.percentage",
        Kind::Notice => "rainmill.reporting.notice",
    }
}

fn type_of(kind: Kind) -> &'static str {
    match kind {
        Kind::Section => "section",
        Kind::Indicator => "indicator",
        Kind::Chart => "chart",
        Kind::Table => "table",
        Kind::Percentage => "percentage",
        Kind::Notice => "notice",
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::String(s) => matches!(s.as_str(), "1" | "true" | "on" | "yes"),
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        _ => false,
    }
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().filter_map(text).collect(),
        Value::String(s) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// A widget as a dashboard's YAML writes it, as the flat settings a board
/// keeps and Configure edits: metrics as a list of codes with their colours
/// beside them, numbers as text, filters as JSON text.
pub fn settings_of(spec: &Value) -> Value {
    let mut out = Map::new();
    let Some(spec) = spec.as_object() else {
        return Value::Object(out);
    };
    for key in [
        "title",
        "data_source",
        "dimension",
        "metric",
        "chart",
        "sort",
        "order",
        "group",
        "icon",
        "link",
        "text",
        "tone",
    ] {
        if let Some(value) = spec.get(key).and_then(text) {
            out.insert(key.into(), value.into());
        }
    }
    for key in ["limit", "goal"] {
        if let Some(value) = spec.get(key).and_then(text) {
            out.insert(key.into(), value.into());
        }
    }
    for key in ["hide_empty", "lower_is_better", "show_period"] {
        if spec.get(key).is_some_and(truthy) {
            out.insert(key.into(), true.into());
        }
    }
    if let Some(Value::Array(metrics)) = spec.get("metrics") {
        let mut codes = Vec::new();
        let mut colours = Vec::new();
        let (mut totals, mut bars) = (false, false);
        for metric in metrics {
            match metric {
                Value::String(code) => {
                    codes.push(code.clone());
                    colours.push(String::new());
                }
                Value::Object(full) => {
                    codes.push(full.get("metric").and_then(text).unwrap_or_default());
                    colours.push(full.get("colour").and_then(text).unwrap_or_default());
                    totals |= full.get("totals").is_some_and(truthy);
                    bars |= full.get("bar").is_some_and(truthy);
                }
                _ => {}
            }
        }
        out.insert("metrics".into(), json!(codes));
        if colours.iter().any(|c| !c.is_empty()) {
            out.insert("colours".into(), colours.join(", ").into());
        }
        if totals {
            out.insert("totals".into(), true.into());
        }
        if bars {
            out.insert("bars".into(), true.into());
        }
    }
    if let Some(filters) = spec
        .get("filters")
        .filter(|f| f.as_array().is_some_and(|a| !a.is_empty()))
    {
        out.insert("filters".into(), filters.to_string().into());
    }
    Value::Object(out)
}

/// The widget a board's settings describe. Lenient about shape: a number may
/// be text, a switch `true` or `"1"`, and an empty value is as if unset.
pub fn spec_of(kind: Kind, settings: &Value) -> Result<WidgetSpec, String> {
    let empty = Map::new();
    let settings = settings.as_object().unwrap_or(&empty);
    let mut spec = Map::new();
    spec.insert("type".into(), type_of(kind).into());
    for key in [
        "title",
        "data_source",
        "dimension",
        "metric",
        "chart",
        "sort",
        "order",
        "group",
        "icon",
        "link",
        "text",
        "tone",
    ] {
        if let Some(value) = settings.get(key).and_then(text) {
            spec.insert(key.into(), value.into());
        }
    }
    if let Some(limit) = settings
        .get("limit")
        .and_then(text)
        .and_then(|l| l.parse::<u32>().ok())
    {
        spec.insert("limit".into(), limit.into());
    }
    if let Some(goal) = settings
        .get("goal")
        .and_then(text)
        .and_then(|g| g.parse::<f64>().ok())
    {
        spec.insert("goal".into(), goal.into());
    }
    for key in ["hide_empty", "lower_is_better", "show_period"] {
        if settings.get(key).is_some_and(truthy) {
            spec.insert(key.into(), true.into());
        }
    }
    let codes = settings.get("metrics").map(list).unwrap_or_default();
    if !codes.is_empty() {
        let colours: Vec<String> = settings
            .get("colours")
            .and_then(Value::as_str)
            .map(|c| c.split(',').map(|c| c.trim().to_string()).collect())
            .unwrap_or_default();
        let totals = settings.get("totals").is_some_and(truthy);
        let bars = settings.get("bars").is_some_and(truthy);
        let metrics: Vec<Value> = codes
            .iter()
            .enumerate()
            .map(|(i, code)| {
                let colour = colours.get(i).filter(|c| !c.is_empty());
                if colour.is_none() && !totals && !bars {
                    json!(code)
                } else {
                    json!({ "metric": code, "colour": colour, "totals": totals, "bar": bars })
                }
            })
            .collect();
        spec.insert("metrics".into(), metrics.into());
    }
    match settings.get("filters") {
        Some(Value::String(json)) if !json.trim().is_empty() => {
            let filters: Value = serde_json::from_str(json)
                .map_err(|e| format!("the filters are not valid JSON: {e}"))?;
            spec.insert("filters".into(), filters);
        }
        Some(filters @ Value::Array(_)) => {
            spec.insert("filters".into(), filters.clone());
        }
        _ => {}
    }
    WidgetSpec::from_json(Value::Object(spec))
}

fn option(value: &str, label: &str) -> Value {
    json!({ "value": value, "label": label })
}

fn select(name: &str, label: Text, options: Vec<Value>) -> FormField {
    FormField::of(name, label, "select").options(json!({ "options": options }))
}

fn half(field: FormField) -> FormField {
    field.span(Span::columns(6))
}

/// A report widget type as a board kind.
struct Report(Kind);

impl Report {
    fn source<'a>(cx: &'a BoardCx<'_>, code: &str) -> Option<&'a dyn DataSource> {
        cx.ctx
            .contributions::<DataSourceReg>()
            .into_iter()
            .find(|r| r.source.code() == code)
            .map(|r| r.source.as_ref())
    }
}

#[laterite_core::strata::async_trait]
impl BoardKind for Report {
    fn code(&self) -> &str {
        code_of(self.0)
    }
    fn label(&self) -> Text {
        match self.0 {
            Kind::Section => t!("Section title"),
            Kind::Indicator => t!("Indicator"),
            Kind::Chart => t!("Chart"),
            Kind::Table => t!("Table"),
            Kind::Percentage => t!("Breakdown"),
            Kind::Notice => t!("Notice"),
        }
    }
    fn group(&self) -> Text {
        t!("Reports")
    }
    fn description(&self) -> Option<Text> {
        Some(match self.0 {
            Kind::Section => t!("A heading across the dashboard."),
            Kind::Indicator => t!("One figure, with its change and a goal."),
            Kind::Chart => t!("A line, bar, doughnut or pie chart."),
            Kind::Table => t!("Rows by a dimension, with totals."),
            Kind::Percentage => t!("Each value's share of the whole."),
            Kind::Notice => t!("A line of text."),
        })
    }
    fn default_width(&self) -> u8 {
        match self.0 {
            Kind::Section | Kind::Notice => COLUMNS,
            Kind::Indicator => 5,
            _ => 10,
        }
    }
    fn resizable(&self) -> bool {
        self.0 != Kind::Section
    }
    fn bare(&self) -> bool {
        self.0 == Kind::Section
    }
    fn reload_on(&self) -> Vec<&'static str> {
        vec!["data_source"]
    }
    fn properties(&self) -> Vec<FormField> {
        // The fields that need no data source; the whole form comes from
        // `properties_for`.
        vec![FormField::text("title", t!("Title"))]
    }
    fn properties_for(&self, cx: &BoardCx<'_>, current: &Value) -> Vec<FormField> {
        let shell = cx.shell;
        let mut fields = vec![FormField::text("title", t!("Title"))];
        match self.0 {
            Kind::Section => {
                fields.push(FormField::switch(
                    "show_period",
                    t!("Show the period beside it"),
                ));
                return fields;
            }
            Kind::Notice => {
                fields.push(FormField::textarea("text", t!("Text")).required());
                fields.push(select(
                    "tone",
                    t!("Tone"),
                    vec![
                        option("info", &shell.t("Information")),
                        option("success", &shell.t("Success")),
                        option("warning", &shell.t("Warning")),
                        option("danger", &shell.t("Danger")),
                    ],
                ));
                return fields;
            }
            _ => {}
        }
        let sources: Vec<&DataSourceReg> = cx
            .ctx
            .contributions::<DataSourceReg>()
            .into_iter()
            .filter(|r| r.source.permission().is_none_or(|p| shell.allows(p)))
            .collect();
        let mut source_options = vec![option("", &shell.t("Choose a data source"))];
        source_options.extend(
            sources
                .iter()
                .map(|r| option(r.source.code(), &shell.tt(&r.source.label()))),
        );
        fields.push(select("data_source", t!("Data source"), source_options).required());
        let chosen = current
            .get("data_source")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(source) = sources
            .iter()
            .find(|r| r.source.code() == chosen)
            .map(|r| &r.source)
        else {
            return fields;
        };
        let metric_options: Vec<Value> = source
            .metrics()
            .iter()
            .map(|m| option(&m.code, &shell.tt(&m.label)))
            .collect();
        let mut dimension_options = vec![option("", &shell.t("By date"))];
        dimension_options.extend(
            source
                .dimensions()
                .iter()
                .filter(|d| !d.date)
                .map(|d| option(&d.code, &shell.tt(&d.label))),
        );
        let order = || {
            half(select(
                "order",
                t!("Order"),
                vec![
                    option("", &shell.t("As the source gives it")),
                    option("desc", &shell.t("Largest first")),
                    option("asc", &shell.t("Smallest first")),
                ],
            ))
        };
        let limit = || {
            half(
                FormField::of("limit", t!("Most rows"), "text")
                    .options(json!({ "input": "number", "min": 1 })),
            )
        };
        match self.0 {
            Kind::Indicator => {
                fields.push(half(
                    select("metric", t!("Figure"), metric_options).required(),
                ));
                fields.push(half(FormField::text("icon", t!("Icon"))));
                fields.push(half(
                    FormField::of("goal", t!("Goal"), "text").options(json!({ "input": "number" })),
                ));
                fields.push(half(FormField::text("link", t!("Link"))));
                fields.push(FormField::switch("lower_is_better", t!("Lower is better")));
            }
            Kind::Percentage => {
                fields.push(half(select(
                    "dimension",
                    t!("Broken down by"),
                    dimension_options,
                )));
                fields.push(half(
                    select("metric", t!("Figure"), metric_options).required(),
                ));
                fields.push(limit());
                fields.push(FormField::switch(
                    "hide_empty",
                    t!("Leave out empty values"),
                ));
            }
            _ => {
                if self.0 == Kind::Chart {
                    fields.push(half(select(
                        "chart",
                        t!("Chart"),
                        vec![
                            option("line", &shell.t("Line")),
                            option("bar", &shell.t("Bar")),
                            option("stacked_bar", &shell.t("Stacked bar")),
                            option("horizontal_bar", &shell.t("Horizontal bar")),
                            option("doughnut", &shell.t("Doughnut")),
                            option("pie", &shell.t("Pie")),
                        ],
                    )));
                }
                fields.push(half(select(
                    "dimension",
                    t!("Broken down by"),
                    dimension_options,
                )));
                fields.push(
                    FormField::of("metrics", t!("Figures"), "checklist")
                        .options(json!({ "options": metric_options }))
                        .required(),
                );
                fields.push(half(FormField::text("colours", t!("Colours")).help(t!(
                    "One for each figure in order, separated by commas; blank takes the palette."
                ))));
                fields.push(limit());
                fields.push(order());
                fields.push(FormField::switch("totals", t!("Show totals")));
                if self.0 == Kind::Table {
                    fields.push(FormField::switch(
                        "bars",
                        t!("Show a bar in each figure's cell"),
                    ));
                }
                fields.push(FormField::switch(
                    "hide_empty",
                    t!("Leave out empty values"),
                ));
            }
        }
        fields.push(
            FormField::textarea("filters", t!("Filters")).help(t!(
                "JSON: a list of filters, each with an op (equals, starts_with, contains, one_of) and a value or values."
            )),
        );
        fields
    }
    fn check(&self, properties: &Value) -> Result<(), String> {
        spec_of(self.0, properties).map(|_| ())
    }
    fn title(&self, _shell: &Shell, properties: &Value) -> Option<String> {
        match self.0 {
            Kind::Section | Kind::Indicator => None,
            _ => properties.get("title").and_then(text),
        }
    }
    async fn render(&self, cx: &BoardCx<'_>, properties: &Value) -> Result<Markup, String> {
        let shell = cx.shell;
        let view = View::from_query(cx.query, dashboard::today());
        let spec = match spec_of(self.0, properties) {
            Ok(spec) => spec,
            // A widget just added has no settings yet.
            Err(_) if properties.as_object().is_none_or(Map::is_empty) => {
                return Ok(draw::unavailable(
                    &shell.t("Configure this widget to choose what it shows."),
                ));
            }
            Err(e) => return Err(e),
        };
        let Some(code) = spec.data_source.as_deref() else {
            return Ok(dashboard::static_widget(&spec, &view));
        };
        let Some(source) = Self::source(cx, code) else {
            return Err(shell.t("This widget's data source is not installed."));
        };
        if source.permission().is_some_and(|p| !shell.allows(p)) {
            return Ok(draw::unavailable(&shell.t("Not available.")));
        }
        let report = ReportCx {
            db: cx.ctx.db(),
            crypt: cx.ctx.crypt(),
            cache: cx.ctx.cache(),
        };
        match dashboard::widget(shell, &report, source, &spec, &view).await {
            Ok(markup) => Ok(markup),
            Err(ReportError::Source(reason)) => Ok(draw::unavailable(&format!(
                "{}: {reason}",
                shell.tt(&source.label())
            ))),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Registers the report widgets as board kinds.
pub(crate) fn register(registry: &mut Registry) {
    for kind in [
        Kind::Indicator,
        Kind::Chart,
        Kind::Table,
        Kind::Percentage,
        Kind::Section,
        Kind::Notice,
    ] {
        registry.add(KindReg::new(Arc::new(Report(kind))));
    }
}
