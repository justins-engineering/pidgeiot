pub fn parse_json_string(value: &Option<Option<serde_json::Value>>) -> String {
  match value {
    Some(Some(serde_json::Value::String(s))) => s.clone(),
    Some(Some(serde_json::Value::Number(n))) => n.to_string(),
    Some(Some(serde_json::Value::Bool(b))) => b.to_string(),
    Some(Some(serde_json::Value::Array(a))) => format!("{a:?}"),
    _ => String::new(),
  }
}

pub fn parse_json_bool(value: &Option<Option<serde_json::Value>>) -> bool {
  matches!(value, Some(Some(serde_json::Value::Bool(true))))
}
