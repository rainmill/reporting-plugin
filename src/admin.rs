//! The Reports screen: the dashboards an operator may open, each a board
//! over the chosen period.

use std::collections::HashMap;
use std::sync::Arc;

use laterite_admin::axum::extract::Query;
use laterite_admin::axum::http::StatusCode;
use laterite_admin::axum::response::{IntoResponse, Response};
use laterite_admin::axum::routing::get;
use laterite_admin::axum::{Extension, Router};
use laterite_admin::board::{Board, BoardCx, Layout, Placement, COLUMNS};
use laterite_admin::chart::CHART_ASSETS;
use laterite_admin::html::Markup;
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::{AdminRegistry, AuthenticatedUser, PageHead, Permission, Shell};
use laterite_core::{t, Registry};

use crate::dashboard::{self, DashboardReg, Definition, View};
use crate::draw::escape;
use crate::kinds;
use crate::{CSS, MANAGE, VIEW};

pub(crate) fn register(registry: &mut Registry) {
    let group = || t!("Reports");
    registry.add_permission(
        Permission::new(VIEW, t!("Open report dashboards"), group())
            .roles([laterite_admin::ROLE_ADMIN]),
    );
    registry.add_permission(
        Permission::new(
            MANAGE,
            t!("Arrange report dashboards, for oneself and for everyone"),
            group(),
        )
        .roles([laterite_admin::ROLE_ADMIN]),
    );
    kinds::register(registry);
    registry.add_screen(
        ScreenReg::new("/reports", VIEW, Arc::new(Reports))
            .in_menu(t!("Reports"))
            .menu(
                laterite_admin::nav::Placement::new()
                    .icon("chart-line")
                    .order(15),
            ),
    );
}

/// The board a declared dashboard starts as: each widget a report kind with
/// its written settings, arranged by anyone who may manage reports.
pub fn board_of(reg: &DashboardReg) -> Result<Board, String> {
    let definition = Definition::parse(reg.yaml)?;
    let rows = definition
        .rows()
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(code, spec)| {
                    let written = definition
                        .written
                        .iter()
                        .find(|(c, _)| c == code)
                        .map(|(_, value)| kinds::settings_of(value))
                        .unwrap_or_default();
                    Placement::new(
                        code.clone(),
                        kinds::code_of(spec.kind),
                        spec.width.unwrap_or(COLUMNS).clamp(1, COLUMNS),
                    )
                    .properties(written)
                })
                .collect()
        })
        .collect();
    let mut board = Board::new(
        format!("{}.{}", kinds::PREFIX, reg.code),
        Layout::rows(rows),
    )
    .edit_permission(MANAGE)
    .share_permission(MANAGE)
    .offer([format!("{}.*", kinds::PREFIX)])
    .carry(["range", "from", "to", "compare", "refresh"]);
    if let Some(permission) = reg.permission {
        board = board.view_permission(permission);
    }
    Ok(board)
}

struct Reports;

impl Screen for Reports {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let list_cx = ctx.clone();
        let mut router = Router::new().route(
            "/",
            get(move |Extension(shell): Extension<Shell>| {
                let ctx = list_cx.clone();
                async move { list(&ctx, &shell) }
            }),
        );
        for reg in ctx.contributions::<DashboardReg>() {
            let board = match board_of(reg) {
                Ok(board) => Arc::new(board),
                // An invalid definition has no page; the list still names the rest.
                Err(_) => {
                    continue;
                }
            };
            let base = format!("{}/{}/board", ctx.base_path(), reg.code);
            let (page, page_ctx, code) = (board.clone(), ctx.clone(), reg.code);
            router = router
                .route(
                    &format!("/{}", reg.code),
                    get(
                        move |Query(query): Query<HashMap<String, String>>,
                              Extension(shell): Extension<Shell>,
                              Extension(user): Extension<AuthenticatedUser>| {
                            let (board, ctx, base) = (page.clone(), page_ctx.clone(), base.clone());
                            async move { show(&ctx, &board, &base, code, &shell, &user, &query).await }
                        },
                    ),
                )
                .nest_service(&format!("/{}/board", reg.code), board.routes(ctx));
        }
        router
    }
}

/// The dashboards this operator may open.
fn visible<'a>(ctx: &'a RouteCtx, shell: &Shell) -> Vec<&'a DashboardReg> {
    ctx.contributions::<DashboardReg>()
        .into_iter()
        .filter(|d| d.permission.is_none_or(|p| shell.allows(p)))
        .collect()
}

fn list(ctx: &RouteCtx, shell: &Shell) -> Response {
    let cards: String = visible(ctx, shell)
        .iter()
        .map(|d| {
            format!(
                r#"<a class="rp-card" href="{base}/{code}"><span class="rp-figure__icon">{icon}</span><b>{name}</b></a>"#,
                base = escape(ctx.base_path()),
                code = escape(d.code),
                icon = shell.icon(d.icon),
                name = escape(&shell.tt(&d.name)),
            )
        })
        .collect();
    let body = if cards.is_empty() {
        format!(
            r#"<p class="rp-empty">{}</p>"#,
            escape(&shell.t("No dashboards yet. A plugin with a data source adds its own."))
        )
    } else {
        format!(r#"<div class="rp-cards">{cards}</div>"#)
    };
    shell.page_with(
        PageHead::new(t!("Reports")).assets(&[CSS]),
        Markup::raw_unchecked(body),
    )
}

async fn show(
    ctx: &RouteCtx,
    board: &Board,
    base: &str,
    code: &str,
    shell: &Shell,
    user: &AuthenticatedUser,
    query: &HashMap<String, String>,
) -> Response {
    let Some(reg) = visible(ctx, shell).into_iter().find(|d| d.code == code) else {
        return (StatusCode::NOT_FOUND, shell.t("No such dashboard.")).into_response();
    };
    let view = View::from_query(query, dashboard::today());
    let cx = BoardCx {
        ctx,
        shell,
        user,
        query,
        width: COLUMNS,
    };
    let body = format!(
        "{}{}",
        dashboard::period_bar(shell, &view),
        board.body(&cx, base).await.as_str()
    );
    let mut assets: Vec<&'static str> = CHART_ASSETS.to_vec();
    assets.push(CSS);
    for asset in board.assets(ctx) {
        if !assets.contains(&asset) {
            assets.push(asset);
        }
    }
    shell.page_with(
        PageHead::new(reg.name.clone())
            .back(t!("Reports"), ctx.base_path().to_string())
            .assets(&assets),
        Markup::raw_unchecked(body),
    )
}
