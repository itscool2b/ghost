//! The desktop's socket to the relay.

use std::time::Duration;

use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{client::IntoClientRequest, http::header::AUTHORIZATION},
};

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The largest message taken from the relay.
const MAX_FROM_RELAY: usize = 1024 * 1024;
const CONNECT_WITHIN: Duration = Duration::from_secs(20);

/// Opens this desktop's host socket. `relay` is a `wss://` origin; `ws://` is accepted only for
/// a relay on this machine, in tests and development.
pub async fn connect(relay: &str, host_id: &str, host_key: &str) -> Result<Socket, String> {
    let local = relay.starts_with("ws://127.0.0.1:") || relay.starts_with("ws://localhost:");
    if !relay.starts_with("wss://") && !local {
        return Err("The relay address is not secure.".into());
    }
    let mut request = format!("{relay}/api/remote/v1/host/{host_id}")
        .into_client_request()
        .map_err(|error| error.to_string())?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {host_key}")
            .parse()
            .map_err(|_| "The host key is not valid.".to_owned())?,
    );
    // The relay sends a browser's message (96 KiB at most) or a bot's call (256 KiB at most).
    // Nothing larger is taken from it, whatever it turns into.
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_FROM_RELAY))
        .max_frame_size(Some(MAX_FROM_RELAY));
    match tokio::time::timeout(
        CONNECT_WITHIN,
        tokio_tungstenite::connect_async_with_config(request, Some(config), false),
    )
    .await
    {
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(error)) => Err(reason(&error)),
        Err(_) => Err("The relay did not answer in time.".into()),
    }
}

/// A connection failure as one sentence for Settings.
fn reason(error: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Http(response) if response.status() == 401 => {
            "The relay did not recognise this computer.".into()
        }
        Error::Http(response) => format!("The relay answered {}.", response.status().as_u16()),
        Error::Io(_) | Error::Tls(_) => {
            "The relay could not be reached. Check the connection.".into()
        }
        other => other.to_string(),
    }
}
