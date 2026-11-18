//! A dashboard's arrangement as an operator left it: which widgets share a
//! row, how wide each is, and which are hidden. Kept one a person, with one
//! shared arrangement for everyone who has none.

use laterite_core::query::{bind_values, build, on_conflict_ignore};
use laterite_core::sea_query::{Alias, Expr, Query};
use laterite_core::{sqlx, AnyRowExt, Db};
use serde::{Deserialize, Serialize};

use crate::dashboard::{Definition, COLUMNS};

/// The narrowest a widget may be made.
pub const MIN_WIDTH: u8 = 3;
/// The user id the shared arrangement is kept under.
pub const EVERYONE: i64 = 0;

/// A widget's place in a row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placed {
    /// The widget's code.
    pub code: String,
    /// The columns of 20 it spans.
    pub width: u8,
}

/// An arrangement of a dashboard's widgets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    /// The rows, each a list of widgets.
    #[serde(default)]
    pub rows: Vec<Vec<Placed>>,
    /// Widgets taken off the dashboard.
    #[serde(default)]
    pub hidden: Vec<String>,
}

impl Layout {
    /// Reads an arrangement an editor sent, kept to what `definition` has:
    /// unknown widgets dropped, a widget placed once, widths within the grid,
    /// and a row split where it would pass 20 columns.
    pub fn checked(json: &str, definition: &Definition) -> Result<Self, String> {
        let sent: Layout = laterite_core::serde_json::from_str(json).map_err(|e| e.to_string())?;
        let known = |code: &str| definition.reports.iter().any(|(c, _)| c == code);
        let mut seen: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Placed>> = Vec::new();
        for row in sent.rows {
            let mut current: Vec<Placed> = Vec::new();
            let mut used: u16 = 0;
            for mut placed in row {
                if !known(&placed.code) || seen.contains(&placed.code) {
                    continue;
                }
                placed.width = placed.width.clamp(MIN_WIDTH, COLUMNS);
                if used + u16::from(placed.width) > u16::from(COLUMNS) && !current.is_empty() {
                    rows.push(std::mem::take(&mut current));
                    used = 0;
                }
                used += u16::from(placed.width);
                seen.push(placed.code.clone());
                current.push(placed);
            }
            if !current.is_empty() {
                rows.push(current);
            }
        }
        let hidden = sent
            .hidden
            .into_iter()
            .filter(|code| known(code) && !seen.contains(code))
            .collect();
        Ok(Self { rows, hidden })
    }
}

impl Definition {
    /// The definition arranged as `layout` says: its rows and widths applied,
    /// its hidden widgets left out, and any widget it does not mention (one
    /// added to the dashboard since) kept after the rest in its own place.
    pub fn arranged(&self, layout: &Layout) -> Definition {
        let mut reports = Vec::with_capacity(self.reports.len());
        let mut row_number: u32 = 0;
        for row in &layout.rows {
            row_number += 1;
            for placed in row {
                if let Some((code, spec)) = self.reports.iter().find(|(c, _)| *c == placed.code) {
                    let mut spec = spec.clone();
                    spec.row = Some(row_number);
                    spec.width = Some(placed.width);
                    reports.push((code.clone(), spec));
                }
            }
        }
        let placed = |code: &str| reports.iter().any(|(c, _): &(String, _)| c == code);
        let rest: Vec<_> = self
            .reports
            .iter()
            .filter(|(code, _)| !placed(code) && !layout.hidden.contains(code))
            .cloned()
            .collect();
        let mut last = None;
        for (code, mut spec) in rest {
            // Widgets that shared a row keep sharing one.
            if last != Some(spec.row) {
                row_number += 1;
                last = Some(spec.row);
            }
            spec.row = Some(row_number);
            reports.push((code, spec));
        }
        Definition { reports }
    }
}

async fn stored(db: &Db, dashboard: &str, user: i64) -> Result<Option<Layout>, sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Alias::new("layout"))
            .from(Alias::new("report_layouts"))
            .and_where(Expr::col(Alias::new("dashboard")).eq(dashboard))
            .and_where(Expr::col(Alias::new("user_id")).eq(user))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(match row {
        Some(row) => laterite_core::serde_json::from_str(&row.get_text("layout")?).ok(),
        None => None,
    })
}

/// The arrangement `user` sees: their own, else the shared one, else none.
/// The flag says whether it is their own.
pub async fn of(
    db: &Db,
    dashboard: &str,
    user: i64,
) -> Result<(Option<Layout>, bool), sqlx::Error> {
    if let Some(own) = stored(db, dashboard, user).await? {
        return Ok((Some(own), true));
    }
    Ok((stored(db, dashboard, EVERYONE).await?, false))
}

/// Keeps `layout` as `user`'s arrangement; [`EVERYONE`] keeps the shared one.
pub async fn save(
    db: &Db,
    dashboard: &str,
    user: i64,
    layout: &Layout,
    now: &str,
) -> Result<(), sqlx::Error> {
    let json = laterite_core::serde_json::to_string(layout).unwrap_or_else(|_| "{}".into());
    // Built first: a statement builder must not live across an await.
    let insert = build(
        db.backend,
        Query::insert()
            .into_table(Alias::new("report_layouts"))
            .columns(["dashboard", "user_id", "layout", "updated_at"].map(Alias::new))
            .values_panic([
                dashboard.into(),
                user.into(),
                json.as_str().into(),
                now.into(),
            ])
            .on_conflict(on_conflict_ignore(["dashboard", "user_id"].map(Alias::new)))
            .to_owned(),
    );
    let update = build(
        db.backend,
        Query::update()
            .table(Alias::new("report_layouts"))
            .value(Alias::new("layout"), json.as_str())
            .value(Alias::new("updated_at"), now)
            .and_where(Expr::col(Alias::new("dashboard")).eq(dashboard))
            .and_where(Expr::col(Alias::new("user_id")).eq(user))
            .to_owned(),
    );
    for (sql, values) in [insert, update] {
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await?;
    }
    Ok(())
}

/// Forgets `user`'s own arrangement, so they see the shared one again.
pub async fn forget(db: &Db, dashboard: &str, user: i64) -> Result<(), sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(Alias::new("report_layouts"))
            .and_where(Expr::col(Alias::new("dashboard")).eq(dashboard))
            .and_where(Expr::col(Alias::new("user_id")).eq(user))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}
