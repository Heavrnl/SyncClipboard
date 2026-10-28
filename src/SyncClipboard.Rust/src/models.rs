use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileType {
    #[serde(alias = "text", alias = "TEXT")]
    Text,
    #[serde(alias = "image", alias = "IMAGE")]
    Image,
    #[serde(alias = "file", alias = "FILE")]
    File,
    #[serde(alias = "group", alias = "GROUP")]
    Group,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    #[serde(alias = "Type", default = "default_text_type")]
    pub r#type: ProfileType,
    #[serde(alias = "Hash", default)]
    pub hash: String,
    #[serde(alias = "Text", default)]
    pub text: String,
    #[serde(alias = "HasData", default)]
    pub has_data: bool,
    #[serde(alias = "DataName", default)]
    pub data_name: Option<String>,
    #[serde(alias = "TransferDataHash", default)]
    pub transfer_data_hash: Option<String>,
    #[serde(alias = "Size", default)]
    pub size: Option<i64>,
    #[serde(alias = "SyncedAt", default)]
    pub synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn default_text_type() -> ProfileType {
    ProfileType::Text
}

impl Default for ProfileDto {
    fn default() -> Self {
        Self {
            r#type: ProfileType::Text,
            hash: String::new(),
            text: String::new(),
            has_data: false,
            data_name: None,
            transfer_data_hash: None,
            size: None,
            synced_at: None,
        }
    }
}
