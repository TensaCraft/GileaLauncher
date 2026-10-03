//! A server's status as the game's multiplayer list shows it: the Server List Ping (1.7 and
//! later) — a handshake, a status request answered with JSON, and a ping for the round trip.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use launcher_shared::recent::{ServerStatus, parse_motd};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The whole exchange, the SRV lookup included; a server slower than this «Не відповідає».
pub const TIMEOUT: Duration = Duration::from_secs(3);
/// The port the game takes when an address names none; only then is an SRV record looked up.
pub const DEFAULT_PORT: u16 = 25565;
/// The largest status answer read (a favicon is a few tens of kilobytes).
const ANSWER_MAX: usize = 2 * 1024 * 1024;
/// "Whatever version you are": what a client that only asks for the status sends.
const ANY_PROTOCOL: i32 = -1;

fn put_varint(out: &mut Vec<u8>, value: i32) {
    let mut value = value as u32;
    loop {
        if value & !0x7f == 0 {
            out.push(value as u8);
            return;
        }
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
}

async fn read_varint(from: &mut (impl AsyncRead + Unpin)) -> std::io::Result<i32> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = from.read_u8().await?;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value as i32);
        }
    }
    Err(std::io::Error::other("a VarInt longer than five bytes"))
}

/// `body` (a packet id and its fields) with its length in front.
fn packet(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    put_varint(&mut out, body.len() as i32);
    out.extend_from_slice(body);
    out
}

/// The handshake that asks for the status of `host:port`, then the status request.
fn status_request(host: &str, port: u16) -> Vec<u8> {
    let mut handshake = vec![0x00];
    put_varint(&mut handshake, ANY_PROTOCOL);
    put_varint(&mut handshake, host.len() as i32);
    handshake.extend_from_slice(host.as_bytes());
    handshake.extend_from_slice(&port.to_be_bytes());
    put_varint(&mut handshake, 1);
    let mut out = packet(&handshake);
    out.extend(packet(&[0x00]));
    out
}

/// A packet's id and the rest of it.
async fn read_packet(from: &mut (impl AsyncRead + Unpin)) -> std::io::Result<(i32, Vec<u8>)> {
    let length = usize::try_from(read_varint(from).await?).map_err(std::io::Error::other)?;
    if length == 0 || length > ANSWER_MAX {
        return Err(std::io::Error::other(format!("a packet of {length} bytes")));
    }
    let mut body = vec![0; length];
    from.read_exact(&mut body).await?;
    let mut cursor = body.as_slice();
    let id = read_varint(&mut cursor).await?;
    Ok((id, cursor.to_vec()))
}

/// The status JSON of a status answer's body.
async fn status_json(body: &[u8]) -> std::io::Result<Value> {
    let mut cursor = body;
    let length = usize::try_from(read_varint(&mut cursor).await?).map_err(std::io::Error::other)?;
    let text = cursor.get(..length).ok_or_else(|| std::io::Error::other("a cut status answer"))?;
    serde_json::from_slice(text).map_err(std::io::Error::other)
}

/// What the list shows of a status answer, with the round trip `ping_ms`.
fn status_of(json: &Value, ping_ms: u32) -> ServerStatus {
    let count = |key: &str| json["players"][key].as_u64().map_or(0, |n| n.min(u64::from(u32::MAX)) as u32);
    let favicon = json["favicon"]
        .as_str()
        .filter(|f| f.starts_with("data:image/png;base64,"))
        .map(|f| f.replace(['\n', '\r'], ""));
    ServerStatus {
        motd: parse_motd(&json["description"]),
        online: count("online"),
        max: count("max"),
        version: json["version"]["name"].as_str().unwrap_or_default().to_string(),
        favicon,
        ping_ms,
    }
}

/// Asks the server at `address:port`, presenting itself for `host` (what the player typed).
async fn ask(host: &str, address: &str, port: u16) -> std::io::Result<ServerStatus> {
    let mut stream = TcpStream::connect((address, port)).await?;
    stream.set_nodelay(true)?;
    let asked = Instant::now();
    stream.write_all(&status_request(host, port)).await?;
    let (id, body) = read_packet(&mut stream).await?;
    if id != 0x00 {
        return Err(std::io::Error::other(format!("a status answer with id {id}")));
    }
    let json = status_json(&body).await?;
    let status_ms = asked.elapsed().as_millis();
    // The round trip of a ping; a server that closes after the status is timed by that answer.
    let pinged = Instant::now();
    let mut ping = vec![0x01];
    ping.extend_from_slice(&1i64.to_be_bytes());
    let ping_ms = match stream.write_all(&packet(&ping)).await {
        Ok(()) => match read_packet(&mut stream).await {
            Ok((0x01, _)) => pinged.elapsed().as_millis(),
            _ => status_ms,
        },
        Err(_) => status_ms,
    };
    Ok(status_of(&json, ping_ms.min(u128::from(u32::MAX)) as u32))
}

/// Where the game connects for `host:port`: an `_minecraft._tcp` SRV record of a name on the
/// default port, as the game looks it up; otherwise the address as it is.
async fn connect_to(host: &str, port: u16) -> (String, u16) {
    if port != DEFAULT_PORT || host.parse::<IpAddr>().is_ok() || host.eq_ignore_ascii_case("localhost") {
        return (host.to_string(), port);
    }
    let Ok(builder) = hickory_resolver::TokioResolver::builder_tokio() else {
        return (host.to_string(), port);
    };
    match builder.build().srv_lookup(format!("_minecraft._tcp.{host}.")).await {
        Ok(found) => found
            .iter()
            .min_by_key(|srv| srv.priority())
            .map(|srv| (srv.target().to_utf8().trim_end_matches('.').to_string(), srv.port()))
            .unwrap_or_else(|| (host.to_string(), port)),
        Err(_) => (host.to_string(), port),
    }
}

/// The status of the server the player joins as `host:port`, or `None` when it does not answer
/// within `TIMEOUT`.
pub async fn status(host: &str, port: u16) -> Option<ServerStatus> {
    status_within(host, port, TIMEOUT).await
}

async fn status_within(host: &str, port: u16, limit: Duration) -> Option<ServerStatus> {
    let asked = async {
        let (address, at) = connect_to(host, port).await;
        ask(host, &address, at).await
    };
    match tokio::time::timeout(limit, asked).await {
        Ok(Ok(status)) => Some(status),
        Ok(Err(error)) => {
            tracing::debug!("server {host}:{port} did not answer: {error}");
            None
        }
        Err(_) => {
            tracing::debug!("server {host}:{port} did not answer in {limit:?}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;

    /// A server on this machine that answers a status request with `answer` and, when `pongs`,
    /// a ping with its pong; it hands back the handshake's host and port.
    async fn fake_server(answer: Value, pongs: bool) -> (u16, tokio::task::JoinHandle<(String, u16)>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (id, body) = read_packet(&mut stream).await.unwrap();
            assert_eq!(id, 0x00, "a handshake first");
            let mut cursor = body.as_slice();
            assert_eq!(read_varint(&mut cursor).await.unwrap(), ANY_PROTOCOL);
            let length = read_varint(&mut cursor).await.unwrap() as usize;
            let host = String::from_utf8(cursor[..length].to_vec()).unwrap();
            let asked_port = u16::from_be_bytes([cursor[length], cursor[length + 1]]);
            assert_eq!(cursor[length + 2], 1, "asks for the status");
            assert_eq!(read_packet(&mut stream).await.unwrap(), (0x00, Vec::new()), "a status request");
            let text = answer.to_string();
            let mut body = vec![0x00];
            put_varint(&mut body, text.len() as i32);
            body.extend_from_slice(text.as_bytes());
            stream.write_all(&packet(&body)).await.unwrap();
            if pongs {
                let (id, payload) = read_packet(&mut stream).await.unwrap();
                assert_eq!(id, 0x01, "a ping");
                let mut pong = vec![0x01];
                pong.extend_from_slice(&payload);
                stream.write_all(&packet(&pong)).await.unwrap();
            }
            (host, asked_port)
        });
        (port, served)
    }

    #[tokio::test]
    async fn a_server_s_status_is_read_as_the_list_shows_it() {
        let answer = json!({
            "version": {"name": "Velocity 1.7.2-1.21.4", "protocol": 769},
            "players": {"max": 200, "online": 42},
            "description": {"text": "Tensa", "color": "aqua", "extra": [" §eсезон 3"]},
            "favicon": "data:image/png;base64,iVBOR\nw0KGgo="
        });
        let (port, served) = fake_server(answer, true).await;
        let status = status("127.0.0.1", port).await.expect("it answers");
        assert_eq!((status.online, status.max, status.version.as_str()), (42, 200, "Velocity 1.7.2-1.21.4"));
        assert_eq!(status.motd.iter().map(|s| s.text.as_str()).collect::<String>(), "Tensa сезон 3");
        assert_eq!(status.motd[0].color.as_deref(), Some("#55ffff"));
        assert_eq!(status.favicon.as_deref(), Some("data:image/png;base64,iVBORw0KGgo="));
        assert!(status.ping_ms < 3000);
        assert_eq!(served.await.unwrap(), ("127.0.0.1".to_string(), port));
    }

    #[tokio::test]
    async fn a_server_that_drops_the_ping_is_timed_by_its_status() {
        let (port, served) = fake_server(json!({"description": "§cold", "players": {}}), false).await;
        let status = status("127.0.0.1", port).await.expect("the status is enough");
        assert_eq!((status.online, status.max, status.favicon), (0, 0, None));
        assert_eq!(status.motd[0].color.as_deref(), Some("#ff5555"));
        served.await.unwrap();
    }

    #[tokio::test]
    async fn a_silent_or_closed_server_does_not_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let _kept = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        assert_eq!(status_within("127.0.0.1", port, Duration::from_millis(300)).await, None);
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        assert_eq!(status_within("127.0.0.1", closed_port, Duration::from_secs(2)).await, None);
    }

    #[tokio::test]
    async fn varints_go_both_ways() {
        for value in [0, 1, 127, 128, 25565, 2_097_151, i32::MAX, -1] {
            let mut bytes = Vec::new();
            put_varint(&mut bytes, value);
            assert_eq!(read_varint(&mut bytes.as_slice()).await.unwrap(), value);
        }
        let mut minus_one = Vec::new();
        put_varint(&mut minus_one, -1);
        assert_eq!(minus_one, [0xff, 0xff, 0xff, 0xff, 0x0f]);
    }
}
