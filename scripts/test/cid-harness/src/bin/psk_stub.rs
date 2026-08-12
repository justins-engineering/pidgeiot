//! A stand-in for dovecote's `GET /internal/coap-psk/:identity` endpoint,
//! answering loft's PSK resolver with a single harness-minted credential.
//! std-only (no HTTP crate): the request surface is one line and the
//! response is fixed. The PSK it returns matches the cid_client's baked-in
//! key, so no real dovecote and no real credentials are involved anywhere
//! in the harness.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

const IDENTITY: &str = "cid-harness-pigeon";
const SECRET: &str = "0123456789abcdef0123456789abcdef";
const TOKEN: &str = "cid-harness-token";

fn main() {
  let addr = std::env::args()
    .nth(1)
    .unwrap_or_else(|| "127.0.0.1:8788".to_string());
  let listener = TcpListener::bind(&addr).expect("bind psk stub");
  eprintln!("psk_stub listening on {addr}");

  for stream in listener.incoming() {
    let Ok(mut stream) = stream else { continue };
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
      continue;
    }
    // Drain the rest of the headers so the client sees a clean close.
    let mut header = String::new();
    while reader
      .read_line(&mut header)
      .map(|n| n > 0)
      .unwrap_or(false)
    {
      if header == "\r\n" || header == "\n" {
        break;
      }
      header.clear();
    }

    // "GET /internal/coap-psk/<identity> HTTP/1.1"
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let wanted = format!("/internal/coap-psk/{IDENTITY}");
    let response = if path == wanted {
      let body =
        format!("{{\"identity\":\"{IDENTITY}\",\"secret\":\"{SECRET}\",\"token\":\"{TOKEN}\"}}");
      format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
      )
    } else {
      // 404 is loft's authoritative "no such identity" (negative-cacheable).
      "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
  }
}
