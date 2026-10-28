#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod client;
mod clipboard;
mod config;
mod models;
mod settings_dialog;
mod signalr;
mod tray;

use chrono::Local;
use client::ApiClient;
use clipboard::{
    is_hash_processed, mark_hash_processed, ClipboardContent, ClipboardManager, IS_WRITING_CLIPBOARD,
};
use config::AppConfig;
use models::{ProfileDto, ProfileType};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::mpsc;
use tray::{TrayController, IS_PAUSED};

const MAX_LOG_SIZE: u64 = 1024 * 1024; // 1 MB 上限

pub fn log_info(msg: impl AsRef<str>) {
    let text = msg.as_ref();
    #[cfg(debug_assertions)]
    {
        let _ = eprintln!("{}", text);
    }
    use std::io::Write;
    let log_path = "syncclipboard.log";

    // 检查文件体积，超过 1MB 时自动轮转为 syncclipboard.log.bak
    if let Ok(meta) = std::fs::metadata(log_path) {
        if meta.len() >= MAX_LOG_SIZE {
            let backup_path = "syncclipboard.log.bak";
            let _ = std::fs::remove_file(backup_path);
            let _ = std::fs::rename(log_path, backup_path);
        }
    }

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(f, "[{}] {}", Local::now().format("%Y-%m-%d %H:%M:%S"), text);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::panic::set_hook(Box::new(|info| {
        log_info(format!("[PANIC] {:?}", info));
    }));
    log_info("[SyncClipboard Lite] 正在启动...");

    // 1. 读取配置文件
    let config = match AppConfig::load() {
        Ok(cfg) => {
            log_info(format!("[SyncClipboard Lite] 已成功加载配置: URL = {}", cfg.server_url));
            cfg
        }
        Err(err) => {
            log_info(format!("[SyncClipboard Lite] 加载配置失败: {}", err));
            log_info("[SyncClipboard Lite] 请确认 %APPDATA%\\SyncClipboard\\SyncClipboard.json 存在或在同级目录下提供 config.json");
            return Err(err);
        }
    };

    let mut current_config = config.clone();
    let mut api_client = ApiClient::new(&current_config);

    // 2. 初始化通道与托盘图标 (托盘窗口同时负责处理剪贴板原生事件与设置弹窗)
    let (manual_sync_tx, mut manual_sync_rx) = mpsc::unbounded_channel::<()>();
    let (clip_event_tx, mut clip_event_rx) = mpsc::unbounded_channel::<()>();
    let (settings_tx, mut settings_rx) = mpsc::unbounded_channel::<()>();
    let (config_update_tx, mut config_update_rx) = mpsc::unbounded_channel::<AppConfig>();

    let tray_ctrl = tray::start_tray(manual_sync_tx.clone(), clip_event_tx.clone(), settings_tx);
    tray_ctrl.update_status("正在连接");

    // 3. 启动基于 Windows 内核剪贴板序列号的毫秒级高可靠监听
    let seq_clip_tx = clip_event_tx.clone();
    tokio::spawn(async move {
        let mut last_seq = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        let mut interval = tokio::time::interval(Duration::from_millis(300));
        loop {
            interval.tick().await;
            if !IS_PAUSED.load(Ordering::SeqCst) && !IS_WRITING_CLIPBOARD.load(Ordering::SeqCst) {
                let cur_seq = unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() };
                if cur_seq != last_seq {
                    last_seq = cur_seq;
                    let _ = seq_clip_tx.send(());
                }
            }
        }
    });

    // 4. 启动 SignalR 实时长连接
    let (remote_push_tx, mut remote_push_rx) = mpsc::channel::<ProfileDto>(32);
    let signalr_client = api_client.clone();
    tokio::spawn(async move {
        signalr::run_signalr_client(signalr_client, remote_push_tx).await;
    });

    // 5. 启动后台定时轮询任务（兜底容灾）
    let poll_interval_sec = config.pull_interval_sec.max(2);
    let poll_sync_tx = manual_sync_tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(poll_interval_sec));
        loop {
            interval.tick().await;
            if !IS_PAUSED.load(Ordering::SeqCst) {
                let _ = poll_sync_tx.send(());
            }
        }
    });

    tray_ctrl.update_status("在线");
    log_info("[SyncClipboard Lite] 服务初始化就绪");

    // 6. 主事件处理循环
    loop {
        tokio::select! {
            // A. 本地剪贴板发生变化 -> 上传
            Some(_) = clip_event_rx.recv() => {
                if IS_PAUSED.load(Ordering::SeqCst) {
                    continue;
                }
                // 短暂延时等待剪贴板写入完成释放
                tokio::time::sleep(Duration::from_millis(150)).await;

                if let Some(content) = ClipboardManager::read_clipboard() {
                    let hash = content.compute_hash();
                    log_info(format!("[Select] 本地剪贴板读到内容, hash = {}", hash));
                    if is_hash_processed(&hash) {
                        log_info("[Select] 本地内容为近期已处理内容，跳过");
                        continue;
                    }
                    mark_hash_processed(&hash);

                    log_info("[Select] 准备更新托盘状态为 正在上传");
                    tray_ctrl.update_status("正在上传");
                    log_info("[Select] 开始执行 handle_local_upload");
                    if let Err(e) = handle_local_upload(&api_client, content, &hash).await {
                        log_info(format!("[Upload] 上传失败: {}", e));
                        tray_ctrl.update_status("上传失败");
                    } else {
                        log_info("[Upload] 上传成功，恢复托盘在线状态");
                        tray_ctrl.update_status("在线");
                    }
                } else {
                    log_info("[Select] read_clipboard 返回 None");
                }
            }

            // B. 远端 SignalR 实时推送 -> 下载
            Some(remote_dto) = remote_push_rx.recv() => {
                log_info(format!("[Select] 远端推送唤醒: {:?}", remote_dto.r#type));
                if !IS_PAUSED.load(Ordering::SeqCst) {
                    handle_remote_profile(&api_client, remote_dto, &tray_ctrl).await;
                }
            }

            // C. 定时轮询或托盘手动同步 -> 拉取检查
            Some(_) = manual_sync_rx.recv() => {
                if !IS_PAUSED.load(Ordering::SeqCst) {
                    match api_client.get_sync_profile().await {
                        Ok(dto) => {
                            handle_remote_profile(&api_client, dto, &tray_ctrl).await;
                        }
                        Err(_e) => {}
                    }
                }
            }

            // D. 弹出配置设置对话框
            Some(_) = settings_rx.recv() => {
                log_info("[Tray] 用户打开了配置设置对话框");
                settings_dialog::show_settings_dialog(current_config.clone(), config_update_tx.clone());
            }

            // E. 配置已在界面中更新并保存
            Some(new_cfg) = config_update_rx.recv() => {
                log_info(format!("[Config] 收到新配置应用请求: URL = {}", new_cfg.server_url));
                current_config = new_cfg;
                api_client = ApiClient::new(&current_config);
                tray_ctrl.update_status("配置已更新");
                // 立即触发一次拉取
                let _ = manual_sync_tx.send(());
            }
        }
    }
    #[allow(unreachable_code)]
    {
        log_info("[Main] 主循环已退出");
        Ok(())
    }
}

async fn handle_local_upload(
    client: &ApiClient,
    content: ClipboardContent,
    _hash: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    log_info("[handle_local_upload] 进入上传处理流程");
    match content {
        ClipboardContent::Text(text) => {
            log_info(format!("[handle_local_upload] 处理文本内容, 长度: {}", text.len()));
            let text_hash = ClipboardContent::compute_sha256(text.as_bytes());
            mark_hash_processed(&text_hash);
            let dto = ProfileDto {
                r#type: ProfileType::Text,
                hash: text_hash,
                text,
                has_data: false,
                data_name: None,
                transfer_data_hash: None,
                size: None,
                synced_at: Some(chrono::Utc::now()),
            };
            log_info("[handle_local_upload] 正在发起 put_sync_profile HTTP 请求...");
            client.put_sync_profile(&dto).await?;
            log_info("[Upload] 本地文本已成功同步");
        }
        ClipboardContent::Image(png_bytes) => {
            log_info(format!("[handle_local_upload] 处理图片内容, 字节数: {}", png_bytes.len()));
            let file_name = format!("Image_{}.png", Local::now().format("%Y%m%d%H%M%S"));
            let transfer_data_hash = ClipboardContent::compute_sha256(&png_bytes);
            let combined = format!("{}|{}", file_name, transfer_data_hash.to_uppercase());
            let profile_hash = ClipboardContent::compute_sha256(combined.as_bytes());
            mark_hash_processed(&profile_hash);
            mark_hash_processed(&transfer_data_hash);
            let size = png_bytes.len() as i64;

            // 1. 上传图片文件
            log_info(format!("[handle_local_upload] 正在上传图片文件 {}...", file_name));
            client.put_file(&file_name, png_bytes).await?;

            // 2. 上传元数据 ProfileDto
            let dto = ProfileDto {
                r#type: ProfileType::Image,
                hash: profile_hash,
                text: file_name.clone(),
                has_data: true,
                data_name: Some(file_name),
                transfer_data_hash: Some(transfer_data_hash),
                size: Some(size),
                synced_at: Some(chrono::Utc::now()),
            };
            log_info("[handle_local_upload] 正在上传图片 ProfileDto...");
            client.put_sync_profile(&dto).await?;
            log_info("[Upload] 本地图片已成功同步");
        }
    }
    Ok(())
}

async fn handle_remote_profile(
    client: &ApiClient,
    dto: ProfileDto,
    tray_ctrl: &TrayController,
) {
    if dto.hash.is_empty() {
        return;
    }

    if is_hash_processed(&dto.hash) {
        return;
    }
    if let Some(ref th) = dto.transfer_data_hash {
        if is_hash_processed(th) {
            return;
        }
    }

    log_info(format!("[Download] 检测到远端新剪贴板 (类型: {:?}, Hash: {})", dto.r#type, dto.hash));
    tray_ctrl.update_status("正在下载");

    let res = match dto.r#type {
        ProfileType::Text => {
            let full_text = if dto.has_data {
                if let Some(ref data_name) = dto.data_name {
                    match client.get_file(data_name).await {
                        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                        Err(e) => {
                            log_info(format!("[Download] 下载超长文本附件失败: {}", e));
                            dto.text.clone()
                        }
                    }
                } else {
                    dto.text.clone()
                }
            } else {
                dto.text.clone()
            };

            let text_sha256 = ClipboardContent::compute_sha256(full_text.as_bytes());
            mark_hash_processed(&dto.hash);
            mark_hash_processed(&text_sha256);

            let write_res = ClipboardManager::write_text(&full_text);
            if write_res.is_ok() {
                log_info("[Download] 远程文本已成功写入本地剪贴板！");
            }
            write_res
        }
        ProfileType::Image => {
            if let Some(ref data_name) = dto.data_name {
                match client.get_file(data_name).await {
                    Ok(png_bytes) => {
                        let content_hash = ClipboardContent::compute_sha256(&png_bytes);
                        mark_hash_processed(&dto.hash);
                        if let Some(ref th) = dto.transfer_data_hash {
                            mark_hash_processed(th);
                        }
                        mark_hash_processed(&content_hash);

                        let write_res = ClipboardManager::write_image(&png_bytes);
                        if write_res.is_ok() {
                            log_info("[Download] 远程图片已成功写入本地剪贴板！");
                        }
                        write_res
                    }
                    Err(e) => Err(e),
                }
            } else {
                Err("图片 Profile 缺少 data_name".into())
            }
        }
        _ => Ok(()),
    };

    if let Err(e) = res {
        log_info(format!("[Download] 下载写入失败: {}", e));
        tray_ctrl.update_status("下载失败");
    } else {
        tray_ctrl.update_status("在线");
    }
}
