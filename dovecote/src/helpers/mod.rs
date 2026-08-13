mod hyperdrive;
pub use hyperdrive::get_db_client;
pub use hyperdrive::get_hyperdrive_conn;

mod auth;
pub use auth::authenticate_browser;

mod access;
pub use access::verify_cf_access;

mod crypto;
pub use crypto::constant_time_eq;

// No `pub use` yet -- the checkout-session, customer-portal and
// meter-reporting routes that call this client are a later phase, and
// re-exporting names nothing imports would only add warnings.
mod stripe_api;

mod stripe_webhook;
pub use stripe_webhook::STRIPE_WEBHOOK_SECRET;
pub use stripe_webhook::StripeWebhookEvent;
pub use stripe_webhook::verify_webhook_signature;

mod billing;
pub use billing::WebhookClaim;
pub use billing::apply_subscription;
pub use billing::claim_webhook_event;
pub use billing::ensure_billing_tables;
pub use billing::mark_webhook_event_processed;

mod flocks;
pub use flocks::backfill_owner_email;
pub use flocks::create_user_flock;
pub use flocks::get_user_flocks;

mod pigeons;
pub use pigeons::PigeonAccess;
pub use pigeons::check_pigeon_authz;
pub use pigeons::delete_pigeon_pg_db;
pub use pigeons::grant_org_acl_via_do;
pub use pigeons::insert_pigeon_pg_db;
pub use pigeons::proxy_binary_to_pigeon_do;
pub use pigeons::proxy_to_pigeon_do;
pub use pigeons::proxy_websocket_to_pigeon_do;
pub use pigeons::psk_lookup_via_do;
pub use pigeons::update_pigeon_pg_db;
pub use pigeons::update_shadow_pg_db;
pub use pigeons::update_telemetry_endpoint_pg_db;
pub use pigeons::upsert_acl_pg_db;
pub use pigeons::verify_device_via_do;

mod demo;
pub use demo::is_demo_pigeon;

mod coap_service;
pub use coap_service::is_allowed_coap_service_ip;

mod telemetry;
pub use telemetry::TelemetryHistoryPage;
pub use telemetry::ensure_pigeons_telemetry_endpoint_column;
pub use telemetry::get_flock_pigeon_ids;
pub use telemetry::query_telemetry_history_for_flock;
pub use telemetry::query_telemetry_history_for_pigeon;
pub use telemetry::write_telemetry_history;

mod greptime;
pub use greptime::build_line_protocol;
pub use greptime::greptime_origin;
pub use greptime::post_line_protocol;
pub use greptime::query_greptime_history_for_pigeon;
pub use greptime::query_greptime_history_for_pigeons;
pub use greptime::url_encode_component;
pub use greptime::write_telemetry_default;

mod firmware;
pub use firmware::FlockAccess;
pub use firmware::get_firmware_board;
pub use firmware::list_flock_firmware;
pub use firmware::sha256_hex;
pub use firmware::upsert_flock_firmware;

mod ops_probe;
pub use ops_probe::probe_kratos_health;

mod feedback;
pub use feedback::send_feedback_email;

mod orgs;
pub use orgs::FlockAction;
pub use orgs::Principal;
pub use orgs::accept_invite;
pub use orgs::authorize_flock;
pub use orgs::build_invite_url;
pub use orgs::change_member_role;
pub use orgs::create_invite;
pub use orgs::create_organization;
pub use orgs::delete_organization_if_empty;
pub use orgs::ensure_org_tables;
pub use orgs::get_flock_with_pigeons;
pub use orgs::get_organization;
pub use orgs::list_org_invites;
pub use orgs::list_org_members;
pub use orgs::list_user_organizations;
pub use orgs::load_org_roles;
pub use orgs::mint_invite_token;
pub use orgs::org_role_of;
pub use orgs::remove_member;
pub use orgs::rename_organization;
pub use orgs::revoke_invite;
pub use orgs::send_invite_email;
pub use orgs::set_flock_org;

mod alerts;
pub use alerts::check_telemetry_alerts;
pub use alerts::create_flock_alert;
pub use alerts::create_pigeon_alert;
pub use alerts::delete_alert_definition;
pub use alerts::evaluate_scheduled_alerts;
pub use alerts::is_alert_owner;
pub use alerts::list_flock_alert_state;
pub use alerts::list_flock_alerts;
pub use alerts::list_pigeon_alert_state;
pub use alerts::list_pigeon_alerts;
pub use alerts::update_alert_definition;
