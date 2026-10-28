use crate::config::AppConfig;
use crate::models::ProfileDto;
use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use std::error::Error;
use std::time::Duration;

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    server_url: String,
    auth_header: String,
}

impl ApiClient {
    pub fn new(config: &AppConfig) -> Self {
        let mut headers = HeaderMap::new();
        let creds = format!("{}:{}", config.user_name, config.password);
        let encoded = base64::engine::general_purpose::STANDARD.encode(creds);
        let auth_val = format!("Basic {}", encoded);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_val).unwrap_or(HeaderValue::from_static("")),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            server_url: config.server_url.trim_end_matches('/').to_string(),
            auth_header: auth_val,
        }
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub fn auth_header(&self) -> &str {
        &self.auth_header
    }

    pub async fn get_sync_profile(&self) -> Result<ProfileDto, Box<dyn Error + Send + Sync>> {
        let url = format!("{}/SyncClipboard.json", self.server_url);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(format!("获取 Profile 失败，状态码: {}", resp.status()).into());
        }
        let dto = resp.json::<ProfileDto>().await?;
        Ok(dto)
    }

    pub async fn put_sync_profile(&self, profile: &ProfileDto) -> Result<(), Box<dyn Error + Send + Sync>> {
        let url = format!("{}/SyncClipboard.json", self.server_url);
        let resp = self
            .client
            .put(&url)
            .header(CONTENT_TYPE, "application/json")
            .json(profile)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("上传 Profile 失败，状态码: {}", resp.status()).into());
        }
        Ok(())
    }

    pub async fn get_file(&self, file_name: &str) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        let url = format!("{}/file/{}", self.server_url, file_name);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(format!("下载文件 {} 失败，状态码: {}", file_name, resp.status()).into());
        }
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    pub async fn put_file(&self, file_name: &str, data: Vec<u8>) -> Result<(), Box<dyn Error + Send + Sync>> {
        let url = format!("{}/file/{}", self.server_url, file_name);
        let resp = self
            .client
            .put(&url)
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(data)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("上传文件 {} 失败，状态码: {}", file_name, resp.status()).into());
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn delete_file_folder(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let url = format!("{}/file", self.server_url);
        let _ = self.client.delete(&url).send().await;
        Ok(())
    }
}
