use crate::client::ApiClient;
use crate::log_info;
use crate::models::ProfileDto;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

pub async fn run_signalr_client(api_client: ApiClient, tx: Sender<ProfileDto>) {
    let ws_url = {
        let base = api_client.server_url();
        let ws_base = if base.starts_with("https://") {
            format!("wss://{}", &base[8..])
        } else if base.starts_with("http://") {
            format!("ws://{}", &base[7..])
        } else {
            format!("ws://{}", base)
        };
        format!("{}/syncclipboardhub", ws_base)
    };

    loop {
        if let Err(e) = connect_and_listen(&ws_url, api_client.auth_header(), &tx).await {
            log_info(format!("[SignalR] 连接异常或断开: {}，将在 5 秒后重试...", e));
        }
        sleep(Duration::from_secs(5)).await;
    }
}

async fn connect_and_listen(
    ws_url: &str,
    auth_header: &str,
    tx: &Sender<ProfileDto>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut req = ws_url.into_client_request()?;
    req.headers_mut().insert(
        "Authorization",
        auth_header.parse()?,
    );

    let (ws_stream, _) = connect_async(req).await?;
    let (mut write, mut read) = ws_stream.split();

    // 1. 发送 SignalR 握手请求
    let handshake = "{\"protocol\":\"json\",\"version\":1}\u{1e}";
    write.send(Message::Text(handshake.into())).await?;

    // 2. 接收握手响应
    if let Some(msg) = read.next().await {
        let msg = msg?;
        if let Message::Text(txt) = msg {
            if !txt.contains("{}") && !txt.contains("\u{1e}") {
                return Err("SignalR 握手失败".into());
            }
        }
    }

    log_info("[SignalR] 实时长连接已建立成功");

    // 3. 消息循环与心跳维护
    let mut ping_interval = tokio::time::interval(Duration::from_secs(15));
    let mut rx_buffer = String::new();

    loop {
        tokio::select! {
            _ = ping_interval.tick() => {
                // 定时发送心跳消息
                let ping = "{\"type\":6}\u{1e}";
                if let Err(e) = write.send(Message::Text(ping.into())).await {
                    return Err(format!("发送心跳失败: {}", e).into());
                }
            }
            msg_opt = read.next() => {
                match msg_opt {
                    Some(Ok(Message::Text(text))) => {
                        rx_buffer.push_str(&text);
                        while let Some(pos) = rx_buffer.find('\u{1e}') {
                            let frame = rx_buffer[..pos].to_string();
                            rx_buffer = rx_buffer[pos + 1..].to_string();

                            if frame.is_empty() {
                                continue;
                            }

                            if let Ok(v) = serde_json::from_str::<Value>(&frame) {
                                // 检查消息类型
                                let msg_type = v.get("type").and_then(|t| t.as_i64()).unwrap_or(0);
                                if msg_type == 1 {
                                    // 目标调用
                                    if let Some(target) = v.get("target").and_then(|t| t.as_str()) {
                                        if target.eq_ignore_ascii_case("RemoteProfileChanged") {
                                            log_info("[SignalR] 收到 RemoteProfileChanged 实时广播推送");
                                            if let Some(args) = v.get("arguments").and_then(|a| a.as_array()) {
                                                if let Some(first) = args.first() {
                                                    match serde_json::from_value::<ProfileDto>(first.clone()) {
                                                        Ok(dto) => {
                                                            let _ = tx.send(dto).await;
                                                        }
                                                        Err(err) => {
                                                            log_info(format!("[SignalR] 解析 ProfileDto 失败: {}, 原始 json: {}", err, first));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else if msg_type == 6 {
                                    // 服务端心跳帧
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        write.send(Message::Pong(data)).await?;
                    }
                    Some(Ok(Message::Close(_))) => {
                        return Err("服务器关闭了 WebSocket 连接".into());
                    }
                    Some(Err(e)) => {
                        return Err(format!("WebSocket 读取错误: {}", e).into());
                    }
                    None => {
                        return Err("WebSocket 流已结束".into());
                    }
                    _ => {}
                }
            }
        }
    }
}
