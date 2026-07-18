mod hyperdrive;
pub use hyperdrive::get_db_client;
pub use hyperdrive::get_hyperdrive_conn;

mod auth;
pub use auth::authenticate_browser;

mod access;
pub use access::verify_cf_access;

mod flocks;
pub use flocks::create_user_flock;
pub use flocks::get_user_flocks;

mod pigeons;
pub use pigeons::delete_pigeon_pg_db;
pub use pigeons::insert_pigeon_pg_db;
pub use pigeons::proxy_binary_to_pigeon_do;
pub use pigeons::proxy_to_pigeon_do;
pub use pigeons::update_pigeon_pg_db;
pub use pigeons::update_shadow_pg_db;
pub use pigeons::update_telemetry_endpoint_pg_db;
pub use pigeons::upsert_acl_pg_db;
pub use pigeons::verify_device_via_do;

mod telemetry;
pub use telemetry::ensure_pigeons_telemetry_endpoint_column;
pub use telemetry::query_telemetry_history_for_flock;
pub use telemetry::query_telemetry_history_for_pigeon;
pub use telemetry::write_telemetry_history;

mod firmware;
pub use firmware::is_flock_owner;
pub use firmware::list_flock_firmware;
pub use firmware::sha256_hex;
pub use firmware::upsert_flock_firmware;
