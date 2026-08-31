mod navbar;
pub use navbar::Navbar;

mod footer;
pub use footer::Footer;

mod theme_controller;
pub use theme_controller::ThemeController;

mod ory_form_builder;
pub use ory_form_builder::FormBuilder;

pub mod ory_error;

mod ory_log_out;
pub use ory_log_out::OryLogOut;

mod alert;
pub use alert::Alert;

mod session_cookie;
pub use session_cookie::SetSessionCookie;

mod json_view;
pub use json_view::JsonViewer;

mod connector_badge;
pub use connector_badge::ConnectorBadge;

mod stat_tiles;
pub use stat_tiles::TelemetryStatTiles;

mod gauge_strip;
pub use gauge_strip::{GaugeReading, GaugeStrip};

mod occupancy_grid;
// Deliberately unwired: the fleet view that would feed it does not fetch
// per-pigeon telemetry yet, and the public demo serves a single pigeon so
// it cannot show this honestly. Exported and tested so the shape is settled
// before anything depends on it -- not dead code to be tidied away.
#[allow(unused_imports)]
pub use occupancy_grid::{OccupancyCell, OccupancyGrid};

mod telemetry_chart;
pub use telemetry_chart::{ChartKind, ChartReference, ChartSeries, TelemetryChart};

mod graph_widget;
pub use graph_widget::{FlockGraphs, GraphDef, PigeonGraphs};

mod track_widget;
pub use track_widget::TrackWidget;

mod telemetry_endpoint_modal;
pub use telemetry_endpoint_modal::TelemetryEndpointModal;

mod log_viewer;
pub use log_viewer::LogViewer;

mod firmware_modal;
pub use firmware_modal::FirmwareModal;

mod connection_badge;
pub use connection_badge::ConnectionBadge;

mod board_select;
pub use board_select::{BOARD_DATALIST_ID, BoardDatalist};

mod maturity_badge;
pub use maturity_badge::{Maturity, MaturityBadge};

mod alerts_panel;
pub use alerts_panel::{FlockAlerts, PigeonAlerts};

mod feedback_modal;
pub use feedback_modal::{FeedbackForm, FeedbackModal};

mod comparison;
pub use comparison::{ComparisonTable, ComparisonTables};

mod danger_zone;
pub use danger_zone::{DangerAction, DangerZone};
