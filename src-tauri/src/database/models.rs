use serde::{Deserialize, Serialize};

/// Preserves the original history schema so older records remain readable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,

    // Audio
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_local_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mic_device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_metadata: Option<String>,

    // Text
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refined_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_text_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_reverted_ai: Option<bool>,

    // Language
    #[serde(skip_serializing_if = "Option::is_none")]
    pub languages: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_language: Option<String>,

    // Context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_app_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_app_bundle_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_app_window_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_app_window_web_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_app_window_web_domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused_app_window_web_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ax_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ax_html: Option<String>,

    // Mode & metadata
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_meta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_context: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_info: Option<String>,
}
