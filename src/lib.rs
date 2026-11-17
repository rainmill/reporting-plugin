//! Rainmill Reporting: dashboards and reports for a Laterite application.
//!
//! A [`DataSource`] answers one question shape, a [`ReportQuery`]: a dimension,
//! some metrics, a period cut into groups, and optional filters, order and
//! limit. [`run`] asks it, through the cache, fills the gaps in a date series,
//! and runs the comparison period when one is asked for.
//!
//! ```ignore
//! registry.add(rainmill_reporting::DataSourceReg::new(Arc::new(MySource)));
//! let report = rainmill_reporting::run(&source, &cx, &query, Compare::Previous).await?;
//! ```

mod admin;
pub mod dashboard;
pub mod draw;
mod engine;
pub mod period;
mod source;
mod table;

pub use dashboard::DashboardReg;
pub use engine::{run, Report};
pub use period::{Compare, Group, Preset};
pub use source::{
    Aggregate, DataSource, DataSourceReg, Dimension, Filter, FilterOp, Metric, Order, OrderBy,
    Page, ReportCx, ReportData, ReportError, ReportQuery, Row, Unit,
};
pub use table::TableSource;

use laterite_admin::{AdminAsset, AdminRegistry};
use laterite_core::{Module, ModuleId, Registry};

/// The widgets' stylesheet, as an admin asset key.
pub const CSS: &str = "rainmill/reporting.css";
/// The permission to open report dashboards.
pub const VIEW: &str = "rainmill.reporting.view";
/// The permission to build and share report dashboards.
pub const MANAGE: &str = "rainmill.reporting.manage";

/// The module's identity.
pub const MODULE_ID: &str = "rainmill.reporting";

/// The plugin as a module, for the application's module list.
pub fn module() -> Box<dyn Module> {
    Box::new(ReportingModule)
}

/// The reporting plugin.
pub struct ReportingModule;

#[laterite_core::strata::async_trait]
impl Module for ReportingModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(MODULE_ID)
    }

    fn register(&self, registry: &mut Registry) {
        registry.add_asset(
            CSS,
            AdminAsset::css(include_bytes!("../assets/reporting.css")),
        );
        admin::register(registry);
    }
}
