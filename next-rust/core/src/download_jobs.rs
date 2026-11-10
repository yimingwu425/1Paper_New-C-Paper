use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Default)]
pub(super) struct CancelFlag {
    flag: AtomicBool,
    notify: tokio::sync::Notify,
}
impl CancelFlag {
    fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_one()
    }
    fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}
#[derive(Default)]
pub(super) struct QueueRuntime {
    pub(super) job: Option<DownloadJob>,
    cancel: Arc<CancelFlag>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct JobItem {
    pub(super) file: PreviewFile,
    pub(super) id: String,
    pub(super) filename: String,
    pub(super) destination: String,
    pub(super) status: String,
    pub(super) downloaded_bytes: u64,
    pub(super) total_bytes: Option<u64>,
    pub(super) error: Option<String>,
    #[serde(default)]
    pub(super) reused_existing: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct DownloadJob {
    pub(super) id: String,
    pub(super) preview_token: String,
    pub(super) status: String,
    pub(super) save_directory: String,
    pub(super) items: Vec<JobItem>,
    pub(super) total: usize,
    pub(super) completed: usize,
    pub(super) failed: usize,
    pub(super) cancelled: usize,
    pub(super) interrupted: usize,
}
impl DownloadJob {
    fn recount(&mut self) {
        self.total = self.items.len();
        self.completed = self
            .items
            .iter()
            .filter(|i| i.status == "completed")
            .count();
        self.failed = self.items.iter().filter(|i| i.status == "failed").count();
        self.cancelled = self
            .items
            .iter()
            .filter(|i| i.status == "cancelled")
            .count();
        self.interrupted = self
            .items
            .iter()
            .filter(|i| i.status == "interrupted")
            .count();
    }
}
#[derive(Deserialize)]
pub(super) struct StartJob {
    preview_token: String,
    #[serde(default)]
    accept_partial: bool,
}
async fn persist(app: &App, job: &DownloadJob) -> Result<(), ErrApi> {
    let _guard = app.io.lock().await;
    write_control(&app.root.join(".queue.json"), job).await
}
pub(super) async fn restore_queue(app: &App) -> Result<(), ErrApi> {
    tokio::fs::create_dir_all(&app.root)
        .await
        .map_err(upstream)?;
    let path = app.root.join(".queue.json");
    if !path.exists() {
        return Ok(());
    }
    let mut job: DownloadJob = read_control(&path).await?;
    validate_job(&job)?;
    for item in &mut job.items {
        if ["queued", "downloading"].contains(&item.status.as_str()) {
            item.status = "interrupted".into();
            item.error = Some("上次运行中断，请核对后重试；未自动续传".into())
        }
    }
    job.recount();
    if job.status == "running" {
        job.status = if job.interrupted > 0 {
            "interrupted"
        } else if job.failed > 0 {
            "failed"
        } else if job.cancelled > 0 {
            "cancelled"
        } else {
            "completed"
        }
        .into()
    }
    persist(app, &job).await?;
    app.queue.lock().await.job = Some(job);
    Ok(())
}
pub(super) async fn jobs_get(State(app): State<App>) -> Json<Value> {
    Json(json!({"job":app.queue.lock().await.job}))
}
fn trusted_file(file: &PreviewFile) -> bool {
    parse(&file.filename).is_some()
        && file.id == file.filename
        && file.source_url
            == format!(
                "https://cie.fraft.cn/obj/Common/Fetch/redir/{}",
                file.filename
            )
}
fn validate_job(job: &DownloadJob) -> Result<(), ErrApi> {
    if job.id.len() != 64
        || !job.id.bytes().all(|b| b.is_ascii_hexdigit())
        || job.id != job.preview_token
        || job.items.len() > 5000
        || !PathBuf::from(&job.save_directory).is_absolute()
    {
        return Err(bad("持久队列标识或目录无效"));
    }
    for item in &job.items {
        if !trusted_file(&item.file)
            || item.id != item.file.id
            || item.filename != item.file.filename
            || item.destination != item.file.destination
            || !PathBuf::from(&item.destination).starts_with(&job.save_directory)
        {
            return Err(bad("持久队列包含无效来源或路径"));
        }
    }
    Ok(())
}
pub(super) async fn jobs_start(
    State(app): State<App>,
    h: HeaderMap,
    Json(req): Json<StartJob>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    preview_path(&app, &req.preview_token)?;
    let mut queue = app.queue.lock().await;
    let archive = app
        .root
        .join("queue-history")
        .join(format!("{}.json", req.preview_token));
    if archive.exists() {
        let old: DownloadJob = read_control(&archive).await?;
        validate_job(&old)?;
        return Ok(Json(json!({"job":old,"replayed":true})));
    }
    if let Some(old) = &queue.job {
        if old.preview_token == req.preview_token {
            return Ok(Json(json!({"job":old,"replayed":true})));
        }
        if old.status == "running" {
            return Err(err(StatusCode::CONFLICT, "已有下载队列运行中"));
        }
    }
    let preview: BatchPreview = read_control(&preview_path(&app, &req.preview_token)?).await?;
    if preview.token != req.preview_token || preview.files.is_empty() {
        return Err(bad("预览token不匹配或清单为空"));
    }
    if !preview.complete && !req.accept_partial {
        return Err(err(
            StatusCode::CONFLICT,
            "部分年份考季查询失败；请明确确认仅下载已成功组合",
        ));
    }
    if preview.files.iter().any(|f| !trusted_file(f)) {
        return Err(bad("清单含未授权来源或文件标识"));
    }
    let root = tokio::fs::canonicalize(&preview.save_directory)
        .await
        .map_err(upstream)?;
    for f in &preview.files {
        if !PathBuf::from(&f.destination).starts_with(&root)
            || PathBuf::from(&f.destination)
                .file_name()
                .and_then(|n| n.to_str())
                != Some(f.filename.as_str())
        {
            return Err(bad("清单目标不在冻结保存目录"));
        }
    }
    if let Some(old) = &queue.job {
        let _guard = app.io.lock().await;
        let dir = app.root.join("queue-history");
        tokio::fs::create_dir_all(&dir).await.map_err(upstream)?;
        let archive = dir.join(format!("{}.json", old.id));
        if !archive.exists() {
            atomic_new(&archive, &serde_json::to_vec_pretty(old).map_err(upstream)?).await?;
        }
    }
    let mut job = DownloadJob {
        id: preview.token.clone(),
        preview_token: preview.token,
        status: "running".into(),
        save_directory: root.to_string_lossy().into(),
        items: preview
            .files
            .into_iter()
            .map(|f| JobItem {
                id: f.id.clone(),
                filename: f.filename.clone(),
                destination: f.destination.clone(),
                file: f,
                status: "queued".into(),
                downloaded_bytes: 0,
                total_bytes: None,
                error: None,
                reused_existing: false,
            })
            .collect(),
        total: 0,
        completed: 0,
        failed: 0,
        cancelled: 0,
        interrupted: 0,
    };
    job.recount();
    persist(&app, &job).await?;
    queue.job = Some(job.clone());
    queue.cancel = Arc::new(CancelFlag::default());
    let cancel = queue.cancel.clone();
    let id = job.id.clone();
    drop(queue);
    tokio::spawn(run_job(app, id, cancel));
    Ok(Json(json!({"job":job})))
}
pub(super) async fn jobs_cancel(
    State(app): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    let mut queue = app.queue.lock().await;
    if queue.job.as_ref().map(|j| j.id.as_str()) != Some(&id) {
        return Err(err(StatusCode::NOT_FOUND, "队列不存在"));
    }
    queue.cancel.cancel();
    let job = queue.job.as_mut().unwrap();
    if job.status == "running" {
        job.status = "cancelled".into();
        for item in &mut job.items {
            if ["queued", "downloading"].contains(&item.status.as_str()) {
                item.status = "cancelled".into();
                item.error = Some("用户取消；禁止迟到提交".into())
            }
        }
        job.recount();
        persist(&app, job).await?
    }
    Ok(Json(json!({"job":job})))
}
pub(super) async fn jobs_retry(
    State(app): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    let mut queue = app.queue.lock().await;
    let job = queue
        .job
        .as_mut()
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "队列不存在"))?;
    if job.id != id {
        return Err(err(StatusCode::NOT_FOUND, "队列不存在"));
    }
    if job.status == "running" {
        return Err(err(StatusCode::CONFLICT, "队列仍在运行"));
    }
    let mut candidate = job.clone();
    let mut count = 0;
    for item in &mut candidate.items {
        if ["failed", "interrupted", "cancelled"].contains(&item.status.as_str()) {
            item.status = "queued".into();
            item.downloaded_bytes = 0;
            item.total_bytes = None;
            item.error = None;
            count += 1
        }
    }
    if count == 0 {
        return Err(bad("没有可重试的失败、中断或取消项"));
    }
    candidate.status = "running".into();
    candidate.recount();
    persist(&app, &candidate).await?;
    let result = candidate.clone();
    *job = candidate;
    queue.cancel = Arc::new(CancelFlag::default());
    let cancel = queue.cancel.clone();
    drop(queue);
    tokio::spawn(run_job(app, id, cancel));
    Ok(Json(json!({"job":result})))
}
async fn stream_pdf(
    app: &App,
    id: &str,
    index: usize,
    p: &Paper,
    cancel: &CancelFlag,
) -> Result<Vec<u8>, ErrApi> {
    if cancel.is_cancelled() {
        return Err(bad("cancelled"));
    }
    let response=tokio::select!{_=cancel.notify.notified()=>return Err(bad("cancelled")),r=app.client.get(&p.source_url).send()=>r.map_err(upstream)?}.error_for_status().map_err(upstream)?;
    let total = response.content_length();
    if total.unwrap_or(0) > 25 * 1024 * 1024 {
        return Err(upstream("PDF超过25MiB"));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = vec![];
    loop {
        if cancel.is_cancelled() {
            return Err(bad("cancelled"));
        }
        let chunk = tokio::select! {_=cancel.notify.notified()=>return Err(bad("cancelled")),c=stream.next()=>c};
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(upstream)?;
        if bytes.len() + chunk.len() > 25 * 1024 * 1024 {
            return Err(upstream("PDF超过25MiB"));
        }
        bytes.extend_from_slice(&chunk);
        let mut queue = app.queue.lock().await;
        if let Some(job) = queue.job.as_mut().filter(|j| j.id == id) {
            if !cancel.is_cancelled() {
                job.items[index].downloaded_bytes = bytes.len() as u64;
                job.items[index].total_bytes = total
            }
        }
    }
    if !bytes.starts_with(b"%PDF-") || !bytes.windows(5).rev().take(2048).any(|b| b == b"%%EOF") {
        return Err(upstream("上游不是完整PDF"));
    }
    Ok(bytes)
}
fn interrupt_remaining(job: &mut DownloadJob) {
    for item in &mut job.items {
        if ["queued", "downloading"].contains(&item.status.as_str()) {
            item.status = "interrupted".into();
            item.error = Some("队列持久化失败，核对后手动重试".into());
        }
    }
    job.status = "failed".into();
    job.recount();
}
async fn safe_parent(base: &std::path::Path, parent: &std::path::Path) -> Result<PathBuf, ErrApi> {
    let real = tokio::fs::canonicalize(base).await.map_err(upstream)?;
    if real != base {
        return Err(bad("冻结保存目录已变为链接"));
    }
    let relative = parent
        .strip_prefix(base)
        .map_err(|_| bad("目录逃离冻结保存位置"))?;
    let mut path = real;
    for part in relative.components() {
        let std::path::Component::Normal(name) = part else {
            return Err(bad("目录组件无效"));
        };
        path.push(name);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(bad("目录含链接或非目录")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir(&path).await.map_err(upstream)?;
                let meta = tokio::fs::symlink_metadata(&path).await.map_err(upstream)?;
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    return Err(bad("目录创建后发生变化"));
                }
            }
            Err(e) => return Err(upstream(e)),
        }
    }
    Ok(path)
}
async fn run_job(app: App, id: String, cancel: Arc<CancelFlag>) {
    let count = app
        .queue
        .lock()
        .await
        .job
        .as_ref()
        .filter(|j| j.id == id)
        .map(|j| j.items.len())
        .unwrap_or(0);
    for index in 0..count {
        let file = {
            let mut queue = app.queue.lock().await;
            if cancel.is_cancelled() {
                return;
            }
            let Some(job) = queue
                .job
                .as_mut()
                .filter(|j| j.id == id && j.status == "running")
            else {
                return;
            };
            if job.items[index].status != "queued" {
                continue;
            }
            job.items[index].status = "downloading".into();
            if let Err((_, Json(e))) = persist(&app, job).await {
                job.items[index].status = "failed".into();
                job.items[index].error = Some(format!("队列持久化失败：{}", e["error"]));
                interrupt_remaining(job);
                return;
            }
            job.items[index].file.clone()
        };
        let record_path = app.root.join(format!("{}.download.json", file.filename));
        let verified = if record_path.exists() {
            match read_control::<Value>(&record_path).await {
                Ok(record) if record["filename"] == file.filename => {
                    verified_pdf_at(
                        std::path::Path::new(&file.destination),
                        record["sha256"].as_str().unwrap_or(""),
                    )
                    .await
                }
                _ => None,
            }
        } else {
            None
        };
        if let Some(size) = verified {
            let mut queue = app.queue.lock().await;
            if cancel.is_cancelled() {
                return;
            }
            if let Some(job) = queue
                .job
                .as_mut()
                .filter(|j| j.id == id && j.status == "running")
            {
                job.items[index].status = "completed".into();
                job.items[index].downloaded_bytes = size;
                job.items[index].total_bytes = Some(size);
                job.items[index].reused_existing = true;
                job.recount();
                if persist(&app, job).await.is_err() {
                    interrupt_remaining(job);
                    return;
                }
            }
            continue;
        }
        let result = stream_pdf(&app, &id, index, &file.paper(), &cancel).await;
        // Cancellation and the final commit share this mutex. Acknowledged cancel cannot be followed by a commit.
        let mut queue = app.queue.lock().await;
        if cancel.is_cancelled() {
            return;
        }
        let Some(job) = queue
            .job
            .as_mut()
            .filter(|j| j.id == id && j.status == "running")
        else {
            return;
        };
        let result = match result {
            Ok(bytes) => {
                let parent = PathBuf::from(&file.destination)
                    .parent()
                    .unwrap()
                    .to_path_buf();
                match safe_parent(std::path::Path::new(&job.save_directory), &parent).await {
                    Ok(real) => {
                        commit_pdf(&app, &file.paper(), &bytes, &real.join(&file.filename)).await
                    }
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        };
        match result {
            Ok(_) => {
                job.items[index].status = "completed".into();
                job.items[index].error = None
            }
            Err((_, Json(e))) => {
                job.items[index].status = "failed".into();
                job.items[index].error = Some(e["error"].as_str().unwrap_or("下载失败").into())
            }
        }
        job.recount();
        if let Err((_, Json(e))) = persist(&app, job).await {
            interrupt_remaining(job);
            job.items[index].error = Some(format!(
                "文件操作已结束但队列落盘失败，请核对目标：{}",
                e["error"]
            ));
            return;
        }
    }
    let mut queue = app.queue.lock().await;
    if cancel.is_cancelled() {
        return;
    }
    if let Some(job) = queue
        .job
        .as_mut()
        .filter(|j| j.id == id && j.status == "running")
    {
        job.recount();
        job.status = if job.failed > 0 {
            "failed"
        } else {
            "completed"
        }
        .into();
        if persist(&app, job).await.is_err() {
            interrupt_remaining(job);
        }
    }
}

#[cfg(test)]
#[path = "download_jobs_tests.rs"]
mod tests;
