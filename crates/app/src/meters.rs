use futures_util::StreamExt;
use pipeweaver_smc_core::Engine;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use ulid::Ulid;

#[derive(Deserialize)]
struct MeterEvent {
    id: Ulid,
    percent: u8,
}

/// Subscribe to Pipeweaver's live meter stream and keep the latest percentage
/// for every node. Reconnects if Pipeweaver restarts.
pub async fn run(engine: Arc<Engine>) {
    loop {
        if let Ok((mut socket, _)) =
            tokio_tungstenite::connect_async("ws://localhost:14565/api/websocket/meter").await
        {
            log::info!("Meter stream connected");
            while let Some(message) = socket.next().await {
                match message {
                    Ok(Message::Text(text)) => {
                        if let Ok(event) = serde_json::from_str::<MeterEvent>(&text) {
                            engine.set_meter(event.id, event.percent);
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
