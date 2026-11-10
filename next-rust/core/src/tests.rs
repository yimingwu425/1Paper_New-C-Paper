use super::*;
#[test]
fn filename() {
    assert_eq!(parse("9709_s23_qp_12.pdf").unwrap().3, "12");
}
#[test]
fn traversal() {
    for s in [
        "../9709_s23_qp_12.pdf",
        "9709_s23_qp_12.pdf/",
        "9709_s23_qp_12.pdf?x",
        "/tmp/x.pdf",
        "9709_s23_qp_12.PDF",
    ] {
        assert!(parse(s).is_none())
    }
}
#[test]
fn redirect() {
    for s in [
        "http://cie.fraft.cn/x",
        "https://127.0.0.1/x",
        "https://cie.fraft.cn.evil.test/x",
        "https://cie.fraft.cn:8443/x",
    ] {
        assert!(!allowed(&s.parse().unwrap()))
    }
    assert!(allowed(&"https://cie.fraft.cn/x".parse().unwrap()));
}
#[test]
fn collect_only_papers() {
    let mut v = vec![];
    collect(
        &json!({"rows":[{"file":"9709_s23_ms_12.pdf","lesson":"https://elsewhere"}]}),
        &mut v,
    );
    assert_eq!(v.len(), 1);
}
#[test]
fn query_validation() {
    assert!(query_parts(&Search {
        subject: "../../".into(),
        year: 2023,
        season: "s".into()
    })
    .is_err());
    assert_eq!(
        query_parts(&Search {
            subject: "9709".into(),
            year: 2023,
            season: "s".into()
        })
        .unwrap()
        .0,
        "Jun"
    );
}
fn mock_app(body: Vec<u8>) -> (App, Paper) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf);
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        socket.write_all(&body).unwrap();
    });
    let root = std::env::temp_dir().join(format!(
        "cpaper-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let paper = Paper {
        id: "9709_s23_qp_12.pdf".into(),
        filename: "9709_s23_qp_12.pdf".into(),
        kind: "qp".into(),
        source_url: format!("http://{addr}/test"),
        source: default_paper_source(),
        logical_path: None,
    };
    (
        App {
            queue: Default::default(),
            client: reqwest::Client::new(),
            files: Default::default(),
            io: Default::default(),
            root,
        },
        paper,
    )
}
#[tokio::test]
async fn atomic_save_and_hash() {
    let body = b"%PDF-1.7\nsample\n%%EOF".to_vec();
    let (app, p) = mock_app(body.clone());
    let result = save(&app, &p).await.unwrap();
    assert_eq!(result["sha256"], format!("{:x}", Sha256::digest(&body)));
    assert_eq!(
        tokio::fs::read(app.root.join(&p.filename)).await.unwrap(),
        body
    );
    assert!(!app.root.join(format!(".{}.part", p.filename)).exists());
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn bad_pdf_never_lands() {
    let (app, p) = mock_app(b"<html>not a PDF</html>".to_vec());
    assert!(save(&app, &p).await.is_err());
    assert!(!app.root.exists());
}
#[tokio::test]
async fn collision_preserves_original() {
    let (app, p) = mock_app(b"%PDF-1.7\nnew\n%%EOF".to_vec());
    tokio::fs::create_dir_all(&app.root).await.unwrap();
    let dest = app.root.join(&p.filename);
    tokio::fs::write(&dest, b"original").await.unwrap();
    assert_eq!(save(&app, &p).await.unwrap_err().0, StatusCode::CONFLICT);
    assert_eq!(tokio::fs::read(dest).await.unwrap(), b"original");
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[tokio::test]
async fn symlink_target_rejected() {
    let (app, p) = mock_app(b"%PDF-1.7\nnew\n%%EOF".to_vec());
    tokio::fs::create_dir_all(&app.root).await.unwrap();
    let original = app.root.join("keep");
    tokio::fs::write(&original, b"original").await.unwrap();
    std::os::unix::fs::symlink(&original, app.root.join(&p.filename)).unwrap();
    assert_eq!(save(&app, &p).await.unwrap_err().0, StatusCode::BAD_REQUEST);
    assert_eq!(tokio::fs::read(original).await.unwrap(), b"original");
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}

#[test]
fn study_ports_are_explicit_local_only() {
    for port in [0, 80, 443, 47831, 47832] {
        assert!(study_base(port).is_err())
    }
    assert_eq!(study_base(57338).unwrap(), "http://127.0.0.1:57338");
}
#[tokio::test]
async fn fake_study_identity_rejected() {
    let (_app, p) = mock_app(
        br#"{"status":"ok","core":"other","mode":"local","pdf_engine":"PdfCraft 0.20.0"}"#.to_vec(),
    );
    let port = reqwest::Url::parse(&p.source_url).unwrap().port().unwrap();
    assert_eq!(probe_study(port).await.unwrap_err().0, StatusCode::CONFLICT);
}
#[tokio::test]
async fn empty_batch_rejected() {
    let app = App {
        queue: Default::default(),
        client: reqwest::Client::new(),
        files: Default::default(),
        io: Default::default(),
        root: std::env::temp_dir().join("unused-cpaper"),
    };
    assert!(
        batch(State(app), HeaderMap::new(), Json(Batch { ids: vec![] }))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn unconfirmed_import_rejected() {
    let app = App {
        queue: Default::default(),
        client: reqwest::Client::new(),
        files: Default::default(),
        io: Default::default(),
        root: std::env::temp_dir().join("unused-cpaper"),
    };
    assert_eq!(
        study_import(
            State(app),
            HeaderMap::new(),
            Path("9709_s23_qp_12.pdf".into()),
            Json(StudyConfirm {
                port: 57338,
                confirmed: false
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn history_detects_changed_and_missing() {
    let (app, p) = mock_app(b"%PDF-1.7\nsample\n%%EOF".to_vec());
    save(&app, &p).await.unwrap();
    let listed = downloads(State(app.clone())).await.unwrap().0;
    assert_eq!(listed["downloads"][0]["status"], "available");
    tokio::fs::write(app.root.join(&p.filename), b"changed")
        .await
        .unwrap();
    assert_eq!(
        downloads(State(app.clone())).await.unwrap().0["downloads"][0]["status"],
        "changed"
    );
    tokio::fs::remove_file(app.root.join(&p.filename))
        .await
        .unwrap();
    assert_eq!(
        downloads(State(app.clone())).await.unwrap().0["downloads"][0]["status"],
        "missing"
    );
    tokio::fs::remove_dir_all(app.root).await.unwrap();
}
#[test]
fn foreign_origin() {
    let mut h = HeaderMap::new();
    h.insert("origin", "https://evil.test".parse().unwrap());
    assert!(check_origin(&h).is_err());
}

#[test]
fn import_receipt_requires_exact_identity() {
    let good = json!({"imports":[{"id":"abc","filename":"9709_s23_qp_12.pdf","hash":"expected","status":"awaiting_agent"}]});
    assert!(validate_receipt(&good, "9709_s23_qp_12.pdf", "expected").is_ok());
    for bad_value in [
        json!({"imports":[]}),
        json!({}),
        json!({"imports":[{"id":"abc","filename":"other.pdf","hash":"expected","status":"awaiting_agent"}]}),
        json!({"imports":[{"id":"abc","filename":"9709_s23_qp_12.pdf","hash":"wrong","status":"awaiting_agent"}]}),
        json!({"imports":[{"filename":"9709_s23_qp_12.pdf","hash":"expected","status":"awaiting_agent"}]}),
        json!({"imports":[{"id":"abc","filename":"9709_s23_qp_12.pdf","hash":"expected","status":"unknown"}]}),
    ] {
        assert!(validate_receipt(&bad_value, "9709_s23_qp_12.pdf", "expected").is_err());
    }
}
