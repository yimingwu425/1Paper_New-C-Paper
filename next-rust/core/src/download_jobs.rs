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
        self.notify.notify_waiters()
    }
    async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
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
    #[serde(default = "default_source_mode")]
    pub(super) source_mode: String,
    #[serde(default = "default_concurrency")]
    pub(super) download_concurrency: usize,
    #[serde(default = "default_rate")]
    pub(super) requests_per_second: u32,
    #[serde(default)]
    pub(super) cooldown_until_ms: Option<u64>,
    #[serde(default)]
    pub(super) waiting_reason: Option<String>,
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
        if ["queued", "waiting", "downloading"].contains(&item.status.as_str()) {
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
    validate_paper(&file.paper())
}
fn validate_job(job: &DownloadJob) -> Result<(), ErrApi> {
    validate_limits(
        &job.source_mode,
        job.download_concurrency,
        job.requests_per_second,
    )?;
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
    validate_limits(
        &preview.source_mode,
        preview.download_concurrency,
        preview.requests_per_second,
    )?;
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
        source_mode: preview.source_mode,
        download_concurrency: preview.download_concurrency,
        requests_per_second: preview.requests_per_second,
        cooldown_until_ms: queue
            .job
            .as_ref()
            .and_then(|old| old.cooldown_until_ms)
            .filter(|until| *until > epoch_ms()),
        waiting_reason: None,
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
        job.waiting_reason = None;
        for item in &mut job.items {
            if ["queued", "waiting", "downloading"].contains(&item.status.as_str()) {
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
fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn retry_after(value: Option<&str>, now: std::time::SystemTime) -> std::time::Duration {
    let seconds = value
        .and_then(|v| {
            v.trim().parse::<u64>().ok().or_else(|| {
                httpdate::parse_http_date(v)
                    .ok()
                    .map(|date| date.duration_since(now).unwrap_or_default().as_secs())
            })
        })
        .unwrap_or(30)
        .clamp(5, 120);
    std::time::Duration::from_secs(seconds)
}
struct GateState {
    next: tokio::time::Instant,
    cooldown: tokio::time::Instant,
}
struct RequestGate {
    state: Mutex<GateState>,
    spacing: std::time::Duration,
}
impl RequestGate {
    fn new(rate: u32, cooldown_ms: Option<u64>) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            state: Mutex::new(GateState {
                next: now,
                cooldown: now
                    + std::time::Duration::from_millis(
                        cooldown_ms
                            .unwrap_or(0)
                            .saturating_sub(epoch_ms())
                            .min(120_000),
                    ),
            }),
            spacing: std::time::Duration::from_secs_f64(1.0 / rate.clamp(1, 20) as f64),
        }
    }
    async fn acquire(
        &self,
        app: &App,
        id: &str,
        index: usize,
        cancel: &CancelFlag,
    ) -> Result<(), ErrApi> {
        loop {
            if cancel.is_cancelled() {
                return Err(bad("cancelled"));
            }
            let (wait, cooling) = {
                let mut state = self.state.lock().await;
                let now = tokio::time::Instant::now();
                let ready = state.next.max(state.cooldown);
                if ready <= now {
                    state.next = now + self.spacing;
                    (None, false)
                } else {
                    (Some(ready - now), state.cooldown > now)
                }
            };
            {
                let mut queue = app.queue.lock().await;
                let Some(job) = queue
                    .job
                    .as_mut()
                    .filter(|j| j.id == id && j.status == "running")
                else {
                    return Err(bad("队列已停止"));
                };
                if cancel.is_cancelled() {
                    return Err(bad("cancelled"));
                }
                job.items[index].status = if wait.is_some() {
                    "waiting"
                } else {
                    "downloading"
                }
                .into();
                job.waiting_reason = if cooling {
                    Some("429".into())
                } else if wait.is_some() {
                    Some("rate_limit".into())
                } else {
                    None
                };
                if !cooling
                    && job
                        .cooldown_until_ms
                        .is_some_and(|until| until <= epoch_ms())
                {
                    job.cooldown_until_ms = None;
                }
            }
            let Some(wait) = wait else {
                return Ok(());
            };
            tokio::select! { _ = cancel.cancelled() => return Err(bad("cancelled")), _ = tokio::time::sleep(wait.min(std::time::Duration::from_millis(100))) => {} }
        }
    }
    async fn cooldown(&self, app: &App, id: &str, delay: std::time::Duration) {
        {
            let mut state = self.state.lock().await;
            state.cooldown = state.cooldown.max(tokio::time::Instant::now() + delay);
        }
        let mut queue = app.queue.lock().await;
        if let Some(job) = queue
            .job
            .as_mut()
            .filter(|j| j.id == id && j.status == "running")
        {
            job.cooldown_until_ms = Some(
                job.cooldown_until_ms
                    .unwrap_or(0)
                    .max(epoch_ms() + delay.as_millis() as u64),
            );
            job.waiting_reason = Some("429".into());
        }
    }
}
async fn stream_pdf(
    app: &App,
    id: &str,
    index: usize,
    p: &Paper,
    cancel: &CancelFlag,
    gate: &RequestGate,
) -> Result<Vec<u8>, ErrApi> {
    if cancel.is_cancelled() {
        return Err(bad("cancelled"));
    }
    gate.acquire(app, id, index, cancel).await?;
    let response = tokio::select! {_=cancel.cancelled()=>return Err(bad("cancelled")),r=app.client.get(download_url(p)?).send()=>r.map_err(|_| upstream("下载请求失败；未自动重试"))?};
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        let delay = retry_after(
            response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok()),
            std::time::SystemTime::now(),
        );
        gate.cooldown(app, id, delay).await;
        return Err(upstream(format!(
            "HTTP 429：共享队列冷却{}秒；此文件保留失败，需手动重试",
            delay.as_secs()
        )));
    }
    let response = response.error_for_status().map_err(|e| {
        upstream(format!(
            "下载HTTP错误：{}",
            e.status().map(|s| s.as_u16()).unwrap_or(0)
        ))
    })?;
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
        let chunk =
            tokio::select! {_=cancel.cancelled()=>return Err(bad("cancelled")),c=stream.next()=>c};
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| upstream("PDF流读取失败"))?;
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
        if ["queued", "waiting", "downloading"].contains(&item.status.as_str()) {
            item.status = "interrupted".into();
            item.error = Some("队列持久化失败，核对后手动重试".into());
        }
    }
    job.status = "failed".into();
    job.waiting_reason = None;
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
    let (count, concurrency, rate, cooldown) = {
        let queue = app.queue.lock().await;
        let Some(job) = queue
            .job
            .as_ref()
            .filter(|j| j.id == id && j.status == "running")
        else {
            return;
        };
        (
            job.items.len(),
            job.download_concurrency.clamp(1, 16),
            job.requests_per_second.clamp(1, 20),
            job.cooldown_until_ms,
        )
    };
    let gate = Arc::new(RequestGate::new(rate, cooldown));
    futures_util::stream::iter(0..count)
        .map(|index| run_item(app.clone(), id.clone(), index, cancel.clone(), gate.clone()))
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;
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
        job.waiting_reason = None;
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

async fn run_item(
    app: App,
    id: String,
    index: usize,
    cancel: Arc<CancelFlag>,
    gate: Arc<RequestGate>,
) {
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
            return;
        }
        job.items[index].status = "downloading".into();
        if let Err((_, Json(e))) = persist(&app, job).await {
            job.items[index].status = "failed".into();
            job.items[index].error = Some(format!("队列持久化失败：{}", e["error"]));
            interrupt_remaining(job);
            cancel.cancel();
            return;
        }
        job.items[index].file.clone()
    };
    let record_path = app.root.join(format!("{}.download.json", file.filename));
    let verified = if record_path.exists() {
        match read_control::<Value>(&record_path).await {
            Ok(record)
                if record["filename"] == file.filename
                    && record_verifies_source(&record, &file.source_url) =>
            {
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
                cancel.cancel();
                return;
            }
        }
        return;
    }
    let result = stream_pdf(&app, &id, index, &file.paper(), &cancel, &gate).await;
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
        cancel.cancel();
        job.items[index].error = Some(format!(
            "文件操作已结束但队列落盘失败，请核对目标：{}",
            e["error"]
        ));
        return;
    }
}

#[cfg(test)]
#[path = "download_jobs_tests.rs"]
mod tests;
