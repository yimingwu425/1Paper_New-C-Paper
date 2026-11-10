use super::*;
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct LegacyHistory {
    id: String,
    filename: String,
    path: String,
    status: String,
    sha256: Option<String>,
    origin: String,
}
struct LegacySnapshot {
    favorites: Vec<Value>,
    history: Vec<LegacyHistory>,
    favorite_present: bool,
    history_present: bool,
}
fn legacy_directory() -> Result<PathBuf, ErrApi> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or_else(|| bad("未找到用户目录"))?)
            .join("Library/Application Support/C-Paper"),
    )
}
async fn read_optional(path: &std::path::Path) -> Result<Option<Value>, ErrApi> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(read_control(path).await?))
}
async fn read_legacy(directory: &std::path::Path) -> Result<LegacySnapshot, ErrApi> {
    let favorites = read_optional(&directory.join("favorites.json")).await?;
    let history = read_optional(&directory.join("download_history.json")).await?;
    let favorite_present = favorites.is_some();
    let history_present = history.is_some();
    let mut parsed_favorites = vec![];
    if let Some(v) = favorites {
        for f in v
            .as_array()
            .ok_or_else(|| bad("旧版favorites结构无效；原件未改"))?
        {
            let code = f["code"].as_str().ok_or_else(|| bad("旧收藏缺少代码"))?;
            let name = f["name"].as_str().ok_or_else(|| bad("旧收藏缺少名称"))?;
            if code.len() != 4
                || !code.bytes().all(|b| b.is_ascii_digit())
                || name.trim().is_empty()
            {
                return Err(bad("旧收藏包含无效科目"));
            }
            parsed_favorites.push(json!({"code":code,"name":name}));
        }
    }
    let mut parsed_history = vec![];
    if let Some(v) = history {
        for row in v["items"]
            .as_array()
            .ok_or_else(|| bad("旧版download_history缺少items数组；原件未改"))?
        {
            let filename = row["filename"]
                .as_str()
                .ok_or_else(|| bad("旧历史缺少文件名"))?;
            let path = row["save_path"]
                .as_str()
                .ok_or_else(|| bad("旧历史缺少保存路径"))?;
            if !PathBuf::from(path).is_absolute() || filename.is_empty() {
                return Err(bad("旧历史路径不是绝对路径或文件名为空"));
            }
            let exists = tokio::fs::metadata(path)
                .await
                .map(|m| m.is_file())
                .unwrap_or(false);
            parsed_history.push(LegacyHistory {
                id: format!("legacy:{:x}", Sha256::digest(format!("{path}\0{filename}"))),
                filename: filename.into(),
                path: path.into(),
                status: if exists { "unverified" } else { "missing" }.into(),
                sha256: None,
                origin: "legacy".into(),
            });
        }
    }
    Ok(LegacySnapshot {
        favorites: parsed_favorites,
        history: parsed_history,
        favorite_present,
        history_present,
    })
}
async fn stored_history(app: &App) -> Result<Vec<LegacyHistory>, ErrApi> {
    let path = app.root.join("legacy-history.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    read_control(&path).await
}
async fn existing_favorites(app: &App) -> Result<Vec<Value>, ErrApi> {
    let path = app.root.join(".favorites.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    read_control(&path).await
}
async fn preview_from(app: &App, directory: &std::path::Path) -> Result<Value, ErrApi> {
    let source = read_legacy(directory).await?;
    let current = existing_favorites(app).await?;
    let history = stored_history(app).await?;
    let mut codes: std::collections::HashSet<String> = current
        .iter()
        .filter_map(|f| f["code"].as_str().map(str::to_owned))
        .collect();
    let mut ids: std::collections::HashSet<String> = history.iter().map(|h| h.id.clone()).collect();
    let nf = source
        .favorites
        .iter()
        .filter(|f| codes.insert(f["code"].as_str().unwrap().into()))
        .count();
    let nh = source
        .history
        .iter()
        .filter(|h| ids.insert(h.id.clone()))
        .count();
    Ok(
        json!({"sources":{"favorites":{"status":if source.favorite_present{"available"}else{"missing"},"count":source.favorites.len()},"history":{"status":if source.history_present{"available"}else{"missing"},"count":source.history.len()}},"favorites_to_import":nf,"history_to_import":nh,"existing_favorites":current.len(),"existing_history":history.len(),"history_existing_files":source.history.iter().filter(|h|h.status=="unverified").count(),"history_missing_files":source.history.iter().filter(|h|h.status=="missing").count(),"hash_verification":"not_available"}),
    )
}
pub(super) async fn legacy_preview(State(app): State<App>) -> Result<Json<Value>, ErrApi> {
    Ok(Json(preview_from(&app, &legacy_directory()?).await?))
}
#[derive(Deserialize)]
pub(super) struct LegacyConfirm {
    confirmed: bool,
}
async fn import_from(app: &App, directory: &std::path::Path) -> Result<Value, ErrApi> {
    let source = read_legacy(directory).await?;
    tokio::fs::create_dir_all(&app.root)
        .await
        .map_err(upstream)?;
    let _guard = app.io.lock().await;
    let mut favorites = existing_favorites(app).await?;
    let mut history = stored_history(app).await?;
    let mut nf = 0;
    let mut nh = 0;
    for f in &source.favorites {
        if !favorites.iter().any(|old| old["code"] == f["code"]) {
            favorites.push(f.clone());
            nf += 1
        }
    }
    for h in &source.history {
        if !history.iter().any(|old| old.id == h.id) {
            history.push(h.clone());
            nh += 1
        }
    }
    // Each new-store document is atomic; reruns skip imported entries if a later write failed.
    if nh > 0 {
        write_control(&app.root.join("legacy-history.json"), &history).await?
    }
    if nf > 0 {
        write_control(&app.root.join(".favorites.json"), &favorites)
            .await
            .map_err(|_| {
                upstream("旧历史可能已导入，但新收藏保存失败；原件未改，可核对后幂等重试")
            })?
    }
    Ok(
        json!({"imported_favorites":nf,"imported_history":nh,"skipped_existing_favorites":source.favorites.len()-nf,"skipped_existing_history":source.history.len()-nh}),
    )
}
pub(super) async fn legacy_import(
    State(app): State<App>,
    h: HeaderMap,
    Json(req): Json<LegacyConfirm>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    if !req.confirmed {
        return Err(bad("请明确确认只读旧原件并导入新版独立存储"));
    }
    Ok(Json(import_from(&app, &legacy_directory()?).await?))
}
pub(super) async fn legacy_history(State(app): State<App>) -> Result<Json<Value>, ErrApi> {
    let mut history = stored_history(&app).await?;
    for h in &mut history {
        h.status = if tokio::fs::metadata(&h.path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            "unverified"
        } else {
            "missing"
        }
        .into();
        h.sha256 = None;
    }
    Ok(Json(json!({"count":history.len(),"history":history})))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn legacy_preview_and_idempotent_import_preserve_originals() {
        let root = std::env::temp_dir().join(format!(
            "cpaper-legacy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let old = root.join("old");
        let new = root.join("new");
        tokio::fs::create_dir_all(&old).await.unwrap();
        tokio::fs::create_dir_all(&new).await.unwrap();
        let source_favorites =
            br#"[{"code":"9709","name":"old math"},{"code":"9702","name":"physics"}]"#;
        let existing = root.join("existing.pdf");
        tokio::fs::write(&existing, b"not even verified")
            .await
            .unwrap();
        let source_history=serde_json::to_vec(&json!({"items":[{"filename":"one.pdf","save_path":existing},{"filename":"missing.pdf","save_path":root.join("missing.pdf")}]})).unwrap();
        tokio::fs::write(old.join("favorites.json"), source_favorites)
            .await
            .unwrap();
        tokio::fs::write(old.join("download_history.json"), &source_history)
            .await
            .unwrap();
        let app = App {
            queue: Default::default(),
            client: reqwest::Client::new(),
            files: Default::default(),
            io: Default::default(),
            root: new,
        };
        write_control(
            &app.root.join(".favorites.json"),
            &json!([{"code":"9709","name":"new preferred"}]),
        )
        .await
        .unwrap();
        let before = std::fs::read_dir(&app.root).unwrap().count();
        let preview = preview_from(&app, &old).await.unwrap();
        assert_eq!(preview["favorites_to_import"], 1);
        assert_eq!(preview["history_existing_files"], 1);
        assert_eq!(before, std::fs::read_dir(&app.root).unwrap().count());
        let imported = import_from(&app, &old).await.unwrap();
        assert_eq!(imported["imported_history"], 2);
        assert_eq!(imported["imported_favorites"], 1);
        assert_eq!(
            import_from(&app, &old).await.unwrap()["imported_history"],
            0
        );
        assert_eq!(
            existing_favorites(&app).await.unwrap()[0]["name"],
            "new preferred"
        );
        assert_eq!(
            tokio::fs::read(old.join("favorites.json")).await.unwrap(),
            source_favorites
        );
        assert_eq!(
            tokio::fs::read(old.join("download_history.json"))
                .await
                .unwrap(),
            source_history
        );
        let rows = legacy_history(State(app)).await.unwrap().0;
        assert!(rows["history"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["sha256"].is_null()));
        assert_eq!(rows["history"][0]["status"], "unverified");
        tokio::fs::write(old.join("download_history.json"), b"{}")
            .await
            .unwrap();
        assert!(read_legacy(&old).await.is_err());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
