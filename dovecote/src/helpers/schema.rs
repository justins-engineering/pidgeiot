//! The catalog read each `ensure_*` makes before it decides to run any DDL.

use tokio_postgres::{Client, types::Type};
use worker::{Error, Result, console_error};

/// Names what the database lacks among `relations` (tables and indexes) and `columns`
/// (`(table, column)` pairs), as `relation` or `table.column`; empty when all are present.
///
/// One catalog statement, so it takes no table lock where a no-op `IF NOT EXISTS` DDL statement
/// would, and `to_regclass` is STABLE, which keeps it out of Hyperdrive's query cache.
pub async fn missing_schema_objects(
  client: &Client,
  relations: &[&str],
  columns: &[(&str, &str)],
) -> Result<Vec<String>> {
  let (tables, names): (Vec<&str>, Vec<&str>) = columns.iter().copied().unzip();
  let rows = client
    .query_typed(
      "SELECT r.name AS missing FROM unnest($1::text[]) AS r(name)
        WHERE to_regclass(r.name) IS NULL
       UNION ALL
       SELECT c.tbl || '.' || c.col FROM unnest($2::text[], $3::text[]) AS c(tbl, col)
        WHERE NOT EXISTS (
          SELECT 1 FROM pg_attribute a
           WHERE a.attrelid = to_regclass(c.tbl) AND a.attname = c.col
             AND a.attnum > 0 AND NOT a.attisdropped
        );",
      &[
        (&relations, Type::TEXT_ARRAY),
        (&tables, Type::TEXT_ARRAY),
        (&names, Type::TEXT_ARRAY),
      ],
    )
    .await
    .map_err(|e| {
      console_error!("Schema catalog read failed for {relations:?} {columns:?}: {e}");
      Error::RustError("Internal Server Error".into())
    })?;
  Ok(rows.iter().map(|row| row.get("missing")).collect())
}
