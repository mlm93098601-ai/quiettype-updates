use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    bytes: u64,
    expanded_bytes: u64,
    sha256: String,
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    name: String,
    bytes: u64,
    sha256: String,
    url: String,
}

impl Manifest {
    fn validate(&self) -> Result<(), String> {
        let hash = |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
        let mut total = 0u64;
        for (index, part) in self.parts.iter().enumerate() {
            let name = format!("QuietType-Runtime-v1.tar.gz.part{:02}", index + 1);
            let url = format!("https://github.com/mlm93098601-ai/quiettype-updates/releases/download/runtime-v1/{name}");
            if part.name != name || part.url != url || part.bytes == 0 || !hash(&part.sha256) {
                return Err("本地语音下载清单无效，请重新下载 QuietType".into());
            }
            total = total.checked_add(part.bytes).ok_or("下载大小无效")?;
        }
        if self.version != 1
            || self.parts.is_empty()
            || total != self.bytes
            || self.expanded_bytes == 0
            || !hash(&self.sha256)
        {
            return Err("本地语音下载清单无效，请重新下载 QuietType".into());
        }
        Ok(())
    }
}

fn regular_file(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err("下载暂存文件类型异常，已保留原文件".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn valid_file(path: &Path, bytes: u64, hash: &str) -> Result<bool, String> {
    if !regular_file(path)? || fs::metadata(path).map_err(|e| e.to_string())?.len() != bytes {
        return Ok(false);
    }
    let result = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    Ok(result.status.success()
        && String::from_utf8_lossy(&result.stdout)
            .split_whitespace()
            .next()
            == Some(hash))
}

fn preparation_command(
    runtime: &LocalRuntime,
    command: &mut Command,
    mut progress: impl FnMut(),
) -> Result<(), String> {
    {
        let mut child = runtime.child.lock().map_err(|e| e.to_string())?;
        if runtime.stopped.load(Ordering::SeqCst) {
            return Err("QuietType 正在退出".into());
        }
        if let Some(mut previous) = child.take() {
            stop_child(&mut previous);
        }
        *child = Some(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?,
        );
    }
    loop {
        let status = {
            let mut child = runtime.child.lock().map_err(|e| e.to_string())?;
            let current = child.as_mut().ok_or("QuietType 正在退出")?;
            let status = current.try_wait().map_err(|e| e.to_string())?;
            if status.is_some() {
                child.take();
            }
            status
        };
        progress();
        if let Some(status) = status {
            return if status.success() {
                Ok(())
            } else {
                Err(
                    "本地语音准备中断，请检查网络和磁盘空间，再点“重新准备”；已下载的内容会续传"
                        .into(),
                )
            };
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn download_command(destination: &Path, url: &str) -> Result<Command, String> {
    let mut command = Command::new("/usr/bin/curl");
    command
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "3",
            "--retry-all-errors",
            "--connect-timeout",
            "20",
            "--speed-time",
            "60",
            "--speed-limit",
            "1024",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--continue-at",
            "-",
            "--output",
        ])
        .arg(destination)
        .arg(url);
    if let Some(proxy) = crate::native::util_helper::download_proxy()? {
        command.arg("--proxy").arg(proxy);
    }
    Ok(command)
}

pub(super) fn prepare(
    manifest_path: &Path,
    data: &Path,
    runtime: &LocalRuntime,
) -> Result<(), String> {
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(manifest_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("无法读取本地语音下载清单：{e}"))?;
    manifest.validate()?;
    let (target, previous) = runtime_paths(data)?;
    let downloads = data.join("runtime-download-v1");
    if fs::symlink_metadata(&downloads).is_ok_and(|m| !m.is_dir() || m.file_type().is_symlink()) {
        return Err("下载暂存目录异常，已保留原文件".into());
    }
    fs::create_dir_all(&downloads).map_err(|e| e.to_string())?;
    let required = manifest
        .bytes
        .checked_mul(2)
        .and_then(|n| n.checked_add(manifest.expanded_bytes))
        .ok_or("下载大小无效")?;
    require_space(required, available_space(data)?)?;
    let archive = downloads.join("QuietType-Runtime-v1.tar.gz");
    runtime_status("verifying", "正在检查已下载的语音文件…", false);
    if !valid_file(&archive, manifest.bytes, &manifest.sha256)? {
        let mut completed = 0;
        for part in &manifest.parts {
            if runtime.stopped.load(Ordering::SeqCst) {
                return Err("QuietType 正在退出".into());
            }
            let destination = downloads.join(&part.name);
            if !valid_file(&destination, part.bytes, &part.sha256)? {
                let partial = downloads.join(format!("{}.download", part.name));
                if regular_file(&partial)?
                    && fs::metadata(&partial).map_err(|e| e.to_string())?.len() >= part.bytes
                    && !valid_file(&partial, part.bytes, &part.sha256)?
                {
                    fs::remove_file(&partial).map_err(|e| e.to_string())?;
                }
                if !valid_file(&partial, part.bytes, &part.sha256)? {
                    let mut percent = None;
                    preparation_command(
                        runtime,
                        &mut download_command(&partial, &part.url)?,
                        || {
                            let received = fs::metadata(&partial)
                                .map(|m| m.len())
                                .unwrap_or(0)
                                .min(part.bytes);
                            let next = (completed + received) * 100 / manifest.bytes;
                            if percent != Some(next) {
                                percent = Some(next);
                                #[cfg(test)]
                                if next % 10 == 0 {
                                    println!("Public runtime download: {next}%");
                                }
                                runtime_status("downloading", &format!("正在下载本地语音，{next}%（首次使用约 {:.1} GB，可断点续传）", manifest.bytes as f64 / 1e9), false);
                            }
                        },
                    )?;
                }
                runtime_status("verifying", "正在校验已下载的语音文件…", false);
                if !valid_file(&partial, part.bytes, &part.sha256)? {
                    fs::remove_file(&partial).map_err(|e| e.to_string())?;
                    return Err("语音文件校验失败，请点“重新准备”重试".into());
                }
                fs::rename(&partial, &destination).map_err(|e| e.to_string())?;
            }
            completed += part.bytes;
        }
        let mut destination = fs::File::create(&archive).map_err(|e| e.to_string())?;
        for part in &manifest.parts {
            if runtime.stopped.load(Ordering::SeqCst) {
                return Err("QuietType 正在退出".into());
            }
            std::io::copy(
                &mut fs::File::open(downloads.join(&part.name)).map_err(|e| e.to_string())?,
                &mut destination,
            )
            .map_err(|e| e.to_string())?;
        }
        drop(destination);
        if !valid_file(&archive, manifest.bytes, &manifest.sha256)? {
            return Err("语音文件校验失败，请重试".into());
        }
    }
    for part in &manifest.parts {
        let path = downloads.join(&part.name);
        if regular_file(&path)? {
            fs::remove_file(path).map_err(|e| e.to_string())?;
        }
    }
    let temporary = data.join(format!("runtime-v1-install-{}", std::process::id()));
    remove_owned_directory(&temporary)?;
    fs::create_dir(&temporary).map_err(|e| e.to_string())?;
    let result = (|| {
        runtime_status("installing", "正在安装本地语音，完成后即可使用…", false);
        preparation_command(
            runtime,
            Command::new("/usr/bin/tar")
                .arg("-xzf")
                .arg(&archive)
                .arg("-C")
                .arg(&temporary),
            || {},
        )?;
        if runtime.stopped.load(Ordering::SeqCst) {
            return Err("QuietType 正在退出".into());
        }
        runtime_status("verifying", "正在校验本地语音…", false);
        verify_runtime(&temporary)?;
        if runtime.stopped.load(Ordering::SeqCst) {
            return Err("QuietType 正在退出".into());
        }
        commit_runtime(&temporary, &target, &previous)
    })();
    let cleanup = remove_owned_directory(&temporary);
    result.and(cleanup)?;
    remove_owned_directory(&downloads)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_native() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            crate::native::util_helper::init_from_path(
                &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../lib/swift/build/libUtilHelper.dylib"),
            )
            .unwrap();
        });
    }

    #[test]
    fn download_manifest_rejects_wrong_hosts_paths_hashes_and_size() {
        let text = r#"{"version":1,"bytes":3,"expandedBytes":4,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","parts":[{"name":"QuietType-Runtime-v1.tar.gz.part01","bytes":3,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","url":"https://github.com/mlm93098601-ai/quiettype-updates/releases/download/runtime-v1/QuietType-Runtime-v1.tar.gz.part01"}]}"#;
        let mut manifest: Manifest = serde_json::from_str(text).unwrap();
        assert!(manifest.validate().is_ok());
        manifest.parts[0].url = "https://example.org/archive".into();
        assert!(manifest.validate().is_err());
        manifest = serde_json::from_str(text).unwrap();
        manifest.parts[0].name = "../outside".into();
        assert!(manifest.validate().is_err());
        manifest = serde_json::from_str(text).unwrap();
        manifest.bytes += 1;
        assert!(manifest.validate().is_err());
        manifest = serde_json::from_str(text).unwrap();
        manifest.sha256 = "invalid".into();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn quitting_stops_a_preparation_process_and_prevents_a_restart() {
        let runtime = std::sync::Arc::new(LocalRuntime::default());
        let worker = runtime.clone();
        let (ready, wait) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            preparation_command(&worker, Command::new("/bin/sleep").arg("10"), || {
                let _ = ready.send(());
            })
        });
        wait.recv().unwrap();
        runtime.stop();
        assert!(task.join().unwrap().is_err());
        assert!(preparation_command(&runtime, &mut Command::new("/usr/bin/true"), || {}).is_err());
    }

    #[test]
    #[ignore = "Verifies public download with system proxy and no terminal proxy variables"]
    fn public_system_proxy_download() {
        init_native();
        let output =
            std::env::temp_dir().join(format!("quiettype-network-check-{}", std::process::id()));
        assert!(!output.exists());
        let mut command = download_command(&output,
            "https://github.com/mlm93098601-ai/quiettype-updates/releases/download/runtime-v1/runtime-download.json").unwrap();
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ] {
            command.env_remove(name);
        }
        command.args(["--max-time", "30"]);
        let result = preparation_command(&LocalRuntime::default(), &mut command, || {});
        let bytes = fs::read(&output);
        let _ = fs::remove_file(&output);
        result.unwrap();
        assert_eq!(
            bytes.unwrap(),
            fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../release/runtime-download.json")
            )
            .unwrap()
        );
    }

    #[test]
    #[ignore = "Downloads the public runtime into an explicitly selected disposable directory"]
    fn public_first_install() {
        init_native();
        let data = PathBuf::from(
            std::env::var("QUIETTYPE_RUNTIME_INSTALL_CHECK")
                .expect("Choose a disposable test directory"),
        );
        let marker = data.join(".public-install-check");
        let resumed = data.exists();
        if resumed {
            assert_eq!(
                fs::read_to_string(&marker).unwrap(),
                "quiettype public install check v1\n"
            );
            assert!(!data.join("runtime-v1").exists(), "Check already completed");
        } else {
            fs::create_dir_all(&data).unwrap();
            fs::write(&marker, "quiettype public install check v1\n").unwrap();
        }
        let manifest =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../release/runtime-download.json");
        let info: Manifest = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        if !resumed {
            let partial = data
                .join("runtime-download-v1")
                .join(format!("{}.download", info.parts[0].name));
            fs::create_dir_all(partial.parent().unwrap()).unwrap();
            let status = download_command(&partial, &info.parts[0].url)
                .unwrap()
                .args(["--range", "0-1048575", "--max-filesize", "1048576"])
                .status()
                .unwrap();
            assert!(status.success());
            assert_eq!(fs::metadata(&partial).unwrap().len(), 1_048_576);
        }
        prepare(&manifest, &data, &LocalRuntime::default()).unwrap();
        assert!(installed(&data.join("runtime-v1")));
        assert!(!data.join("runtime-download-v1").exists());
    }
}
