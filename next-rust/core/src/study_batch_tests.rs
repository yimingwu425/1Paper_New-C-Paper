use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
static ROOT_SEQ: AtomicUsize = AtomicUsize::new(0);
fn expected() -> Vec<ExpectedImport> {
    ["9709_s23_qp_12.pdf", "9709_s23_ms_12.pdf"]
        .into_iter()
        .map(|name| ExpectedImport {
            id: name.into(),
            filename: name.into(),
            hash: format!("{:x}", Sha256::digest(format!("%PDF-1.7\n{name}\n%%EOF"))),
            source_url: "test".into(),
        })
        .collect()
}
fn response(partial: bool) -> Value {
    let files = expected();
    let ids = [
        "00000000-0000-0000-0000-000000000001",
        "00000000-0000-0000-0000-000000000002",
    ];
    let imports:Vec<_>=files.iter().enumerate().map(|(i,f)|json!({"id":ids[i],"filename":f.filename,"hash":f.hash,"status":if i==1&&partial{"failed"}else{"awaiting_agent"},"duplicate":false})).collect();
    let sources: Vec<_> = files
        .iter()
        .enumerate()
        .filter(|(i, _)| !partial || *i == 0)
        .map(|(i, f)| json!({"id":ids[i],"filename":f.filename,"sha256":f.hash}))
        .collect();
    json!({"imports":imports,"recognition_job":{"id":"00000000-0000-0000-0000-000000000003","status":"awaiting_agent","sources":sources}})
}
#[test]
fn batch_exact_maps_and_partial() {
    assert!(validate_batch_receipt(&response(false), &expected()).is_ok());
    assert!(validate_batch_receipt(&response(true), &expected()).is_ok());
    for field in ["filename", "hash", "id"] {
        let mut value = response(false);
        value["imports"][0][field] = json!("wrong");
        assert!(validate_batch_receipt(&value, &expected()).is_err());
    }
    let mut value = response(false);
    value["imports"] = json!([]);
    assert!(validate_batch_receipt(&value, &expected()).is_err());
    let mut value = response(false);
    value["imports"][1] = value["imports"][0].clone();
    assert!(validate_batch_receipt(&value, &expected()).is_err());
    let mut value = response(false);
    value["recognition_job"]["sources"][0]["filename"] = json!("wrong.pdf");
    assert!(validate_batch_receipt(&value, &expected()).is_err());
    let mut value = response(false);
    value["recognition_job"]["sources"][0]["sha256"] = json!("wrong");
    assert!(validate_batch_receipt(&value, &expected()).is_err());
}
async fn scenario(mode: &str) -> (App, u16, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let root = std::env::temp_dir().join(format!(
        "cpaper-batch-test-{}-{}-{}",
        std::process::id(),
        ROOT_SEQ.fetch_add(1, Ordering::SeqCst),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let count = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    let m = mode.to_owned();
    let root_clone = root.clone();
    let receiver=Router::new().route("/api/health",get(||async{Json(json!({"status":"ok","core":"rust","mode":"local","pdf_engine":"PdfCraft test"}))}))
    .route("/pdf/{name}",get(|Path(name):Path<String>|async move{format!("%PDF-1.7\n{name}\n%%EOF")}))
    .route("/api/imports",post(move ||{let count=counter.clone();let mode=m.clone();let root=root_clone.clone();async move {
        count.fetch_add(1,Ordering::SeqCst);
        if mode=="write-fail" {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&root,std::fs::Permissions::from_mode(0o500)).await.unwrap();
        }
        if mode=="old-orphan" {
            let mut dir=tokio::fs::read_dir(&root).await.unwrap();
            while let Some(entry)=dir.next_entry().await.unwrap(){
                if entry.file_name().to_string_lossy().starts_with("batch-"){
                    tokio::fs::write(entry.path().with_extension("update.part"),b"old orphan").await.unwrap();
                }
            }
        }
        Json(if mode=="invalid"{json!({"imports":[]})}else{response(mode=="partial")})
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move { axum::serve(listener, receiver).await.unwrap() });
    let app = App {
        queue: Default::default(),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        files: Default::default(),
        io: Default::default(),
        root,
    };
    for e in expected() {
        app.files.lock().await.insert(
            e.id.clone(),
            Paper {
                id: e.id,
                filename: e.filename.clone(),
                kind: if e.filename.contains("_qp_") {
                    "qp"
                } else {
                    "ms"
                }
                .into(),
                source_url: format!("http://127.0.0.1:{port}/pdf/{}", e.filename),
                source: default_paper_source(),
                logical_path: None,
            },
        );
    }
    (app, port, count, handle)
}
fn request(port: u16) -> Json<StudyBatch> {
    Json(StudyBatch {
        port,
        confirmed: true,
        ids: expected().into_iter().map(|f| f.id).collect(),
    })
}
#[tokio::test]
async fn batch_success_replayed_after_restart_without_second_post() {
    let (app, port, count, server) = scenario("ok").await;
    let first = study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
        .await
        .unwrap()
        .0;
    assert_eq!(first["operation_state"], "complete");
    assert_eq!(first["imports"].as_array().unwrap().len(), 2);
    app.files.lock().await.clear();
    let second = study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
        .await
        .unwrap()
        .0;
    assert_eq!(second["replayed"], true);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        receipts(State(app.clone())).await.unwrap().0["receipts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn partial_precheck_reports_one_failed() {
    let (app, port, count, server) = scenario("partial").await;
    let value = study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
        .await
        .unwrap()
        .0;
    assert_eq!(value["precheck_failed"], 1);
    assert_eq!(value["published"], false);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn unknown_remote_result_is_not_resent_after_restart() {
    let (app, port, count, server) = scenario("invalid").await;
    assert_eq!(
        study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
            .await
            .unwrap_err()
            .0,
        StatusCode::BAD_GATEWAY
    );
    app.files.lock().await.clear();
    let retry = study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
        .await
        .unwrap_err();
    assert_eq!(retry.0, StatusCode::CONFLICT);
    assert_eq!(retry.1 .0["outcome"], "uncertain");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn local_receipt_failure_does_not_resend() {
    let (app, port, count, server) = scenario("write-fail").await;
    assert!(
        study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
            .await
            .is_err()
    );
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(&app.root, std::fs::Permissions::from_mode(0o700))
        .await
        .unwrap();
    app.files.lock().await.clear();
    assert_eq!(
        study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[test]
fn catalog_preserves_real_codes_and_deduplicates() {
    let value = json!([{"value":"9709","text":"数学"},{"code":"9709","name":"重复"},{"id":"9702","title":"物理"},{"value":"../x","text":"invalid"}]);
    let subjects = parse_subjects(&value);
    assert_eq!(subjects.len(), 2);
    assert_eq!(subjects[0].code, "9702");
    assert_eq!(subjects[1].name, "数学");
}

#[test]
fn subject_post_explicit_empty_content_length() {
    let req = subject_request(&reqwest::Client::new()).build().unwrap();
    assert_eq!(req.headers()["content-length"], "0");
    assert_eq!(req.method(), reqwest::Method::POST);
}

#[tokio::test]
async fn corrupted_receipt_object_rejected_without_panic() {
    let (app, _, _, server) = scenario("ok").await;
    tokio::fs::create_dir_all(&app.root).await.unwrap();
    let file = app.root.join(format!("batch-{}.json", "0".repeat(64)));
    tokio::fs::write(file, b"[]").await.unwrap();
    assert!(receipts(State(app.clone())).await.is_err());
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}

#[tokio::test]
async fn orphan_pdf_and_json_do_not_block_and_are_preserved() {
    let (app, _, _, server) = scenario("ok").await;
    tokio::fs::create_dir_all(&app.root).await.unwrap();
    let id = "9709_s23_qp_12.pdf";
    let orphan_pdf = app.root.join(format!(".{id}.part"));
    let orphan_json = app.root.join(format!("{id}.download.json.part"));
    tokio::fs::write(&orphan_pdf, b"old pdf staging")
        .await
        .unwrap();
    tokio::fs::write(&orphan_json, b"old json staging")
        .await
        .unwrap();
    let paper = lookup(&app, id).await.unwrap();
    let first = save(&app, &paper).await.unwrap();
    assert!(first["path"].is_string());
    assert_eq!(
        tokio::fs::read(&orphan_pdf).await.unwrap(),
        b"old pdf staging"
    );
    assert_eq!(
        tokio::fs::read(&orphan_json).await.unwrap(),
        b"old json staging"
    );
    // A new App handle represents reloading the same isolated on-disk state.
    let restarted = App {
        queue: Default::default(),
        client: app.client.clone(),
        files: Default::default(),
        io: Default::default(),
        root: app.root.clone(),
    };
    assert!(save(&restarted, &paper).await.is_ok());
    tokio::fs::write(app.root.join(id), b"keep original")
        .await
        .unwrap();
    assert_eq!(
        save(&restarted, &paper).await.unwrap_err().0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        tokio::fs::read(app.root.join(id)).await.unwrap(),
        b"keep original"
    );
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn old_journal_update_orphan_is_not_reused_or_removed() {
    let (app, port, count, server) = scenario("old-orphan").await;
    let result = study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
        .await
        .unwrap()
        .0;
    let orphan = std::path::PathBuf::from(result["receipt_path"].as_str().unwrap())
        .with_extension("update.part");
    assert_eq!(tokio::fs::read(orphan).await.unwrap(), b"old orphan");
    assert_eq!(result["operation_state"], "complete");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn pending_subset_superset_and_partial_overlap_block_after_restart() {
    let (app, port, count, server) = scenario("invalid").await;
    let first = study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
        .await
        .unwrap_err();
    let original = first.1 .0["receipt_id"].clone();
    app.files.lock().await.clear();
    for ids in [
        vec!["9709_s23_qp_12.pdf"],
        vec![
            "9709_s23_qp_12.pdf",
            "9709_s23_ms_12.pdf",
            "9709_s23_qp_13.pdf",
        ],
        vec!["9709_s23_qp_13.pdf", "9709_s23_ms_12.pdf"],
    ] {
        let result = study_import_batch(
            State(app.clone()),
            HeaderMap::new(),
            Json(StudyBatch {
                port,
                confirmed: true,
                ids: ids.into_iter().map(str::to_owned).collect(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(result.0, StatusCode::CONFLICT);
        assert_eq!(result.1 .0["receipt_id"], original);
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn concurrent_overlapping_submissions_make_one_remote_post() {
    let (app, port, count, server) = scenario("invalid").await;
    let subset = Json(StudyBatch {
        port,
        confirmed: true,
        ids: vec!["9709_s23_qp_12.pdf".into()],
    });
    let (a, b) = tokio::join!(
        study_import_batch(State(app.clone()), HeaderMap::new(), request(port)),
        study_import_batch(State(app.clone()), HeaderMap::new(), subset)
    );
    let mut codes = vec![a.unwrap_err().0.as_u16(), b.unwrap_err().0.as_u16()];
    codes.sort();
    assert_eq!(codes, vec![409, 502]);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        receipts(State(app.clone())).await.unwrap().0["receipts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn nonoverlapping_pending_submissions_are_not_globally_blocked() {
    let (app, port, count, server) = scenario("invalid").await;
    for id in ["9709_s23_qp_12.pdf", "9709_s23_ms_12.pdf"] {
        let result = study_import_batch(
            State(app.clone()),
            HeaderMap::new(),
            Json(StudyBatch {
                port,
                confirmed: true,
                ids: vec![id.into()],
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(result.0, StatusCode::BAD_GATEWAY);
    }
    assert_eq!(count.load(Ordering::SeqCst), 2);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn unresolved_overlap_takes_priority_over_completed_replay() {
    let (app, port, count, server) = scenario("ok").await;
    let _ = study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
        .await
        .unwrap();
    let id = format!("batch-{}.json", "0".repeat(64));
    tokio::fs::write(app.root.join(&id),serde_json::to_vec(&json!({"port":port,"operation_state":"submitted","expected_files":[{"id":"9709_s23_qp_12.pdf"}]})).unwrap()).await.unwrap();
    let result = study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
        .await
        .unwrap_err();
    assert_eq!(result.0, StatusCode::CONFLICT);
    assert_eq!(result.1 .0["receipt_id"], id);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}

#[tokio::test]
async fn json_atomic_new_preserves_final_and_old_orphan() {
    let (app, _, _, server) = scenario("ok").await;
    tokio::fs::create_dir_all(&app.root).await.unwrap();
    let path = app.root.join("receipt.json");
    let orphan = path.with_extension("json.part");
    tokio::fs::write(&orphan, b"old json staging")
        .await
        .unwrap();
    atomic_new(&path, b"first").await.unwrap();
    assert!(atomic_new(&path, b"replacement").await.is_err());
    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"first");
    assert_eq!(tokio::fs::read(&orphan).await.unwrap(), b"old json staging");
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}

#[tokio::test]
async fn source_changes_cannot_bypass_pending_original_protection() {
    let (app, port, count, server) = scenario("invalid").await;
    let first = study_import_batch(State(app.clone()), HeaderMap::new(), request(port))
        .await
        .unwrap_err();
    app.files.lock().await.clear();
    for source in ["easypaper", "pastpapers", "papacambridge"] {
        let result = study_import_batch(
            State(app.clone()),
            HeaderMap::new(),
            Json(StudyBatch {
                port,
                confirmed: true,
                ids: vec![format!("{source}~9709_s23_qp_12.pdf")],
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(result.0, StatusCode::CONFLICT);
        assert_eq!(result.1 .0["receipt_id"], first.1 .0["receipt_id"]);
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn same_filename_from_two_sources_is_rejected_in_one_study_batch() {
    let (app, port, count, server) = scenario("ok").await;
    let result = study_import_batch(
        State(app.clone()),
        HeaderMap::new(),
        Json(StudyBatch {
            port,
            confirmed: true,
            ids: vec![
                "9709_s23_qp_12.pdf".into(),
                "easypaper~9709_s23_qp_12.pdf".into(),
            ],
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(result.0, StatusCode::BAD_REQUEST);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    server.abort();
    let _ = tokio::fs::remove_dir_all(app.root).await;
}
