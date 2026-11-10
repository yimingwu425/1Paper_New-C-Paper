use super::*;
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Settings {
    pub(super) download_dir: String,
    pub(super) merge_folders: bool,
    #[serde(default = "default_source_mode")]
    pub(super) source_mode: String,
    #[serde(default = "default_concurrency")]
    pub(super) download_concurrency: usize,
    #[serde(default = "default_rate")]
    pub(super) requests_per_second: u32,
}
pub(super) fn default_source_mode() -> String {
    "auto".into()
}
pub(super) fn default_concurrency() -> usize {
    4
}
pub(super) fn default_rate() -> u32 {
    5
}
pub(super) fn validate_limits(source: &str, concurrency: usize, rate: u32) -> Result<(), ErrApi> {
    if ![
        "auto",
        "frankcie",
        "easypaper",
        "pastpapers",
        "papacambridge",
    ]
    .contains(&source)
        || !(1..=16).contains(&concurrency)
        || !(1..=20).contains(&rate)
    {
        return Err(bad("来源无效，下载并发须为1–16，请求速率须为1–20每秒"));
    }
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Favorite {
    code: String,
    name: String,
}
pub(super) async fn read_settings(app: &App) -> Result<Settings, ErrApi> {
    let path = app.root.join(".settings.json");
    if !path.exists() {
        tokio::fs::create_dir_all(&app.root)
            .await
            .map_err(upstream)?;
        return Ok(Settings {
            download_dir: tokio::fs::canonicalize(&app.root)
                .await
                .map_err(upstream)?
                .to_string_lossy()
                .into(),
            merge_folders: false,
            source_mode: default_source_mode(),
            download_concurrency: default_concurrency(),
            requests_per_second: default_rate(),
        });
    }
    read_control(&path).await
}
pub(super) async fn read_control<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
) -> Result<T, ErrApi> {
    let meta = tokio::fs::symlink_metadata(path).await.map_err(upstream)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 16 * 1024 * 1024 {
        return Err(bad("本地配置必须为受控普通文件"));
    }
    serde_json::from_slice(&tokio::fs::read(path).await.map_err(upstream)?).map_err(upstream)
}
pub(super) async fn write_control(
    path: &std::path::Path,
    value: &impl Serialize,
) -> Result<(), ErrApi> {
    let bytes = serde_json::to_vec_pretty(value).map_err(upstream)?;
    match tokio::fs::symlink_metadata(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => atomic_new(path, &bytes).await,
        Err(e) => Err(upstream(e)),
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err(bad("拒绝替换非普通配置"));
            }
            let (tmp, mut file) = new_staging(path, "control").await?;
            use tokio::io::AsyncWriteExt;
            let result = async {
                file.write_all(&bytes).await?;
                file.sync_all().await?;
                tokio::fs::rename(&tmp, path).await
            }
            .await;
            let _ = tokio::fs::remove_file(tmp).await;
            result.map_err(upstream)
        }
    }
}
pub(super) async fn settings_get(State(app): State<App>) -> Result<Json<Settings>, ErrApi> {
    Ok(Json(read_settings(&app).await?))
}
pub(super) async fn settings_put(
    State(app): State<App>,
    h: HeaderMap,
    Json(mut value): Json<Settings>,
) -> Result<Json<Settings>, ErrApi> {
    check_origin(&h)?;
    validate_limits(
        &value.source_mode,
        value.download_concurrency,
        value.requests_per_second,
    )?;
    let path = PathBuf::from(&value.download_dir);
    if !path.is_absolute() {
        return Err(bad("保存目录须为绝对路径"));
    }
    tokio::fs::create_dir_all(&path).await.map_err(upstream)?;
    value.download_dir = tokio::fs::canonicalize(path)
        .await
        .map_err(upstream)?
        .to_string_lossy()
        .into();
    tokio::fs::create_dir_all(&app.root)
        .await
        .map_err(upstream)?;
    let _guard = app.io.lock().await;
    write_control(&app.root.join(".settings.json"), &value).await?;
    Ok(Json(value))
}
async fn read_favorites(app: &App) -> Result<Vec<Favorite>, ErrApi> {
    let path = app.root.join(".favorites.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    read_control(&path).await
}
pub(super) async fn favorites_get(State(app): State<App>) -> Result<Json<Value>, ErrApi> {
    Ok(Json(json!({"favorites":read_favorites(&app).await?})))
}
pub(super) async fn favorites_add(
    State(app): State<App>,
    h: HeaderMap,
    Json(f): Json<Favorite>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    if f.code.len() != 4
        || !f.code.bytes().all(|b| b.is_ascii_digit())
        || f.name.trim().is_empty()
        || f.name.len() > 300
    {
        return Err(bad("收藏需要四位科目代码及有效名称"));
    }
    tokio::fs::create_dir_all(&app.root)
        .await
        .map_err(upstream)?;
    let _guard = app.io.lock().await;
    let mut favorites = read_favorites(&app).await?;
    favorites.retain(|old| old.code != f.code);
    favorites.push(f);
    favorites.sort_by(|a, b| a.code.cmp(&b.code));
    write_control(&app.root.join(".favorites.json"), &favorites).await?;
    Ok(Json(json!({"favorites":favorites})))
}
pub(super) async fn favorites_delete(
    State(app): State<App>,
    h: HeaderMap,
    Path(code): Path<String>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    tokio::fs::create_dir_all(&app.root)
        .await
        .map_err(upstream)?;
    let _guard = app.io.lock().await;
    let mut favorites = read_favorites(&app).await?;
    favorites.retain(|f| f.code != code);
    write_control(&app.root.join(".favorites.json"), &favorites).await?;
    Ok(Json(json!({"favorites":favorites})))
}

fn chosen_directory(output: std::process::Output) -> Result<Option<String>, ErrApi> {
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("(-128)") {
            return Ok(None);
        }
        return Err(upstream("系统目录选择未完成"));
    }
    let value = String::from_utf8(output.stdout).map_err(upstream)?;
    let value = value.strip_suffix('\n').unwrap_or(&value);
    if !std::path::Path::new(value).is_absolute() {
        return Err(bad("系统选择结果不是绝对目录"));
    }
    Ok(Some(value.into()))
}
pub(super) async fn choose_directory(
    State(app): State<App>,
    h: HeaderMap,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    let result = tokio::task::spawn_blocking(|| {
        std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "tell application \"Finder\"\nactivate\nset chosen to choose folder with prompt \"选择 C-Paper 保存目录\"\nreturn POSIX path of chosen\nend tell",
            ])
            .output()
    })
    .await
    .map_err(upstream)?
    .map_err(upstream)?;
    let Some(path) = chosen_directory(result)? else {
        return Ok(Json(
            json!({"cancelled":true,"settings":read_settings(&app).await?}),
        ));
    };
    let path = tokio::fs::canonicalize(&path).await.map_err(upstream)?;
    if !path.is_dir() {
        return Err(bad("系统选择结果不是目录"));
    }
    // The native picker may stay open while source/rate settings change. Merge only its
    // directory into the latest durable settings, inside the same persistence lock.
    let _guard = app.io.lock().await;
    let mut settings = read_settings(&app).await?;
    settings.download_dir = path.to_string_lossy().into();
    write_control(&app.root.join(".settings.json"), &settings).await?;
    Ok(Json(json!({"cancelled":false,"settings":settings})))
}
pub(super) async fn open_directory(
    State(app): State<App>,
    h: HeaderMap,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    let settings = read_settings(&app).await?;
    let path = tokio::fs::canonicalize(&settings.download_dir)
        .await
        .map_err(upstream)?;
    if !path.is_dir() {
        return Err(bad("保存位置不是目录"));
    }
    let selected = path.clone();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new("/usr/bin/open")
            .arg(selected)
            .status()
    })
    .await
    .map_err(upstream)?
    .map_err(upstream)?;
    if !status.success() {
        return Err(upstream("系统未能打开保存目录"));
    }
    Ok(Json(
        json!({"opened":true,"directory":path.to_string_lossy()}),
    ))
}
#[cfg(all(test, unix))]
mod directory_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    #[test]
    fn picker_cancel_error_and_quoted_path_are_data() {
        let output = |status, out: &str, err: &str| std::process::Output {
            status: std::process::ExitStatus::from_raw(status),
            stdout: out.as_bytes().to_vec(),
            stderr: err.as_bytes().to_vec(),
        };
        assert!(chosen_directory(output(256, "", "User canceled. (-128)"))
            .unwrap()
            .is_none());
        assert!(chosen_directory(output(256, "", "not available")).is_err());
        assert!(chosen_directory(output(0, "relative\n", "")).is_err());
        assert_eq!(
            chosen_directory(output(0, "/tmp/a'\";echo nope/\n", ""))
                .unwrap()
                .unwrap(),
            "/tmp/a'\";echo nope/"
        );
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    #[test]
    fn old_settings_gain_safe_defaults_and_limits_are_strict() {
        let old: Settings =
            serde_json::from_value(json!({"download_dir":"/tmp/papers","merge_folders":false}))
                .unwrap();
        assert_eq!(old.source_mode, "auto");
        assert_eq!(old.download_concurrency, 4);
        assert_eq!(old.requests_per_second, 5);
        for mode in [
            "auto",
            "frankcie",
            "easypaper",
            "pastpapers",
            "papacambridge",
        ] {
            assert!(validate_limits(mode, 1, 1).is_ok());
            assert!(validate_limits(mode, 16, 20).is_ok());
        }
        for (c, r) in [(0, 5), (17, 5), (4, 0), (4, 21)] {
            assert!(validate_limits("auto", c, r).is_err());
        }
        assert!(validate_limits("unknown", 4, 5).is_err());
    }
}
