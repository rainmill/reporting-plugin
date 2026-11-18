//! A dashboard's saved arrangement: one a person, and one shared by everyone
//! who has none of their own.

use laterite_core::sea_query;
use laterite_core::strata::*;
use sea_query::Alias;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_layouts"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("report_layouts"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                // The dashboard's code.
                .col(key_col(Alias::new("dashboard")).not_null())
                // Whose it is; 0 is the arrangement everyone starts from.
                .col(ColumnDef::new(Alias::new("user_id")).big_integer().not_null().default(0))
                // The rows and what is hidden, as JSON.
                .col(ColumnDef::new(Alias::new("layout")).text().not_null())
                .col(ColumnDef::new(Alias::new("updated_at")).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("report_layouts_dashboard_user")
                .table(Alias::new("report_layouts"))
                .col(Alias::new("dashboard"))
                .col(Alias::new("user_id"))
                .unique()
                .to_owned(),
        )
        .await
    }
}
