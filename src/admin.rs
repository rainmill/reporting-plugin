//! The Reports screen: the dashboards an operator may open, and each one over
//! the chosen period.

use std::collections::HashMap;
use std::sync::Arc;

use laterite_admin::axum::extract::{Path, Query};
use laterite_admin::axum::http::StatusCode;
use laterite_admin::axum::response::{IntoResponse, Response};
use laterite_admin::axum::routing::get;
use laterite_admin::axum::{Extension, Router};
use laterite_admin::chart::CHART_ASSETS;
use laterite_admin::html::Markup;
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::{AdminRegistry, PageHead, Permission, Shell};
use laterite_core::{t, Registry};

use crate::dashboard::{self, DashboardReg, Definition, View};
use crate::draw::escape;
use crate::{CSS, MANAGE, VIEW};

pub(crate) fn register(registry: &mut Registry) {
    let group = || t!("Reports");
    registry.add_permission(
        Permission::new(VIEW, t!("Open report dashboards"), group())
            .roles([laterite_admin::ROLE_ADMIN]),
    );
    registry.add_permission(
        Permission::new(MANAGE, t!("Build and share report dashboards"), group())
            .roles([laterite_admin::ROLE_ADMIN]),
    );
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

struct Reports;

impl Screen for Reports {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let (list_cx, show_cx) = (ctx.clone(), ctx.clone());
        Router::new()
            .route(
                "/",
                get(move |Extension(shell): Extension<Shell>| {
                    let ctx = list_cx.clone();
                    async move { list(&ctx, &shell) }
                }),
            )
            .route(
                "/{code}",
                get(
                    move |Path(code): Path<String>,
                          Query(query): Query<HashMap<String, String>>,
                          Extension(shell): Extension<Shell>| {
                        let ctx = show_cx.clone();
                        async move { show(&ctx, &shell, &code, &query).await }
                    },
                ),
            )
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
    shell: &Shell,
    code: &str,
    query: &HashMap<String, String>,
) -> Response {
    let Some(reg) = visible(ctx, shell).into_iter().find(|d| d.code == code) else {
        return (StatusCode::NOT_FOUND, shell.t("No such dashboard.")).into_response();
    };
    let definition = match Definition::parse(reg.yaml) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{}: {e}", shell.t("The dashboard's definition is invalid")),
            )
                .into_response()
        }
    };
    let view = View::from_query(query, dashboard::today());
    let body = dashboard::render(ctx, shell, &definition, &view).await;
    let mut assets: Vec<&'static str> = CHART_ASSETS.to_vec();
    assets.push(CSS);
    shell.page_with(
        PageHead::new(reg.name.clone())
            .back(t!("Reports"), ctx.base_path().to_string())
            .assets(&assets),
        body,
    )
}
