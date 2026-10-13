use serde_json::Value;
use std::time::Duration;

const URL: &str = "http://127.0.0.1:11436/personal";

async fn request(app: &tauri::AppHandle, packet: Option<Value>) -> Result<Value, String> {
    crate::ai::local::ensure(app).await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "无法连接个人词库，请重试".to_string())?;
    let request = match packet {
        Some(packet) => client.post(URL).json(&packet),
        None => client.get(URL),
    };
    let response = request
        .send()
        .await
        .map_err(|_| "个人词库服务未响应，请稍后重试".to_string())?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|_| "个人词库返回格式不正确，请重启 QuietType 后重试".to_string())?;
    if !status.is_success() {
        return Err(value["error"].as_str().unwrap_or("个人词库操作失败").into());
    }
    Ok(value)
}

pub async fn get(app: &tauri::AppHandle) -> Result<Value, String> {
    request(app, None).await
}

pub async fn apply(app: &tauri::AppHandle, packet: Value) -> Result<Value, String> {
    request(app, Some(packet)).await
}
