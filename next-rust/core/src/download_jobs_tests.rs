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
        source: default_paper_source(),
        logical_path: None,
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
        source_mode: default_source_mode(),
        download_concurrency: 1,
        requests_per_second: default_rate(),
        cooldown_until_ms: None,
        waiting_reason: None,
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

#[test]
fn retry_after_seconds_dates_and_invalid_values_are_bounded() {
    use std::time::{Duration, UNIX_EPOCH};
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    assert_eq!(retry_after(None, now).as_secs(), 30);
    assert_eq!(retry_after(Some("garbage"), now).as_secs(), 30);
    assert_eq!(retry_after(Some("0"), now).as_secs(), 5);
    assert_eq!(retry_after(Some("999999"), now).as_secs(), 120);
    assert_eq!(retry_after(Some("17"), now).as_secs(), 17);
    assert_eq!(
        retry_after(
            Some(&httpdate::fmt_http_date(now + Duration::from_secs(43))),
            now
        )
        .as_secs(),
        43
    );
    assert_eq!(
        retry_after(
            Some(&httpdate::fmt_http_date(now - Duration::from_secs(10))),
            now
        )
        .as_secs(),
        5
    );
}
#[tokio::test]
async fn shared_gate_serializes_admissions_and_cancel_wakes_every_waiter() {
    let (app, port, _, _, server) = fixture().await;
    let entries = (11..15)
        .map(|n| {
            item(
                &app,
                &format!("9709_s23_qp_{n}.pdf"),
                format!("http://127.0.0.1:{port}/good"),
            )
        })
        .collect();
    let (id, cancel) = set_job(&app, entries).await;
    let gate = Arc::new(RequestGate::new(20, None));
    let start = tokio::time::Instant::now();
    let mut tasks = vec![];
    for index in 0..4 {
        let (app, id, cancel, gate) = (app.clone(), id.clone(), cancel.clone(), gate.clone());
        tasks.push(tokio::spawn(async move {
            gate.acquire(&app, &id, index, &cancel).await.unwrap();
            tokio::time::Instant::now()
        }));
    }
    let mut times = vec![];
    for task in tasks {
        times.push(task.await.unwrap());
    }
    times.sort();
    assert!(times[3] - start >= std::time::Duration::from_millis(145));
    for pair in times.windows(2) {
        assert!(pair[1] - pair[0] >= std::time::Duration::from_millis(45));
    }
    gate.cooldown(&app, &id, std::time::Duration::from_secs(30))
        .await;
    let mut tasks = vec![];
    for index in 0..4 {
        let (app, id, cancel, gate) = (app.clone(), id.clone(), cancel.clone(), gate.clone());
        tasks.push(tokio::spawn(async move {
            gate.acquire(&app, &id, index, &cancel).await
        }));
    }
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    cancel.cancel();
    for task in tasks {
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn concurrent_streams_obey_worker_bound_and_cancel_without_commit() {
    let (app, port, _, _, server) = fixture().await;
    let entries = (11..17)
        .map(|n| {
            item(
                &app,
                &format!("9709_s23_qp_{n}.pdf"),
                format!("http://127.0.0.1:{port}/slow"),
            )
        })
        .collect();
    let (id, cancel) = set_job(&app, entries).await;
    {
        let mut q = app.queue.lock().await;
        let job = q.job.as_mut().unwrap();
        job.download_concurrency = 2;
        job.requests_per_second = 20;
    }
    let worker = tokio::spawn(run_job(app.clone(), id.clone(), cancel));
    let mut simultaneous = false;
    for _ in 0..50 {
        let job = app.queue.lock().await.job.clone().unwrap();
        let active = job.items.iter().filter(|i| i.downloaded_bytes > 0).count();
        assert!(active <= 2);
        if active == 2 {
            simultaneous = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(simultaneous);
    let _ = jobs_cancel(State(app.clone()), HeaderMap::new(), Path(id))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_millis(200), worker)
        .await
        .unwrap()
        .unwrap();
    let job = app.queue.lock().await.job.clone().unwrap();
    assert_eq!(job.cancelled, 6);
    for i in job.items {
        assert!(!PathBuf::from(i.destination).exists());
    }
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn http429_pauses_pending_requests_without_automatic_retry() {
    let (app, port, good, _, server) = fixture().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let router = Router::new().route(
        "/limited",
        get(move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "5")],
                    "limited",
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let limited_port = listener.local_addr().unwrap().port();
    let limited_server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let entries = vec![
        item(
            &app,
            "9709_s23_qp_12.pdf",
            format!("http://127.0.0.1:{limited_port}/limited"),
        ),
        item(
            &app,
            "9709_s23_ms_12.pdf",
            format!("http://127.0.0.1:{port}/good"),
        ),
    ];
    let (id, cancel) = set_job(&app, entries).await;
    let worker = tokio::spawn(run_job(app.clone(), id.clone(), cancel));
    for _ in 0..100 {
        if app.queue.lock().await.job.as_ref().unwrap().items[1].status == "waiting" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let job = app.queue.lock().await.job.clone().unwrap();
    assert_eq!(job.items[0].status, "failed");
    assert_eq!(job.items[1].status, "waiting");
    assert!(job.cooldown_until_ms.unwrap() > epoch_ms());
    assert_eq!(job.waiting_reason.as_deref(), Some("429"));
    assert_eq!(good.load(Ordering::SeqCst), 0);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let _ = jobs_cancel(State(app.clone()), HeaderMap::new(), Path(id))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_millis(200), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(good.load(Ordering::SeqCst), 0);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    server.abort();
    limited_server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}

#[tokio::test]
async fn parallel_persistence_failure_blocks_other_late_commit() {
    let (app, port, _, _, server) = fixture().await;
    let entries = ["9709_s23_qp_12.pdf", "9709_s23_ms_12.pdf"]
        .into_iter()
        .map(|name| item(&app, name, format!("http://127.0.0.1:{port}/slow")))
        .collect();
    let (id, cancel) = set_job(&app, entries).await;
    {
        let mut q = app.queue.lock().await;
        let job = q.job.as_mut().unwrap();
        job.download_concurrency = 2;
        job.requests_per_second = 20;
    }
    let worker = tokio::spawn(run_job(app.clone(), id, cancel));
    for _ in 0..100 {
        if app
            .queue
            .lock()
            .await
            .job
            .as_ref()
            .unwrap()
            .items
            .iter()
            .all(|i| i.downloaded_bytes > 0)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app
        .queue
        .lock()
        .await
        .job
        .as_ref()
        .unwrap()
        .items
        .iter()
        .all(|i| i.downloaded_bytes > 0));
    tokio::fs::remove_file(app.root.join(".queue.json"))
        .await
        .unwrap();
    tokio::fs::create_dir(app.root.join(".queue.json"))
        .await
        .unwrap();
    worker.await.unwrap();
    let job = app.queue.lock().await.job.clone().unwrap();
    assert_eq!(job.status, "failed");
    assert_eq!(job.completed, 1);
    assert_eq!(job.interrupted, 1);
    assert_eq!(
        job.items
            .iter()
            .filter(|i| PathBuf::from(&i.destination).exists())
            .count(),
        1
    );
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn restored_waiting_job_retains_frozen_limits_and_never_autoruns() {
    let (app, _, good, _, server) = fixture().await;
    let mut entry = item(
        &app,
        "9709_s23_qp_12.pdf",
        "https://cie.fraft.cn/obj/Common/Fetch/redir/9709_s23_qp_12.pdf".into(),
    );
    entry.status = "waiting".into();
    set_job(&app, vec![entry]).await;
    {
        let mut q = app.queue.lock().await;
        let job = q.job.as_mut().unwrap();
        job.download_concurrency = 7;
        job.requests_per_second = 3;
        job.source_mode = "frankcie".into();
        job.cooldown_until_ms = Some(epoch_ms() + 30_000);
        persist(&app, job).await.unwrap();
    }
    restore_queue(&app).await.unwrap();
    let job = app.queue.lock().await.job.clone().unwrap();
    assert_eq!(job.status, "interrupted");
    assert_eq!(job.items[0].status, "interrupted");
    assert_eq!(job.download_concurrency, 7);
    assert_eq!(job.requests_per_second, 3);
    assert_eq!(job.source_mode, "frankcie");
    assert!(job.cooldown_until_ms.is_some());
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert_eq!(good.load(Ordering::SeqCst), 0);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}

#[tokio::test]
async fn identical_bytes_preserve_and_reuse_only_verified_source_provenance() {
    let (app, _, _, _, server) = fixture().await;
    let first = item(
        &app,
        "9709_s23_qp_12.pdf",
        "https://cie.fraft.cn/obj/Common/Fetch/redir/9709_s23_qp_12.pdf".into(),
    );
    tokio::fs::create_dir_all(app.root.join("output"))
        .await
        .unwrap();
    let mut second = first.file.paper();
    second.source = "papacambridge".into();
    second.id = paper_id(&second.source, &second.filename);
    second.source_url = format!(
        "https://pastpapers.papacambridge.com/directories/CAIE/CAIE-pastpapers/upload/{}",
        second.filename
    );
    let path = std::path::Path::new(&first.destination);
    let bytes = b"%PDF-1.7\nshared\n%%EOF";
    commit_pdf(&app, &first.file.paper(), bytes, path)
        .await
        .unwrap();
    assert!(cached_saved(&app, &second).await.unwrap().is_none());
    commit_pdf(&app, &second, bytes, path).await.unwrap();
    assert!(cached_saved(&app, &first.file.paper())
        .await
        .unwrap()
        .is_some());
    assert!(cached_saved(&app, &second).await.unwrap().is_some());
    let record: Value = read_control(&app.root.join(format!("{}.download.json", second.filename)))
        .await
        .unwrap();
    assert_eq!(record["source"], "papacambridge");
    assert_eq!(record["verified_sources"].as_array().unwrap().len(), 2);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}

#[tokio::test]
async fn replacement_job_inherits_unexpired_cooldown_and_cancel_wakes_gate() {
    let (app, _, _, _, server) = fixture().await;
    let entry = item(
        &app,
        "9709_s23_qp_12.pdf",
        "https://cie.fraft.cn/obj/Common/Fetch/redir/9709_s23_qp_12.pdf".into(),
    );
    set_job(&app, vec![entry.clone()]).await;
    let until = epoch_ms() + 30_000;
    {
        let mut q = app.queue.lock().await;
        let job = q.job.as_mut().unwrap();
        job.status = "cancelled".into();
        job.items[0].status = "cancelled".into();
        job.cooldown_until_ms = Some(until);
    }
    let token = "c".repeat(64);
    let plan = json!({"token":token,"files":[entry.file],"queries":[],"complete":true,"count":1,"save_directory":app.root.join("output").to_string_lossy(),"merge_folders":true,"source_mode":"frankcie","download_concurrency":2,"requests_per_second":3});
    atomic_new(
        &preview_path(&app, &token).unwrap(),
        &serde_json::to_vec(&plan).unwrap(),
    )
    .await
    .unwrap();
    let response = jobs_start(
        State(app.clone()),
        HeaderMap::new(),
        Json(StartJob {
            preview_token: token.clone(),
            accept_partial: false,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(response["job"]["cooldown_until_ms"], until);
    let _ = jobs_cancel(State(app.clone()), HeaderMap::new(), Path(token))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!PathBuf::from(entry.destination).exists());
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
