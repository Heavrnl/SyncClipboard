use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server_url: String,
    pub user_name: String,
    pub password: String,
    pub pull_interval_sec: u64,
    pub enable_push: bool,
    pub enable_pull: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            user_name: String::new(),
            password: String::new(),
            pull_interval_sec: 3,
            enable_push: true,
            enable_pull: true,
        }
    }
}

#[derive(Deserialize)]
struct OldAccountConfig {
    #[serde(rename = "RemoteURL", default)]
    remote_url: String,
    #[serde(rename = "UserName", default)]
    user_name: String,
    #[serde(rename = "Password", default)]
    password: String,
}

#[derive(Deserialize)]
struct OldSyncService {
    #[serde(rename = "IntervalTime", default = "default_interval")]
    interval_time: u64,
    #[serde(rename = "PullSwitchOn", default = "default_true")]
    pull_switch_on: bool,
    #[serde(rename = "PushSwitchOn", default = "default_true")]
    push_switch_on: bool,
}

fn default_interval() -> u64 {
    3
}
fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct OldRootConfig {
    #[serde(rename = "SyncService")]
    sync_service: Option<OldSyncService>,
    #[serde(rename = "SavedAccounts")]
    saved_accounts: Option<serde_json::Value>,
    #[serde(rename = "Account")]
    account: Option<serde_json::Value>,
}

impl AppConfig {
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write("config.json", content)?;
        Ok(())
    }

    pub fn load() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // 1. 尝试从当前运行目录下的 config.json 读取
        if let Ok(content) = std::fs::read_to_string("config.json") {
            if let Ok(cfg) = serde_json::from_str::<AppConfig>(&content) {
                return Ok(cfg);
            }
        }

        // 2. 尝试从 %APPDATA%\SyncClipboard\SyncClipboard.json 读取
        if let Ok(app_data) = std::env::var("APPDATA") {
            let old_path = PathBuf::from(app_data).join("SyncClipboard").join("SyncClipboard.json");
            if old_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&old_path) {
                    if let Ok(cfg) = Self::parse_old_config(&content) {
                        return Ok(cfg);
                    }
                }
            }
        }

        Err("未找到有效的 SyncClipboard 配置文件".into())
    }

    fn parse_old_config(json_str: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let root: OldRootConfig = serde_json::from_str(json_str)?;

        let mut server_url = String::new();
        let mut user_name = String::new();
        let mut password = String::new();

        if let (Some(saved_accounts), Some(account)) = (root.saved_accounts, root.account) {
            let account_type = account.get("AccountType").and_then(|v| v.as_str()).unwrap_or("SyncClipboard");
            let account_id = account.get("AccountId").and_then(|v| v.as_str()).unwrap_or("1");

            if let Some(target) = saved_accounts.get(account_type).and_then(|v| v.get(account_id)) {
                if let Ok(acc) = serde_json::from_value::<OldAccountConfig>(target.clone()) {
                    server_url = acc.remote_url.trim_end_matches('/').to_string();
                    user_name = acc.user_name;
                    password = acc.password;
                }
            }
        }

        let (pull_interval_sec, enable_pull, enable_push) = match root.sync_service {
            Some(s) => (s.interval_time, s.pull_switch_on, s.push_switch_on),
            None => (3, true, true),
        };

        if server_url.is_empty() {
            return Err("解析到的配置缺少 RemoteURL".into());
        }

        Ok(Self {
            server_url,
            user_name,
            password,
            pull_interval_sec,
            enable_push,
            enable_pull,
        })
    }
}
