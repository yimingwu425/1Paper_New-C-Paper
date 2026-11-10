use super::*;
static STAGING_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(super) async fn new_staging(
    path: &std::path::Path,
    purpose: &str,
) -> Result<(PathBuf, tokio::fs::File), ErrApi> {
    let parent = path.parent().ok_or_else(|| bad("暂存文件缺少父目录"))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| bad("目标文件名无效"))?;
    for _ in 0..32 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(upstream)?
            .as_nanos();
        let temporary = parent.join(format!(
            ".{name}.{purpose}.{}.{stamp}.{sequence}.part",
            std::process::id()
        ));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await
        {
            Ok(file) => return Ok((temporary, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(upstream(e)),
        }
    }
    Err(upstream("暂存命名发生连续冲突；保留已有文件"))
}

pub(super) async fn verified_pdf_at(path: &std::path::Path, hash: &str) -> Option<u64> {
    let meta = tokio::fs::symlink_metadata(path).await.ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 25 * 1024 * 1024 {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    if !bytes.starts_with(b"%PDF-")
        || !bytes.windows(5).rev().take(2048).any(|b| b == b"%%EOF")
        || format!("{:x}", Sha256::digest(&bytes)) != hash
    {
        return None;
    }
    Some(bytes.len() as u64)
}
pub(super) fn record_verifies_source(record: &Value, source_url: &str) -> bool {
    record["source_url"] == source_url
        || record["verified_sources"]
            .as_array()
            .is_some_and(|origins| {
                origins
                    .iter()
                    .any(|origin| origin["source_url"] == source_url)
            })
}
pub(super) async fn cached_saved(app: &App, p: &Paper) -> Result<Option<Value>, ErrApi> {
    let record_path = app.root.join(format!("{}.download.json", p.filename));
    if !record_path.exists() {
        return Ok(None);
    }
    let record: Value = read_control(&record_path).await?;
    let Some(path) = record["path"].as_str().map(PathBuf::from) else {
        return Ok(None);
    };
    if !path.is_absolute()
        || path.file_name().and_then(|n| n.to_str()) != Some(p.filename.as_str())
        || record["filename"] != p.filename
        || !record_verifies_source(&record, &p.source_url)
    {
        return Ok(None);
    }
    if verified_pdf_at(&path, record["sha256"].as_str().unwrap_or(""))
        .await
        .is_some()
    {
        return Ok(Some(
            json!({"filename":p.filename,"path":path,"sha256":record["sha256"],"reused_existing":true}),
        ));
    }
    Ok(None)
}
pub(super) async fn save(app: &App, p: &Paper) -> Result<Value, ErrApi> {
    if let Some(saved) = cached_saved(app, p).await? {
        return Ok(saved);
    }
    let bytes = pdf(app, p).await?;
    commit_pdf(app, p, &bytes, &app.root.join(&p.filename)).await
}
pub(super) async fn commit_pdf(
    app: &App,
    p: &Paper,
    bytes: &[u8],
    destination: &std::path::Path,
) -> Result<Value, ErrApi> {
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let _lock = app.io.lock().await;
    tokio::fs::create_dir_all(&app.root)
        .await
        .map_err(upstream)?;
    let dest = destination.to_path_buf();
    if dest.file_name().and_then(|s| s.to_str()) != Some(p.filename.as_str()) {
        return Err(bad("目标文件名与清单不符"));
    }
    let parent = dest.parent().ok_or_else(|| bad("目标缺少父目录"))?;
    let meta = tokio::fs::symlink_metadata(parent)
        .await
        .map_err(upstream)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(bad("目标父目录须为已验证普通目录"));
    }
    if tokio::fs::try_exists(&dest).await.map_err(upstream)? {
        if tokio::fs::symlink_metadata(&dest)
            .await
            .map_err(upstream)?
            .file_type()
            .is_symlink()
        {
            return Err(bad("拒绝符号链接目标"));
        }
        let old = tokio::fs::read(&dest).await.map_err(upstream)?;
        if old != bytes {
            return Err(err(StatusCode::CONFLICT, "已有同名文件内容不同，保留原件"));
        }
    } else {
        let (temp, mut f) = new_staging(&dest, "pdf").await?;
        use tokio::io::AsyncWriteExt;
        let result = async {
            f.write_all(&bytes).await?;
            f.sync_all().await?;
            tokio::fs::hard_link(&temp, &dest).await
        }
        .await;
        let _ = tokio::fs::remove_file(&temp).await;
        result.map_err(upstream)?;
    }
    let record_path = app.root.join(format!("{}.download.json", p.filename));
    let mut record = json!({"id":p.id,"filename":p.filename,"path":dest,"sha256":hash,"source":p.source,"source_url":p.source_url,"bytes":bytes.len(),"saved_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()});
    if tokio::fs::try_exists(&record_path)
        .await
        .map_err(upstream)?
    {
        if tokio::fs::symlink_metadata(&record_path)
            .await
            .map_err(upstream)?
            .file_type()
            .is_symlink()
        {
            return Err(bad("拒绝下载记录符号链接"));
        }
        let old: Value =
            serde_json::from_slice(&tokio::fs::read(&record_path).await.map_err(upstream)?)
                .map_err(upstream)?;
        if old["sha256"] != record["sha256"] || old["filename"] != record["filename"] {
            return Err(err(StatusCode::CONFLICT, "PDF已保存，但已有下载记录不同"));
        }
        if old["path"] != record["path"]
            || old["source_url"] != record["source_url"]
            || old["id"] != record["id"]
        {
            let mut origins = old["verified_sources"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for origin in [&old, &record] {
                let entry = json!({"id":origin["id"],"source":origin["source"],"source_url":origin["source_url"]});
                if !origins.contains(&entry) {
                    origins.push(entry);
                }
            }
            record["verified_sources"] = json!(origins);
            write_control(&record_path, &record).await?;
        }
    } else {
        atomic_new(&record_path, &serde_json::to_vec_pretty(&record).unwrap()).await?;
    }
    Ok(json!({"filename":p.filename,"path":dest,"sha256":hash}))
}

pub(super) async fn atomic_new(path: &std::path::Path, bytes: &[u8]) -> Result<(), ErrApi> {
    let (tmp, mut f) = new_staging(path, "json").await?;
    use tokio::io::AsyncWriteExt;
    let result = async {
        f.write_all(bytes).await?;
        f.sync_all().await?;
        tokio::fs::hard_link(&tmp, path).await
    }
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;
    result.map_err(upstream)
}
#[derive(Deserialize)]
pub(super) struct Batch {
    pub(super) ids: Vec<String>,
}
pub(super) async fn batch(
    State(app): State<App>,
    h: HeaderMap,
    Json(body): Json<Batch>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    if body.ids.is_empty() || body.ids.len() > 50 {
        return Err(bad("一次选择1–50份文件"));
    }
    let mut results = vec![];
    let mut succeeded = 0;
    let mut seen = std::collections::HashSet::new();
    for id in body.ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        let result = async {
            let p = lookup(&app, &id).await?;
            save(&app, &p).await
        }
        .await;
        match result {
            Ok(mut v) => {
                v["id"] = json!(id);
                v["ok"] = json!(true);
                succeeded += 1;
                results.push(v)
            }
            Err((status, Json(e))) => results
                .push(json!({"id":id,"ok":false,"error":e["error"],"http_status":status.as_u16()})),
        }
    }
    Ok(Json(
        json!({"succeeded":succeeded,"failed":results.len()-succeeded,"results":results}),
    ))
}
pub(super) async fn downloads(State(app): State<App>) -> Result<Json<Value>, ErrApi> {
    if !tokio::fs::try_exists(&app.root).await.map_err(upstream)? {
        return Ok(Json(json!({"downloads":[]})));
    }
    let mut dir = tokio::fs::read_dir(&app.root).await.map_err(upstream)?;
    let mut records = vec![];
    while let Some(entry) = dir.next_entry().await.map_err(upstream)? {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".download.json")
            || !entry.file_type().await.map_err(upstream)?.is_file()
        {
            continue;
        }
        let bytes = tokio::fs::read(entry.path()).await.map_err(upstream)?;
        let mut record: Value = serde_json::from_slice(&bytes).map_err(upstream)?;
        let file = record["filename"]
            .as_str()
            .ok_or_else(|| bad("下载记录缺少文件名"))?;
        if parse(file).is_none() {
            return Err(bad("下载记录文件名无效"));
        }
        let path = record["path"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| app.root.join(file));
        if !path.is_absolute() || path.file_name().and_then(|n| n.to_str()) != Some(file) {
            return Err(bad("下载记录路径无效"));
        }
        record["path"] = json!(path);
        record["status"] = match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                let b = tokio::fs::read(&path).await.map_err(upstream)?;
                json!(if format!("{:x}", Sha256::digest(&b))
                    == record["sha256"].as_str().unwrap_or("")
                {
                    "available"
                } else {
                    "changed"
                })
            }
            _ => json!("missing"),
        };
        records.push(record);
    }
    records.sort_by(|a, b| b["saved_at"].as_u64().cmp(&a["saved_at"].as_u64()));
    Ok(Json(json!({"downloads":records})))
}
pub(super) async fn download(
    State(app): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    let p = lookup(&app, &id).await?;
    Ok(Json(save(&app, &p).await?))
}

pub(super) async fn handoff(
    State(app): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    let p = lookup(&app, &id).await?;
    let saved = save(&app, &p).await?;
    let manifest = json!({"schema":"cpaper.study-handoff.v1","stable_id":format!("cambridge:{}",p.filename),"source":p.source,"source_url":p.source_url,"filename":p.filename,"kind":p.kind,"local_file":saved["path"],"sha256":saved["sha256"],"review_status":"unreviewed","imported":false});
    let path = app.root.join(format!("{}.handoff.json", p.filename));
    let _lock = app.io.lock().await;
    let encoded = serde_json::to_vec_pretty(&manifest).unwrap();
    if tokio::fs::try_exists(&path).await.map_err(upstream)? {
        if tokio::fs::symlink_metadata(&path)
            .await
            .map_err(upstream)?
            .file_type()
            .is_symlink()
        {
            return Err(bad("拒绝符号链接清单"));
        }
        let old = tokio::fs::read(&path).await.map_err(upstream)?;
        if old != encoded {
            return Err(err(StatusCode::CONFLICT, "已有交接清单内容不同，保留原件"));
        }
    } else {
        let (tmp, mut f) = new_staging(&path, "manifest").await?;
        use tokio::io::AsyncWriteExt;
        let result = async {
            f.write_all(&encoded).await?;
            f.sync_all().await?;
            tokio::fs::hard_link(&tmp, &path).await
        }
        .await;
        let _ = tokio::fs::remove_file(&tmp).await;
        result.map_err(upstream)?;
    }
    Ok(Json(json!({"path":path,"manifest":manifest})))
}
