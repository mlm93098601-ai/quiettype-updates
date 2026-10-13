pub(crate) async fn ensure_microphone_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        match crate::native::util_helper::microphone_authorization_status() {
            Ok(mut status) => {
                log::info!("macOS 麦克风权限状态: {}", status);
                if status == 0 {
                    match crate::native::util_helper::request_microphone_permission() {
                        Ok(true) => log::info!("已发起 macOS 麦克风权限请求，等待用户选择"),
                        Ok(false) => status = 2,
                        Err(error) => {
                            log::warn!("无法主动请求麦克风权限: {}", error);
                            status = -1;
                        }
                    }
                    if status == 0 {
                        for _ in 0..120 {
                            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                            match crate::native::util_helper::microphone_authorization_status() {
                                Ok(current) if current != 0 => {
                                    status = current;
                                    break;
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    log::warn!("等待麦克风权限时读取状态失败: {}", error);
                                    status = -1;
                                    break;
                                }
                            }
                        }
                        log::info!("macOS 麦克风权限请求结果: {}", status);
                    }
                }
                if status == 1 || status == 2 {
                    return Err("麦克风权限未授权。请前往 系统设置 → 隐私与安全性 → 麦克风，允许 QuietType 使用麦克风。".into());
                }
                if status == 0 {
                    return Err("等待麦克风授权超时，请重新录音后允许访问。".into());
                }
            }
            Err(error) => log::warn!("无法读取麦克风权限状态: {}", error),
        }
    }
    Ok(())
}
