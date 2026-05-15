use worker::{Response, console_error};

pub async fn get_stub(
  headers: &worker::Headers,
  ctx: &worker::RouteContext<()>,
  binding: &'static str,
) -> Result<worker::Stub, worker::Result<Response>> {
  let Ok(Some(user_id)) = headers.get("X-User-Id") else {
    console_error!("Request missing 'X-User-Id' header");
    return Err(Response::error("Unauthorized", 401));
  };

  let Ok(namespace) = ctx.durable_object(binding) else {
    console_error!("Failed to get DO namespace! Binding: {binding}");
    return Err(Response::error("Failed to get DO namespace", 500));
  };

  let Ok(object_id) = namespace.id_from_name(&user_id) else {
    console_error!("Failed to get DO object_id from user_id! Binding: {binding}");
    return Err(Response::error(
      "Failed to get DO object_id from user_id",
      500,
    ));
  };

  let Ok(stub) = object_id.get_stub() else {
    console_error!("Failed to get DO stub from object_id! Binding: {binding}");
    return Err(Response::error("Failed to get DO stub from object_id", 500));
  };

  Ok(stub)
}
