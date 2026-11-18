//! Drawing report widgets: server-rendered markup and SVG, no script. Charts
//! share the framework chart's look (`chart.css`); the rest is `reporting.css`.

use chrono::NaiveDate;
use laterite_admin::html::Markup;
use laterite_admin::Shell;

use crate::{Group, Unit};

/// How many colours the palette cycles through, matching `chart.css`.
const COLOURS: usize = 6;
/// The plot's width in drawing units; its height is 100.
const WIDTH: f64 = 1000.0;

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn trusted(html: String) -> Markup {
    // Every caller builds `html` from escaped text and fixed markup.
    Markup::raw_unchecked(html)
}

/// The colour of series `i`: its own when set, else the palette's.
fn colour(own: Option<&str>, i: usize) -> String {
    match own {
        Some(c)
            if c.starts_with('#')
                && c.len() <= 9
                && c[1..].chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            c.to_string()
        }
        _ => format!("var(--chart-{})", i % COLOURS + 1),
    }
}

/// A number with thousands separators and a fixed count of decimals.
fn grouped(value: f64, decimals: usize) -> String {
    let text = format!("{:.decimals$}", value.abs());
    let (whole, rest) = text.split_once('.').unwrap_or((&text, ""));
    let mut out = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if !rest.is_empty() {
        out.push('.');
        out.push_str(rest);
    }
    if value < 0.0 && value.abs() >= 0.5 * 10f64.powi(-(decimals as i32)) {
        out.insert(0, '-');
    }
    out
}

/// A metric's value as an operator reads it; a dash when there is none.
pub fn value_text(unit: &Unit, value: Option<f64>) -> String {
    let Some(v) = value else {
        return "–".into();
    };
    match unit {
        Unit::Number if v.fract() == 0.0 => grouped(v, 0),
        Unit::Number if v.abs() >= 100.0 => grouped(v, 0),
        Unit::Number => grouped(v, 2),
        Unit::Percent => format!("{}%", grouped(v * 100.0, 1)),
        Unit::Duration => {
            let s = v.round() as i64;
            match s {
                ..=59 => format!("{s}s"),
                60..=3599 => format!("{}m {}s", s / 60, s % 60),
                _ => format!("{}h {}m", s / 3600, s % 3600 / 60),
            }
        }
        Unit::Money(code) => format!("{code} {}", grouped(v / 100.0, 2)),
    }
}

/// A date row's key as a label for its group.
pub fn date_label(key: &str, group: Group, to: NaiveDate) -> String {
    let Ok(date) = NaiveDate::parse_from_str(key, "%Y-%m-%d") else {
        return key.to_string();
    };
    match group {
        Group::Day | Group::Week => date.format("%-d %b").to_string(),
        Group::Month => date.format("%b %Y").to_string(),
        Group::Quarter => format!(
            "Q{} {}",
            (date.format("%m").to_string().parse::<u32>().unwrap_or(1) - 1) / 3 + 1,
            date.format("%Y")
        ),
        Group::Year => date.format("%Y").to_string(),
        Group::Whole => format!("{} – {}", date.format("%-d %b %Y"), to.format("%-d %b %Y")),
    }
}

/// The change from `previous` to `current`, as a signed share; `None` when
/// there is nothing to compare with.
pub fn change(current: Option<f64>, previous: Option<f64>) -> Option<f64> {
    match (current, previous) {
        (Some(c), Some(p)) if p != 0.0 => Some((c - p) / p.abs()),
        _ => None,
    }
}

/// One series of a chart or one column of a table.
pub struct Shown {
    /// The metric's name, already localized.
    pub label: String,
    /// How its values read.
    pub unit: Unit,
    /// Its own colour, `#rrggbb`.
    pub colour: Option<String>,
    /// Draw a bar under each cell, relative to the column's largest value.
    pub relative: bool,
}

/// How an axis chart draws its series.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// A line per series.
    Line,
    /// Bars side by side.
    Bar,
    /// Bars stacked into one per point.
    StackedBar,
}

/// The scale's top for a tallest value: the next of 1, 2, 4, 5, 8 and 10
/// times a power of ten.
fn clean_top(tallest: f64) -> f64 {
    if tallest <= 0.0 {
        return 4.0;
    }
    let power = 10f64.powf(tallest.log10().floor());
    let lead = tallest / power;
    let step = [1.0, 2.0, 4.0, 5.0, 8.0, 10.0]
        .into_iter()
        .find(|s| lead <= *s)
        .unwrap_or(10.0);
    (step * power).max(if tallest.fract() == 0.0 { 4.0 } else { 0.0 })
}

/// A line or bar chart: one point per label, a series per metric, a legend
/// with each series' total when given, and a tooltip per point.
pub fn axis(
    shell: &Shell,
    kind: Axis,
    labels: &[String],
    series: &[(Shown, Vec<Option<f64>>)],
    totals: &[Option<f64>],
) -> Markup {
    let n = labels.len().max(1);
    let point = |s: &Vec<Option<f64>>, i: usize| s.get(i).copied().flatten().unwrap_or(0.0);
    let tallest = (0..n)
        .map(|i| {
            if kind == Axis::StackedBar {
                series.iter().map(|(_, v)| point(v, i)).sum()
            } else {
                series.iter().map(|(_, v)| point(v, i)).fold(0.0, f64::max)
            }
        })
        .fold(0.0, f64::max);
    let top = clean_top(tallest);
    let unit = series
        .first()
        .map(|(s, _)| s.unit.clone())
        .unwrap_or(Unit::Number);
    let y = |v: f64| 100.0 - (v / top * 100.0).clamp(0.0, 100.0);
    let step = WIDTH / n as f64;
    let mut svg = String::new();
    for k in 1..4 {
        let at = f64::from(k) * 25.0;
        svg.push_str(&format!(
            r#"<line class="lat-chart__grid" x1="0" x2="{WIDTH}" y1="{at}" y2="{at}"/>"#
        ));
    }
    match kind {
        Axis::Line => {
            for (i, (shown, values)) in series.iter().enumerate() {
                let c = colour(shown.colour.as_deref(), i);
                let pts: Vec<String> = (0..n)
                    .map(|p| format!("{:.1},{:.2}", step * (p as f64 + 0.5), y(point(values, p))))
                    .collect();
                if n > 1 {
                    svg.push_str(&format!(
                        r#"<polygon class="lat-chart__fill" fill="{c}" points="{:.1},100 {} {:.1},100"/>"#,
                        step * 0.5,
                        pts.join(" "),
                        step * (n as f64 - 0.5)
                    ));
                }
                svg.push_str(&format!(
                    r#"<polyline class="lat-chart__line" stroke="{c}" points="{}"/>"#,
                    pts.join(" ")
                ));
            }
        }
        Axis::Bar => {
            let count = series.len().max(1) as f64;
            let width = step * 0.7 / count;
            for (i, (shown, values)) in series.iter().enumerate() {
                let c = colour(shown.colour.as_deref(), i);
                for p in 0..n {
                    let top_at = y(point(values, p));
                    let x = step * p as f64 + step * 0.15 + width * i as f64;
                    svg.push_str(&format!(
                        r#"<rect fill="{c}" x="{x:.1}" y="{top_at:.2}" width="{width:.1}" height="{:.2}"/>"#,
                        100.0 - top_at
                    ));
                }
            }
        }
        Axis::StackedBar => {
            let width = step * 0.6;
            for p in 0..n {
                let mut base = 0.0;
                for (i, (shown, values)) in series.iter().enumerate() {
                    let v = point(values, p);
                    let (low, high) = (y(base), y(base + v));
                    base += v;
                    svg.push_str(&format!(
                        r#"<rect fill="{}" x="{:.1}" y="{high:.2}" width="{width:.1}" height="{:.2}"/>"#,
                        colour(shown.colour.as_deref(), i),
                        step * p as f64 + step * 0.2,
                        low - high
                    ));
                }
            }
        }
    }
    let legend: String = series
        .iter()
        .enumerate()
        .map(|(i, (shown, _))| {
            let total = totals
                .get(i)
                .copied()
                .flatten()
                .map(|t| format!(" <b>{}</b>", escape(&value_text(&shown.unit, Some(t)))))
                .unwrap_or_default();
            format!(
                r#"<span class="lat-chart__chip"><i style="background:{}"></i>{}{total}</span>"#,
                colour(shown.colour.as_deref(), i),
                escape(&shown.label)
            )
        })
        .collect();
    let ticks: String = (0..=4)
        .rev()
        .map(|k| {
            format!(
                "<span>{}</span>",
                escape(&value_text(&unit, Some(top * f64::from(k) / 4.0)))
            )
        })
        .collect();
    let columns: String = (0..n)
        .map(|p| {
            let rows: String = series
                .iter()
                .enumerate()
                .map(|(i, (shown, values))| {
                    format!(
                        r#"<span><i style="background:{}"></i>{}<b>{}</b></span>"#,
                        colour(shown.colour.as_deref(), i),
                        escape(&shown.label),
                        escape(&value_text(&shown.unit, values.get(p).copied().flatten()))
                    )
                })
                .collect();
            let side = if p * 2 >= n { " is-right" } else { "" };
            format!(
                r#"<div class="lat-chart__col{side}"><div class="lat-chart__tip"><strong>{}</strong>{rows}</div></div>"#,
                escape(labels.get(p).map_or("", String::as_str))
            )
        })
        .collect();
    // At most about eight dates under the axis.
    let every = n.div_ceil(8).max(1);
    let dates: String = (0..n)
        .map(|p| {
            let text = if p % every == 0 {
                labels.get(p).map_or("", String::as_str)
            } else {
                ""
            };
            let alt = if (p / every) % 2 == 1 {
                r#" class="is-alt""#
            } else {
                ""
            };
            format!("<span{alt}>{}</span>", escape(text))
        })
        .collect();
    let empty = if tallest <= 0.0 {
        format!(
            r#"<p class="lat-chart__empty">{}</p>"#,
            escape(&shell.t("Nothing in this period."))
        )
    } else {
        String::new()
    };
    trusted(format!(
        r#"<div class="rp-axis"><div class="lat-chart__legend">{legend}</div><div class="lat-chart__body"><div class="lat-chart__y" aria-hidden="true">{ticks}</div><div class="lat-chart__plot"><svg viewBox="0 0 {WIDTH} 100" preserveAspectRatio="none" aria-hidden="true">{svg}</svg><div class="lat-chart__cols" style="grid-template-columns:repeat({n},1fr)">{columns}</div>{empty}</div><div class="lat-chart__x" style="grid-template-columns:repeat({n},1fr)" aria-hidden="true">{dates}</div></div>{}</div>"#,
        data_table(labels, series)
    ))
}

/// The figures behind a chart, for screen readers.
fn data_table(labels: &[String], series: &[(Shown, Vec<Option<f64>>)]) -> String {
    let heads: String = series
        .iter()
        .map(|(s, _)| format!("<th>{}</th>", escape(&s.label)))
        .collect();
    let rows: String = labels
        .iter()
        .enumerate()
        .map(|(p, label)| {
            let cells: String = series
                .iter()
                .map(|(s, v)| {
                    format!(
                        "<td>{}</td>",
                        escape(&value_text(&s.unit, v.get(p).copied().flatten()))
                    )
                })
                .collect();
            format!("<tr><th>{}</th>{cells}</tr>", escape(label))
        })
        .collect();
    format!(
        r#"<table class="lat-chart__data"><thead><tr><td></td>{heads}</tr></thead><tbody>{rows}</tbody></table>"#
    )
}

/// Horizontal bars: a row per label, longest first as given.
pub fn bars(shell: &Shell, labels: &[String], shown: &Shown, values: &[Option<f64>]) -> Markup {
    let largest = values.iter().flatten().copied().fold(0.0, f64::max);
    if labels.is_empty() || largest <= 0.0 {
        return nothing(shell);
    }
    let c = colour(shown.colour.as_deref(), 0);
    let rows: String = labels
        .iter()
        .zip(values)
        .map(|(label, v)| {
            let share = v.unwrap_or(0.0) / largest * 100.0;
            format!(
                r#"<li><span class="rp-bars__label" title="{l}">{l}</span><span class="rp-bars__track"><i style="width:{share:.1}%;background:{c}"></i></span><b>{}</b></li>"#,
                escape(&value_text(&shown.unit, *v)),
                l = escape(label)
            )
        })
        .collect();
    trusted(format!(
        r#"<ol class="rp-bars" aria-label="{}">{rows}</ol>"#,
        escape(&shown.label)
    ))
}

/// How a ring chart is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    /// A ring with the total in the middle.
    Doughnut,
    /// A full disc.
    Pie,
}

/// A doughnut or pie: a slice per label, and a legend with each value and
/// share.
pub fn ring(
    shell: &Shell,
    kind: Ring,
    labels: &[String],
    shown: &Shown,
    values: &[Option<f64>],
    total: Option<f64>,
) -> Markup {
    let sum: f64 = values.iter().flatten().filter(|v| **v > 0.0).sum();
    if labels.is_empty() || sum <= 0.0 {
        return nothing(shell);
    }
    let (r, inner) = (50.0, if kind == Ring::Doughnut { 31.0 } else { 0.0 });
    let mut angle = -std::f64::consts::FRAC_PI_2;
    let mut slices = String::new();
    let mut legend = String::new();
    for (i, (label, v)) in labels.iter().zip(values).enumerate() {
        let v = v.unwrap_or(0.0).max(0.0);
        let share = v / sum;
        let c = colour(None, i);
        legend.push_str(&format!(
            r#"<li><i style="background:{c}"></i><span title="{l}">{l}</span><b>{}</b><small>{}</small></li>"#,
            escape(&value_text(&shown.unit, Some(v))),
            escape(&value_text(&Unit::Percent, Some(share))),
            l = escape(label)
        ));
        if share <= 0.0 {
            continue;
        }
        if share >= 0.9999 {
            slices.push_str(&format!(
                r#"<circle cx="50" cy="50" r="{:.2}" fill="none" stroke="{c}" stroke-width="{:.2}"/>"#,
                (r + inner) / 2.0,
                r - inner
            ));
            continue;
        }
        let end = angle + share * std::f64::consts::TAU;
        let large = u8::from(share > 0.5);
        let p = |a: f64, rad: f64| (50.0 + rad * a.cos(), 50.0 + rad * a.sin());
        let (x1, y1) = p(angle, r);
        let (x2, y2) = p(end, r);
        let path = if inner > 0.0 {
            let (x3, y3) = p(end, inner);
            let (x4, y4) = p(angle, inner);
            format!("M{x1:.2} {y1:.2}A{r} {r} 0 {large} 1 {x2:.2} {y2:.2}L{x3:.2} {y3:.2}A{inner} {inner} 0 {large} 0 {x4:.2} {y4:.2}Z")
        } else {
            format!("M50 50L{x1:.2} {y1:.2}A{r} {r} 0 {large} 1 {x2:.2} {y2:.2}Z")
        };
        slices.push_str(&format!(
            r#"<path d="{path}" fill="{c}"><title>{}</title></path>"#,
            escape(label)
        ));
        angle = end;
    }
    let middle = match (kind, total.or(Some(sum))) {
        (Ring::Doughnut, Some(t)) => format!(
            r#"<span class="rp-ring__total"><b>{}</b><small>{}</small></span>"#,
            escape(&value_text(&shown.unit, Some(t))),
            escape(&shown.label)
        ),
        _ => String::new(),
    };
    trusted(format!(
        r#"<div class="rp-ring"><div class="rp-ring__plot"><svg viewBox="0 0 100 100" role="img" aria-label="{}">{slices}</svg>{middle}</div><ul class="rp-ring__legend">{legend}</ul></div>"#,
        escape(&shown.label)
    ))
}

/// Each value as a share of the whole: one stacked bar and a list.
pub fn shares(shell: &Shell, labels: &[String], shown: &Shown, values: &[Option<f64>]) -> Markup {
    let sum: f64 = values.iter().flatten().filter(|v| **v > 0.0).sum();
    if labels.is_empty() || sum <= 0.0 {
        return nothing(shell);
    }
    let mut strip = String::new();
    let mut list = String::new();
    for (i, (label, v)) in labels.iter().zip(values).enumerate() {
        let share = v.unwrap_or(0.0).max(0.0) / sum;
        let c = colour(None, i);
        strip.push_str(&format!(
            r#"<i style="width:{:.2}%;background:{c}" title="{}"></i>"#,
            share * 100.0,
            escape(label)
        ));
        list.push_str(&format!(
            r#"<li><i style="background:{c}"></i><span>{}</span><b>{}</b><small>{}</small></li>"#,
            escape(label),
            escape(&value_text(&Unit::Percent, Some(share))),
            escape(&value_text(&shown.unit, *v))
        ));
    }
    trusted(format!(
        r#"<div class="rp-shares"><div class="rp-shares__strip" aria-hidden="true">{strip}</div><ul class="rp-ring__legend">{list}</ul></div>"#
    ))
}

/// One figure, with its change against the comparison and its progress toward
/// a goal.
pub struct Figure<'a> {
    /// What it is, already localized.
    pub label: &'a str,
    /// How it reads.
    pub unit: &'a Unit,
    /// The value.
    pub value: Option<f64>,
    /// The comparison period's value.
    pub previous: Option<f64>,
    /// A target the value is measured against.
    pub goal: Option<f64>,
    /// A framework icon name.
    pub icon: Option<&'a str>,
    /// Where the figure links to.
    pub link: Option<&'a str>,
    /// A falling value is the good direction (bounce rate, cost).
    pub lower_is_better: bool,
}

/// A figure on its own.
pub fn indicator(shell: &Shell, f: &Figure<'_>) -> Markup {
    let icon = f
        .icon
        .map(|i| format!(r#"<span class="rp-figure__icon">{}</span>"#, shell.icon(i)))
        .unwrap_or_default();
    let delta = change(f.value, f.previous)
        .map(|c| {
            let good = (c >= 0.0) != f.lower_is_better;
            let tone = if c.abs() < 0.0005 { "flat" } else if good { "up" } else { "down" };
            let arrow = if c > 0.0 { "▲" } else if c < 0.0 { "▼" } else { "" };
            format!(
                r#"<small class="rp-figure__delta" data-tone="{tone}" title="{}">{arrow} {}</small>"#,
                escape(&format!("{} {}", shell.t("Previous period:"), value_text(f.unit, f.previous))),
                escape(&value_text(&Unit::Percent, Some(c.abs())))
            )
        })
        .unwrap_or_default();
    let goal = match (f.goal, f.value) {
        (Some(g), v) if g > 0.0 => {
            let share = v.unwrap_or(0.0) / g;
            format!(
                r#"<div class="rp-figure__goal"><span class="rp-figure__track"><i style="width:{:.1}%"></i></span><small>{} {}</small></div>"#,
                (share * 100.0).clamp(0.0, 100.0),
                escape(&value_text(&Unit::Percent, Some(share))),
                escape(&format!(
                    "{} {}",
                    shell.t("of"),
                    value_text(f.unit, Some(g))
                ))
            )
        }
        _ => String::new(),
    };
    let body = format!(
        r#"{icon}<span class="rp-figure__label">{}</span><b class="rp-figure__value">{}</b>{delta}{goal}"#,
        escape(f.label),
        escape(&value_text(f.unit, f.value))
    );
    trusted(match f.link {
        Some(href) => format!(r#"<a class="rp-figure" href="{}">{body}</a>"#, escape(href)),
        None => format!(r#"<div class="rp-figure">{body}</div>"#),
    })
}

/// A table: the dimension, then a column per metric, a totals row when given.
pub fn table(
    shell: &Shell,
    dimension: &str,
    columns: &[Shown],
    rows: &[(String, Vec<Option<f64>>)],
    totals: Option<&[Option<f64>]>,
) -> Markup {
    if rows.is_empty() {
        return nothing(shell);
    }
    let largest: Vec<f64> = (0..columns.len())
        .map(|i| {
            rows.iter()
                .filter_map(|(_, v)| v.get(i).copied().flatten())
                .fold(0.0, f64::max)
        })
        .collect();
    let heads: String = columns
        .iter()
        .map(|c| format!(r#"<th class="rp-num">{}</th>"#, escape(&c.label)))
        .collect();
    let body: String = rows
        .iter()
        .map(|(key, values)| {
            let cells: String = columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let v = values.get(i).copied().flatten();
                    let bar = if c.relative && largest[i] > 0.0 {
                        format!(
                            r#"<span class="rp-cell__bar"><i style="width:{:.1}%;background:{}"></i></span>"#,
                            v.unwrap_or(0.0) / largest[i] * 100.0,
                            colour(c.colour.as_deref(), i)
                        )
                    } else {
                        String::new()
                    };
                    format!(r#"<td class="rp-num">{}{bar}</td>"#, escape(&value_text(&c.unit, v)))
                })
                .collect();
            format!(r#"<tr><td class="rp-key" title="{k}">{k}</td>{cells}</tr>"#, k = escape(key))
        })
        .collect();
    let foot = totals
        .map(|t| {
            let cells: String = columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    format!(
                        r#"<td class="rp-num">{}</td>"#,
                        escape(&value_text(&c.unit, t.get(i).copied().flatten()))
                    )
                })
                .collect();
            format!(
                r#"<tfoot><tr><td>{}</td>{cells}</tr></tfoot>"#,
                escape(&shell.t("Total"))
            )
        })
        .unwrap_or_default();
    trusted(format!(
        r#"<div class="lat-tablewrap"><table class="lat-table rp-table"><thead><tr><th>{}</th>{heads}</tr></thead><tbody>{body}</tbody>{foot}</table></div>"#,
        escape(dimension)
    ))
}

/// A heading across the row, with the period when given.
pub fn section(title: &str, period: Option<&str>) -> Markup {
    let period = period
        .map(|p| format!(r#"<small>{}</small>"#, escape(p)))
        .unwrap_or_default();
    trusted(format!(
        r#"<div class="rp-section"><h2>{}</h2>{period}</div>"#,
        escape(title)
    ))
}

/// A paragraph of text in a tone: `info`, `success`, `warning` or `danger`.
pub fn notice(text: &str, tone: &str) -> Markup {
    let tone = match tone {
        "success" | "warning" | "danger" => tone,
        _ => "info",
    };
    trusted(format!(
        r#"<p class="rp-notice" data-tone="{tone}">{}</p>"#,
        escape(text)
    ))
}

/// A widget that could not be drawn, with the reason.
pub fn failed(reason: &str) -> Markup {
    notice(reason, "danger")
}

/// A widget whose source is failing, named once above the dashboard.
pub fn unavailable(text: &str) -> Markup {
    trusted(format!(r#"<p class="rp-empty">{}</p>"#, escape(text)))
}

fn nothing(shell: &Shell) -> Markup {
    trusted(format!(
        r#"<p class="rp-empty">{}</p>"#,
        escape(&shell.t("Nothing in this period."))
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_by_their_unit() {
        assert_eq!(value_text(&Unit::Number, Some(1234567.0)), "1,234,567");
        assert_eq!(value_text(&Unit::Number, Some(2.456)), "2.46");
        assert_eq!(value_text(&Unit::Percent, Some(0.2761)), "27.6%");
        assert_eq!(value_text(&Unit::Duration, Some(42.4)), "42s");
        assert_eq!(value_text(&Unit::Duration, Some(185.0)), "3m 5s");
        assert_eq!(value_text(&Unit::Duration, Some(3725.0)), "1h 2m");
        assert_eq!(
            value_text(&Unit::Money("INR".into()), Some(123456.0)),
            "INR 1,234.56"
        );
        assert_eq!(value_text(&Unit::Number, None), "–");
        assert_eq!(value_text(&Unit::Number, Some(-1500.0)), "-1,500");
    }

    #[test]
    fn date_keys_read_as_their_group() {
        let to = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
        assert_eq!(date_label("2026-11-03", Group::Day, to), "3 Nov");
        assert_eq!(date_label("2026-10-01", Group::Month, to), "Oct 2026");
        assert_eq!(date_label("2026-10-01", Group::Quarter, to), "Q4 2026");
        assert_eq!(date_label("2026-01-01", Group::Year, to), "2026");
        assert_eq!(date_label("(other)", Group::Day, to), "(other)");
    }

    #[test]
    fn change_is_a_share_of_the_previous_value() {
        assert_eq!(change(Some(150.0), Some(100.0)), Some(0.5));
        assert_eq!(change(Some(50.0), Some(100.0)), Some(-0.5));
        assert_eq!(change(Some(5.0), Some(0.0)), None);
        assert_eq!(change(None, Some(3.0)), None);
    }

    #[test]
    fn only_a_hex_colour_is_used_as_given() {
        assert_eq!(colour(Some("#84cc16"), 0), "#84cc16");
        assert_eq!(colour(Some("red;background:url(x)"), 1), "var(--chart-2)");
        assert_eq!(colour(None, 7), "var(--chart-2)");
    }
}
