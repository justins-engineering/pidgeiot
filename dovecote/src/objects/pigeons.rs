use capsules::{Pigeon, PigeonMessage};
use worker::{
  DurableObject, Env, Request, Response, ResponseBuilder, Result, SqlStorage, State, console_error,
  durable_object, wasm_bindgen,
};

#[durable_object]
pub struct Pigeons {
  sql: SqlStorage,
  #[allow(unused)]
  state: State,
  #[allow(unused)]
  env: Env,
}

impl DurableObject for Pigeons {
  fn new(state: State, env: Env) -> Pigeons {
    let sql = state.storage().sql();
    sql
      .exec(
        "CREATE TABLE IF NOT EXISTS pigeons (
          id INTEGER NOT NULL PRIMARY KEY,
          flock_id INTEGER NOT NULL,
          serial TEXT,
          name TEXT,
          tags TEXT,
          connector TEXT,
          location TEXT,
          last_connected INTEGER,
          updated_at INTEGER DEFAULT (unixepoch()),
          created_at INTEGER DEFAULT (unixepoch())
        );

        CREATE TRIGGER IF NOT EXISTS prevent_immutable_updates_on_pigeons
        BEFORE UPDATE OF id, created_at ON pigeons
        WHEN OLD.id IS NOT NEW.id
          OR OLD.created_at IS NOT NEW.created_at
        BEGIN
          SELECT RAISE(ABORT, 'Error: id and created_at columns are immutable.');
        END;

        CREATE TRIGGER IF NOT EXISTS set_updated_at
        AFTER UPDATE ON pigeons
        FOR EACH ROW
        WHEN NEW.updated_at = OLD.updated_at
        BEGIN
          UPDATE pigeons
          SET updated_at = unixepoch()
          WHERE id = OLD.id;
        END;

        CREATE INDEX IF NOT EXISTS idx_pigeons_flock_id ON pigeons(flock_id);
        CREATE INDEX IF NOT EXISTS idx_pigeons_serial ON pigeons(serial);
        CREATE INDEX IF NOT EXISTS idx_pigeons_name ON pigeons(name);
        CREATE INDEX IF NOT EXISTS idx_pigeons_tags ON pigeons(tags);
        CREATE INDEX IF NOT EXISTS idx_pigeons_last_connected ON pigeons(last_connected DESC);",
        None,
      )
      .expect("created pigeons table");

    sql
      .exec(
        "CREATE TABLE IF NOT EXISTS pigeon_messages (
          id INTEGER NOT NULL PRIMARY KEY,
          pigeon_id INTEGER NOT NULL,
          message TEXT NOT NULL,
          timestamp INTEGER DEFAULT (unixepoch()),
          FOREIGN KEY (pigeon_id) REFERENCES pigeons(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_pigeon_messages_pigeon_id ON pigeon_messages(pigeon_id);
        CREATE INDEX IF NOT EXISTS idx_pigeon_messages_timestamp ON pigeon_messages(timestamp DESC);
        CREATE VIRTUAL TABLE IF NOT EXISTS pigeon_messages_fts USING fts5(message);",
        None,
      )
      .expect("created pigeon_messages table");

    Pigeons { sql, state, env }
  }

  async fn fetch(&self, req: Request) -> Result<Response> {
    match req.path().as_str() {
      "/pigeons/list" => list(self, req).await,
      "/pigeons/get" => get(self, req).await,
      "/pigeons/create/" => create(self, req).await,
      "/pigeons/update" => update(self, req).await,
      "/pigeons/delete" => delete(self, req).await,
      "/pigeon_messages/list" => list_pigeon_messages(self, req).await,
      "/pigeon_messages/get" => get_pigeon_messages(self, req).await,
      "/pigeon_messages/create" => create_pigeon_messages(self, req).await,
      _ => Response::error("Not found", 404),
    }
  }
}

async fn list(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  match req.text().await {
    Ok(flock_id) => {
      let query: std::result::Result<Vec<Pigeon>, worker::Error> = pigeons
        .sql
        .exec(
          "SELECT
          pigeons.*,
          COUNT(pigeon_messages.id) AS pigeon_count
        FROM
          pigeons
        LEFT JOIN
          pigeon_messages ON pigeons.id = pigeon_messages.pigeon_id
        WHERE
          pigeons.flock_id = ?
        GROUP BY
          pigeons.id;",
          vec![flock_id.into()],
        )?
        .to_array::<Pigeon>();

      match query {
        Ok(rows) => Response::from_json(&rows),
        Err(e) => {
          console_error!("Pigeons READ error: {e}");
          Response::error("Internal Server Error", 500)
        }
      }
    }
    Err(e) => {
      console_error!("Pigeons READ error: {e}");
      Response::error("Bad Request", 400)
    }
  }
}

async fn get(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  match req.text().await {
    Ok(pigeon_id) => {
      let query = pigeons
        .sql
        .exec(
          "SELECT * FROM pigeons WHERE id = ?;",
          vec![pigeon_id.into()],
        )?
        .one::<Pigeon>();

      match query {
        Ok(pigeon) => Response::from_json(&pigeon),
        Err(e) => {
          console_error!(
            "Pigeons read error: {e}\nRequest body: {:?}",
            req.text().await?
          );
          Response::error("Internal Server Error", 500)
        }
      }
    }
    Err(e) => {
      console_error!("Pigeons READ error: {e}");
      Response::error("Bad Request", 400)
    }
  }
}

async fn create(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  match req.json::<Pigeon>().await {
    Ok(row) => {
      let last_connected_timestamp: Option<i64> = row.last_connected.map(|dt| dt.unix_timestamp());
      let query = pigeons
        .sql
        .exec(
          "INSERT INTO pigeons (
            flock_id,
            serial,
            name,
            tags,
            connector,
            location,
            last_connected
          )
          VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING *;",
          vec![
            row.flock_id.into(),
            row.serial.into(),
            row.name.into(),
            row.tags.into(),
            row.connector.into(),
            row.location.into(),
            last_connected_timestamp.into(),
          ],
        )?
        .one::<Pigeon>();

      match query {
        Ok(pigeon) => {
          let mut location = String::with_capacity(72);
          location.push_str("/pigeons/");
          location.push_str(&pigeon.id.to_string());

          ResponseBuilder::new()
            .with_status(201)
            .with_header("Location", &location)?
            .from_json(&pigeon)
        }
        Err(e) => {
          console_error!(
            "Pigeons create error: {e}\nRequest body: {:?}",
            req.text().await?
          );
          Response::error("Internal Server Error", 500)
        }
      }
    }
    Err(e) => {
      console_error!("Pigeons CREATE error: {e}");
      Response::error("Bad Request", 400)
    }
  }
}

async fn update(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  match req.json::<Pigeon>().await {
    Ok(row) => {
      let last_connected_timestamp: Option<i64> = row.last_connected.map(|dt| dt.unix_timestamp());
      let query = pigeons
        .sql
        .exec(
          "UPDATE pigeons SET
          serial=?,
          name=?,
          tags=?,
          connector=?,
          location=?,
          last_connected=?
          WHERE id = ?
          RETURNING *;",
          vec![
            row.serial.into(),
            row.name.into(),
            row.tags.into(),
            row.connector.into(),
            row.location.into(),
            last_connected_timestamp.into(),
            row.id.into(),
          ],
        )?
        .one::<Pigeon>();

      match query {
        Ok(pigeon) => Response::from_json(&pigeon),
        Err(e) => {
          console_error!(
            "Pigeons create error: {e}\nRequest body: {:?}",
            req.text().await?
          );
          Response::error("Internal Server Error", 500)
        }
      }
    }
    Err(e) => {
      console_error!("Pigeons UPDATE error: {e}");
      Response::error("Bad Request", 400)
    }
  }
}

async fn delete(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  match req.json::<Pigeon>().await {
    Ok(row) => {
      pigeons
        .sql
        .exec("DELETE FROM pigeons WHERE id = ?;", vec![row.id.into()])?;

      Response::empty()
    }
    Err(e) => {
      console_error!("Pigeons DELETE error: {e}");
      Response::error("Bad Request", 400)
    }
  }
}

async fn list_pigeon_messages(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  match req.json::<Pigeon>().await {
    Ok(row) => {
      let query: std::result::Result<Vec<PigeonMessage>, worker::Error> = pigeons
        .sql
        .exec(
          "SELECT * FROM pigeon_messages WHERE pigeon_id = ?;",
          vec![row.id.into()],
        )?
        .to_array::<PigeonMessage>();

      match query {
        Ok(rows) => Response::from_json(&rows),
        Err(e) => {
          console_error!("PigeonMessages READ error: {e}");
          Response::error("Internal Server Error", 500)
        }
      }
    }
    Err(e) => {
      console_error!("PigeonMessages READ error: {e}");
      Response::error("Bad Request", 400)
    }
  }
}

async fn get_pigeon_messages(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  match req.json::<PigeonMessage>().await {
    Ok(row) => {
      let query = pigeons
        .sql
        .exec(
          "SELECT * FROM pigeon_messages WHERE id = ?;",
          vec![row.id.into()],
        )?
        .one::<PigeonMessage>();

      match query {
        Ok(pigeon_message) => Response::from_json(&pigeon_message),
        Err(e) => {
          console_error!(
            "PigeonMessages read error: {e}\nRequest body: {:?}",
            req.text().await?
          );
          Response::error("Internal Server Error", 500)
        }
      }
    }
    Err(e) => {
      console_error!("PigeonMessages READ error: {e}");
      Response::error("Bad Request", 400)
    }
  }
}

async fn create_pigeon_messages(pigeons: &Pigeons, mut req: Request) -> Result<Response> {
  match req.json::<PigeonMessage>().await {
    Ok(row) => {
      let query = pigeons
        .sql
        .exec(
          "INSERT INTO pigeon_messages (pigeon_id, message) VALUES (?, ?) RETURNING *;",
          vec![row.pigeon_id.into(), row.message.into()],
        )?
        .one::<PigeonMessage>();

      match query {
        Ok(pigeon_message) => {
          let mut location = String::with_capacity(72);
          location.push_str("/pigeon_messages/");
          location.push_str(&pigeon_message.id.to_string());

          ResponseBuilder::new()
            .with_status(201)
            .with_header("Location", &location)?
            .from_json(&pigeon_message)
        }
        Err(e) => {
          console_error!(
            "PigeonMessages create error: {e}\nRequest body: {:?}",
            req.text().await?
          );
          Response::error("Internal Server Error", 500)
        }
      }
    }
    Err(e) => {
      console_error!("PigeonMessages CREATE error: {e}");
      Response::error("Bad Request", 400)
    }
  }
}
