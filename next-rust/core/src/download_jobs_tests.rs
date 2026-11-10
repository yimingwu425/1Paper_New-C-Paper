use super::*;
use std::sync::atomic::AtomicUsize;
static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
async fn fixture() -> (
    App,
    u16,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let n = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "cpaper-queue-test-{}-{n}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let root = tokio::fs::canonicalize(root).await.unwrap();
    let good = Arc::new(AtomicUsize::new(0));
    let flaky = Arc::new(AtomicUsize::new(0));
    let g = good.clone();
    let f = flaky.clone();
    let router = Router::new()
        .route(
            "/slow",
            get(|| async {
                let stream = futures_util::stream::unfold(0, |step| async move {
                    match step {
                        0 => Some((Ok::<_, std::io::Error>(Vec::from(&b"%PDF-1.7\n"[..])), 1)),
                        1 => {
                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                            Some((Ok(Vec::from(&b"body\n%%EOF"[..])), 2))
                        }
                        _ => None,
                    }
                });
                axum::response::Response::builder()
                    .header("Content-Length", "19")
                    .body(axum::body::Body::from_stream(stream))
                    .unwrap()
            }),
        )
        .route(
            "/good",
            get(move || {
                let c = g.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    "%PDF-1.7\ngood\n%%EOF"
                }
            }),
        )
        .route(
            "/flaky",
            get(move || {
                let c = f.clone();
                async move {
                    if c.fetch_add(1, Ordering::SeqCst) == 0 {
                        (StatusCode::BAD_GATEWAY, "temporary")
                    } else {
                        (StatusCode::OK, "%PDF-1.7\nflaky\n%%EOF")
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let app = App {
        queue: Default::default(),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        files: Default::default(),
        io: Default::default(),
        root,
    };
    (app, port, good, flaky, handle)
}
fn item(app: &App, name: &str, url: String) -> JobItem {
    let file = PreviewFile {
        id: name.into(),
        filename: name.into(),
        kind: if name.contains("_qp_") { "qp" } else { "ms" }.into(),
        source_url: url,
        destination: app.root.join("output").join(name).to_string_lossy().into(),
    };
    JobItem {
        id: name.into(),
        filename: name.into(),
        destination: file.destination.clone(),
        file,
        status: "queued".into(),
        downloaded_bytes: 0,
        total_bytes: None,
        error: None,
        reused_existing: false,
    }
}
async fn set_job(app: &App, items: Vec<JobItem>) -> (String, Arc<CancelFlag>) {
    tokio::fs::create_dir_all(app.root.join("output"))
        .await
        .unwrap();
    let id = "a".repeat(64);
    let mut job = DownloadJob {
        id: id.clone(),
        preview_token: id.clone(),
        status: "running".into(),
        save_directory: app.root.join("output").to_string_lossy().into(),
        items,
        total: 0,
        completed: 0,
        failed: 0,
        cancelled: 0,
        interrupted: 0,
    };
    job.recount();
    let mut queue = app.queue.lock().await;
    queue.job = Some(job);
    queue.cancel = Arc::new(CancelFlag::default());
    (id, queue.cancel.clone())
}
async fn terminal(app: &App) -> DownloadJob {
    for _ in 0..200 {
        let job = app.queue.lock().await.job.clone().unwrap();
        if job.status != "running" {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await
    }
    panic!("queue did not settle")
}
#[tokio::test]
async fn acknowledged_cancel_has_no_late_commit_and_real_bytes() {
    let (app, port, _, _, server) = fixture().await;
    let entry = item(
        &app,
        "9709_s23_qp_12.pdf",
        format!("http://127.0.0.1:{port}/slow"),
    );
    let path = PathBuf::from(&entry.destination);
    let (id, cancel) = set_job(&app, vec![entry]).await;
    let worker = tokio::spawn(run_job(app.clone(), id.clone(), cancel));
    let mut observed = false;
    for _ in 0..100 {
        if app.queue.lock().await.job.as_ref().unwrap().items[0].downloaded_bytes > 0 {
            observed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(3)).await
    }
    assert!(observed);
    let result = jobs_cancel(State(app.clone()), HeaderMap::new(), Path(id))
        .await
        .unwrap()
        .0;
    assert_eq!(result["job"]["status"], "cancelled");
    worker.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    assert!(!path.exists());
    assert_eq!(
        app.queue.lock().await.job.as_ref().unwrap().items[0].status,
        "cancelled"
    );
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn retry_failed_only_preserves_completed_files() {
    let (app, port, good, flaky, server) = fixture().await;
    let entries = vec![
        item(
            &app,
            "9709_s23_qp_12.pdf",
            format!("http://127.0.0.1:{port}/good"),
        ),
        item(
            &app,
            "9709_s23_ms_12.pdf",
            format!("http://127.0.0.1:{port}/flaky"),
        ),
    ];
    let (id, cancel) = set_job(&app, entries).await;
    run_job(app.clone(), id.clone(), cancel).await;
    let first = terminal(&app).await;
    assert_eq!(first.completed, 1);
    assert_eq!(first.failed, 1);
    let _ = jobs_retry(State(app.clone()), HeaderMap::new(), Path(id))
        .await
        .unwrap();
    let second = terminal(&app).await;
    assert_eq!(second.completed, 2);
    assert_eq!(good.load(Ordering::SeqCst), 1);
    assert_eq!(flaky.load(Ordering::SeqCst), 2);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn startup_marks_interrupted_without_network_and_preserves_done() {
    let (app, _, _, _, server) = fixture().await;
    let mut completed = item(
        &app,
        "9709_s23_qp_12.pdf",
        "https://cie.fraft.cn/obj/Common/Fetch/redir/9709_s23_qp_12.pdf".into(),
    );
    completed.status = "completed".into();
    let mut active = item(
        &app,
        "9709_s23_ms_12.pdf",
        "https://cie.fraft.cn/obj/Common/Fetch/redir/9709_s23_ms_12.pdf".into(),
    );
    active.status = "downloading".into();
    tokio::fs::create_dir_all(app.root.join("output"))
        .await
        .unwrap();
    tokio::fs::write(&completed.destination, b"keep")
        .await
        .unwrap();
    set_job(&app, vec![completed.clone(), active]).await;
    persist(&app, app.queue.lock().await.job.as_ref().unwrap())
        .await
        .unwrap();
    let restarted = App {
        queue: Default::default(),
        client: app.client.clone(),
        files: Default::default(),
        io: Default::default(),
        root: app.root.clone(),
    };
    restore_queue(&restarted).await.unwrap();
    let job = restarted.queue.lock().await.job.clone().unwrap();
    assert_eq!(job.status, "interrupted");
    assert_eq!(job.completed, 1);
    assert_eq!(job.interrupted, 1);
    assert_eq!(
        tokio::fs::read(completed.destination).await.unwrap(),
        b"keep"
    );
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn verified_existing_queue_file_skips_network() {
    let (app, port, good, _, server) = fixture().await;
    let entry = item(
        &app,
        "9709_s23_qp_12.pdf",
        format!("http://127.0.0.1:{port}/good"),
    );
    tokio::fs::create_dir_all(app.root.join("output"))
        .await
        .unwrap();
    commit_pdf(
        &app,
        &entry.file.paper(),
        b"%PDF-1.7\ngood\n%%EOF",
        std::path::Path::new(&entry.destination),
    )
    .await
    .unwrap();
    let (id, cancel) = set_job(&app, vec![entry]).await;
    run_job(app.clone(), id, cancel).await;
    let job = terminal(&app).await;
    assert_eq!(job.completed, 1);
    assert!(job.items[0].reused_existing);
    assert_eq!(good.load(Ordering::SeqCst), 0);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}

#[tokio::test]
async fn persistence_failure_before_work_and_retry_preserves_retryable_state() {
    let (app, port, good, _, server) = fixture().await;
    let a = item(
        &app,
        "9709_s23_qp_12.pdf",
        format!("http://127.0.0.1:{port}/good"),
    );
    let b = item(
        &app,
        "9709_s23_ms_12.pdf",
        format!("http://127.0.0.1:{port}/good"),
    );
    let (id, cancel) = set_job(&app, vec![a, b]).await;
    tokio::fs::create_dir(app.root.join(".queue.json"))
        .await
        .unwrap();
    run_job(app.clone(), id.clone(), cancel).await;
    let before = app.queue.lock().await.job.clone().unwrap();
    assert_eq!(before.status, "failed");
    assert_eq!(before.interrupted, 1);
    assert_eq!(good.load(Ordering::SeqCst), 0);
    assert!(jobs_retry(State(app.clone()), HeaderMap::new(), Path(id))
        .await
        .is_err());
    let after = app.queue.lock().await.job.clone().unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn persistence_failure_after_commit_keeps_original_and_remaining_retryable() {
    let (app, port, _, _, server) = fixture().await;
    let a = item(
        &app,
        "9709_s23_qp_12.pdf",
        format!("http://127.0.0.1:{port}/slow"),
    );
    let dest = PathBuf::from(&a.destination);
    let b = item(
        &app,
        "9709_s23_ms_12.pdf",
        format!("http://127.0.0.1:{port}/good"),
    );
    let (id, cancel) = set_job(&app, vec![a, b]).await;
    let worker = tokio::spawn(run_job(app.clone(), id, cancel));
    loop {
        if app.queue.lock().await.job.as_ref().unwrap().items[0].downloaded_bytes > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    tokio::fs::remove_file(app.root.join(".queue.json"))
        .await
        .unwrap();
    tokio::fs::create_dir(app.root.join(".queue.json"))
        .await
        .unwrap();
    worker.await.unwrap();
    let job = app.queue.lock().await.job.clone().unwrap();
    assert_eq!(job.completed, 1);
    assert_eq!(job.interrupted, 1);
    assert_eq!(job.status, "failed");
    assert_eq!(
        tokio::fs::read(dest).await.unwrap(),
        b"%PDF-1.7\nbody\n%%EOF"
    );
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn directory_symlink_rejected_before_any_external_creation() {
    let (app, _, _, _, server) = fixture().await;
    let base = app.root.join("output");
    let outside = app.root.join("outside");
    tokio::fs::create_dir(&base).await.unwrap();
    tokio::fs::create_dir(&outside).await.unwrap();
    std::os::unix::fs::symlink(&outside, base.join("2023")).unwrap();
    assert!(safe_parent(&base, &base.join("2023/QP")).await.is_err());
    assert!(tokio::fs::read_dir(&outside)
        .await
        .unwrap()
        .next_entry()
        .await
        .unwrap()
        .is_none());
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
