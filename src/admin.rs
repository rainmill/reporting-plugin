//! The Reports screen: the dashboards an operator may open, and each one over
//! the chosen period.

use std::collections::HashMap;
use std::sync::Arc;

use laterite_admin::axum::extract::{Path, Query};
use laterite_admin::axum::http::StatusCode;
use laterite_admin::axum::response::{IntoResponse, Response};
use laterite_admin::axum::routing::{get, post};
use laterite_admin::axum::{Extension, Form, Router};
use laterite_admin::chart::CHART_ASSETS;
use laterite_admin::html::Markup;
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::{
    AdminRegistry, AuthenticatedUser, FlashLevel, PageHead, Permission, SessionHandle, Shell,
};
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
        let (list_cx, show_cx, widget_cx, layout_cx) =
            (ctx.clone(), ctx.clone(), ctx.clone(), ctx.clone());
        Router::new()
            .route(
                "/",
                get(move |Extension(shell): Extension<Shell>| {
                    let ctx = list_cx.clone();
                    async move { list(&ctx, &shell) }
                }),
            )
            .route(
                "/{code}/layout",
                post(
                    move |Path(code): Path<String>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>,
                          Extension(session): Extension<SessionHandle>,
                          Form(form): Form<HashMap<String, String>>| {
                        let ctx = layout_cx.clone();
                        async move { arrange(&ctx, &shell, &user, &session, &code, &form).await }
                    },
                ),
            )
            .route(
                "/{code}/widgets/{widget}",
                get(
                    move |Path((code, widget)): Path<(String, String)>,
                          Query(query): Query<HashMap<String, String>>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>| {
                        let ctx = widget_cx.clone();
                        async move { one(&ctx, &shell, &user, &code, &widget, &query).await }
                    },
                ),
            )
            .route(
                "/{code}",
                get(
                    move |Path(code): Path<String>,
                          Query(query): Query<HashMap<String, String>>,
                          Extension(shell): Extension<Shell>,
                          Extension(user): Extension<AuthenticatedUser>| {
                        let ctx = show_cx.clone();
                        async move { show(&ctx, &shell, &user, &code, &query).await }
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

/// The dashboard's definition as `user` arranged it, or as everyone sees it,
/// and whether the arrangement is their own.
async fn arranged(
    ctx: &RouteCtx,
    reg: &DashboardReg,
    user: &AuthenticatedUser,
) -> Result<(Definition, bool), String> {
    let definition = Definition::parse(reg.yaml)?;
    let (layout, personal) = crate::layout::of(ctx.db(), reg.code, user.user.id)
        .await
        .unwrap_or((None, false));
    Ok(match layout {
        Some(layout) => (definition.arranged(&layout), personal),
        None => (definition, false),
    })
}

/// Saves an arrangement: the viewer's own, the one everyone starts from, or
/// (reset) none of their own.
async fn arrange(
    ctx: &RouteCtx,
    shell: &Shell,
    user: &AuthenticatedUser,
    session: &SessionHandle,
    code: &str,
    form: &HashMap<String, String>,
) -> Response {
    let Some(reg) = visible(ctx, shell).into_iter().find(|d| d.code == code) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !shell.allows(MANAGE) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let back = (
        StatusCode::SEE_OTHER,
        [(
            laterite_admin::axum::http::header::LOCATION,
            format!("{}/{}", ctx.base_path(), reg.code),
        )],
    );
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let scope = form.get("scope").map(String::as_str).unwrap_or("mine");
    let done = if scope == "reset" {
        crate::layout::forget(ctx.db(), reg.code, user.user.id)
            .await
            .map_err(|e| e.to_string())
    } else {
        let checked = Definition::parse(reg.yaml).and_then(|definition| {
            crate::layout::Layout::checked(
                form.get("layout").map(String::as_str).unwrap_or_default(),
                &definition,
            )
        });
        match checked {
            Ok(layout) if scope == "default" => {
                // Their own goes too, so they see what everyone now sees.
                let saved =
                    crate::layout::save(ctx.db(), reg.code, crate::layout::EVERYONE, &layout, &now)
                        .await;
                let _ = crate::layout::forget(ctx.db(), reg.code, user.user.id).await;
                saved.map_err(|e| e.to_string())
            }
            Ok(layout) => crate::layout::save(ctx.db(), reg.code, user.user.id, &layout, &now)
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        }
    };
    match done {
        Ok(()) => {
            ctx.audit(
                user,
                "rainmill.reporting.dashboard.arrange",
                Some("report_layouts"),
                Some(reg.code),
                Some(&shell.tt(&reg.name)),
                Some(scope),
            )
            .await;
            session.push_flash(
                FlashLevel::Success,
                match scope {
                    "default" => t!("The layout was saved for everyone."),
                    "reset" => t!("Your own layout was removed."),
                    _ => t!("Your layout was saved."),
                },
            );
        }
        Err(e) => session.push_flash(
            FlashLevel::Error,
            laterite_core::Text::dynamic(format!("The layout could not be saved: {e}")),
        ),
    }
    back.into_response()
}

async fn show(
    ctx: &RouteCtx,
    shell: &Shell,
    user: &AuthenticatedUser,
    code: &str,
    query: &HashMap<String, String>,
) -> Response {
    let Some(reg) = visible(ctx, shell).into_iter().find(|d| d.code == code) else {
        return (StatusCode::NOT_FOUND, shell.t("No such dashboard.")).into_response();
    };
    let (definition, personal) = match arranged(ctx, reg, user).await {
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
    let widgets = format!("{}/{}/widgets", ctx.base_path(), reg.code);
    let layout = format!("{}/{}/layout", ctx.base_path(), reg.code);
    let page = dashboard::Page {
        widgets: &widgets,
        layout: &layout,
        editable: shell.allows(MANAGE),
        personal,
    };
    let body = dashboard::render(ctx, shell, &definition, &view, &page);
    let mut assets: Vec<&'static str> = CHART_ASSETS.to_vec();
    assets.push(CSS);
    assets.push(crate::JS);
    shell.page_with(
        PageHead::new(reg.name.clone())
            .back(t!("Reports"), ctx.base_path().to_string())
            .assets(&assets),
        body,
    )
}

/// One widget of a dashboard, as the page fetches it after opening.
async fn one(
    ctx: &RouteCtx,
    shell: &Shell,
    user: &AuthenticatedUser,
    code: &str,
    widget: &str,
    query: &HashMap<String, String>,
) -> Response {
    let Some(reg) = visible(ctx, shell).into_iter().find(|d| d.code == code) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok((definition, _)) = arranged(ctx, reg, user).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let view = View::from_query(query, dashboard::today());
    match dashboard::render_widget(ctx, shell, &definition, widget, &view).await {
        Some(markup) => (
            [(
                laterite_admin::axum::http::header::CACHE_CONTROL,
                "private, no-store",
            )],
            laterite_admin::axum::response::Html(markup.into_string()),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
