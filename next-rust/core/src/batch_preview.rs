use super::*;
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PreviewFile {
    pub(super) id: String,
    pub(super) filename: String,
    pub(super) kind: String,
    pub(super) source_url: String,
    pub(super) destination: String,
}
impl PreviewFile {
    pub(super) fn paper(&self) -> Paper {
        Paper {
            id: self.id.clone(),
            filename: self.filename.clone(),
            kind: self.kind.clone(),
            source_url: self.source_url.clone(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct BatchPreview {
    pub(super) token: String,
    pub(super) files: Vec<PreviewFile>,
    pub(super) queries: Vec<Value>,
    pub(super) complete: bool,
    pub(super) count: usize,
    pub(super) save_directory: String,
    pub(super) merge_folders: bool,
}
#[derive(Deserialize)]
pub(super) struct PreviewRequest {
    subject: String,
    year_from: u16,
    year_to: u16,
    seasons: Vec<String>,
    papers: Vec<u8>,
    #[serde(default = "yes")]
    include_ms: bool,
    merge_folders: Option<bool>,
}
fn yes() -> bool {
    true
}
#[derive(Deserialize)]
pub(super) struct Selection {
    ids: Vec<String>,
}
pub(super) fn preview_path(app: &App, token: &str) -> Result<PathBuf, ErrApi> {
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad("预览token无效"));
    }
    Ok(app.root.join(format!(".preview-{token}.json")))
}
pub(super) fn freeze_file(p: &Paper, settings: &Settings) -> Result<PreviewFile, ErrApi> {
    let (_, sy, kind, _) = parse(&p.filename).ok_or_else(|| bad("文件名无效"))?;
    let mut path = PathBuf::from(&settings.download_dir);
    if !path.is_absolute() {
        return Err(bad("保存目录不是绝对路径"));
    }
    if !settings.merge_folders {
        path = path
            .join(format!("20{}", &sy[1..]))
            .join(kind.to_uppercase())
    }
    path = path.join(&p.filename);
    Ok(PreviewFile {
        id: p.id.clone(),
        filename: p.filename.clone(),
        kind: p.kind.clone(),
        source_url: p.source_url.clone(),
        destination: path.to_string_lossy().into(),
    })
}
pub(super) fn matches_selection(filename: &str, papers: &[u8], include_ms: bool) -> bool {
    let Some((_, _, kind, number)) = parse(filename) else {
        return false;
    };
    let n = number.parse::<u8>().unwrap_or(0);
    let group = if n >= 10 { n / 10 } else { n };
    papers.contains(&group) && (kind == "qp" || (include_ms && kind == "ms"))
}
static PREVIEW_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
async fn freeze(
    app: &App,
    settings: Settings,
    files: Vec<Paper>,
    queries: Vec<Value>,
) -> Result<BatchPreview, ErrApi> {
    let mut frozen = vec![];
    for p in files {
        frozen.push(freeze_file(&p, &settings)?)
    }
    frozen.sort_by(|a, b| a.filename.cmp(&b.filename));
    frozen.dedup_by(|a, b| a.filename == b.filename);
    if frozen.len() > 5000 {
        return Err(bad("清单超过5000文件，请缩小范围"));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(upstream)?
        .as_nanos();
    let token = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                std::process::id(),
                PREVIEW_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                stamp,
                &frozen,
                &settings
            ))
            .map_err(upstream)?
        )
    );
    let preview = BatchPreview {
        token: token.clone(),
        count: frozen.len(),
        files: frozen,
        complete: queries.iter().all(|q| q["status"] != "failed"),
        queries,
        save_directory: settings.download_dir,
        merge_folders: settings.merge_folders,
    };
    let _guard = app.io.lock().await;
    atomic_new(
        &preview_path(app, &token)?,
        &serde_json::to_vec_pretty(&preview).map_err(upstream)?,
    )
    .await?;
    Ok(preview)
}
pub(super) async fn batch_preview(
    State(app): State<App>,
    h: HeaderMap,
    Json(req): Json<PreviewRequest>,
) -> Result<Json<BatchPreview>, ErrApi> {
    check_origin(&h)?;
    if req.year_from < 2000
        || req.year_to > 2035
        || req.year_from > req.year_to
        || req.seasons.is_empty()
        || req.papers.is_empty()
        || req.papers.iter().any(|p| !(1..=6).contains(p))
    {
        return Err(bad("选择2000–2035年份范围、至少一个考季和P1–P6"));
    }
    let mut seasons = vec![];
    for s in &req.seasons {
        let (season, _) = query_parts(&Search {
            subject: req.subject.clone(),
            year: req.year_from,
            season: s.clone(),
        })?;
        if !seasons.contains(&season) {
            seasons.push(season)
        }
    }
    let mut settings = read_settings(&app).await?;
    if let Some(merge) = req.merge_folders {
        settings.merge_folders = merge
    }
    let queries: Vec<_> = (req.year_from..=req.year_to)
        .flat_map(|year| seasons.iter().map(move |season| (year, season.to_string())))
        .collect();
    let app_ref = app.clone();
    let subject = req.subject.clone();
    let results: Vec<_> = futures_util::stream::iter(queries)
        .map(|(year, season)| {
            let app = app_ref.clone();
            let subject = subject.clone();
            async move {
                let result = search(
                    State(app),
                    Query(Search {
                        subject,
                        year,
                        season: season.clone(),
                    }),
                )
                .await;
                (year, season, result)
            }
        })
        .buffer_unordered(3)
        .collect()
        .await;
    let (files, reports) = aggregate_results(results, &req.papers, req.include_ms)?;
    Ok(Json(freeze(&app, settings, files, reports).await?))
}
fn aggregate_results(
    results: Vec<(u16, String, Result<Json<Value>, ErrApi>)>,
    papers: &[u8],
    include_ms: bool,
) -> Result<(Vec<Paper>, Vec<Value>), ErrApi> {
    let mut files = vec![];
    let mut reports = vec![];
    for (year, season, result) in results {
        match result{
            Ok(Json(value))=>{
                let mut count=0;
                for group in value["groups"].as_array().into_iter().flatten(){
                    for v in group["files"].as_array().into_iter().flatten(){
                        let p:Paper=serde_json::from_value(v.clone()).map_err(upstream)?;
                        if matches_selection(&p.filename,papers,include_ms){files.push(p);count+=1}
                    }
                }
                reports.push(json!({"year":year,"season":season,"status":if count>0{"ok"}else{"empty"},"file_count":count}));
            },
            Err((_,Json(e)))=>reports.push(json!({"year":year,"season":season,"status":"failed","file_count":0,"error":e["error"]}))
        }
    }
    reports.sort_by_key(|v| {
        (
            v["year"].as_u64().unwrap_or(0),
            match v["season"].as_str() {
                Some("Mar") => 0,
                Some("Jun") => 1,
                _ => 2,
            },
        )
    });
    Ok((files, reports))
}
pub(super) async fn selection_preview(
    State(app): State<App>,
    h: HeaderMap,
    Json(req): Json<Selection>,
) -> Result<Json<BatchPreview>, ErrApi> {
    check_origin(&h)?;
    if req.ids.is_empty() || req.ids.len() > 5000 {
        return Err(bad("选择1–5000个文件"));
    }
    let settings = read_settings(&app).await?;
    let mut files = vec![];
    for id in &req.ids {
        files.push(lookup(&app, id).await?)
    }
    Ok(Json(freeze(&app, settings, files, vec![]).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn search_errors_are_explicit_and_partial_or_empty_start_is_blocked() {
        let root = std::env::temp_dir().join(format!("cpaper-aggregate-{}", std::process::id()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let root = tokio::fs::canonicalize(root).await.unwrap();
        let app = App {
            queue: Default::default(),
            client: reqwest::Client::new(),
            files: Default::default(),
            io: Default::default(),
            root: root.clone(),
        };
        let p = json!({"id":"9709_s23_qp_12.pdf","filename":"9709_s23_qp_12.pdf","kind":"qp","source_url":"https://cie.fraft.cn/obj/Common/Fetch/redir/9709_s23_qp_12.pdf"});
        let (files, reports) = aggregate_results(
            vec![
                (
                    2023,
                    "Jun".into(),
                    Ok(Json(json!({"groups":[{"files":[p]}]}))),
                ),
                (2024, "Nov".into(), Err(upstream("HTTP 503"))),
            ],
            &[1],
            true,
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(reports[1]["year"], 2024);
        assert_eq!(reports[1]["season"], "Nov");
        assert_eq!(reports[1]["status"], "failed");
        assert!(reports[1]["error"].as_str().unwrap().contains("503"));
        let settings = Settings {
            download_dir: root.to_string_lossy().into(),
            merge_folders: true,
        };
        let preview = freeze(&app, settings.clone(), files, reports)
            .await
            .unwrap();
        assert!(!preview.complete);
        let request =
            serde_json::from_value(json!({"preview_token":preview.token,"accept_partial":false}))
                .unwrap();
        assert!(
            jobs_start(State(app.clone()), HeaderMap::new(), Json(request))
                .await
                .is_err()
        );
        let (files, reports) = aggregate_results(
            vec![(2024, "Nov".into(), Err(upstream("HTTP 503")))],
            &[1],
            true,
        )
        .unwrap();
        let empty = freeze(&app, settings, files, reports).await.unwrap();
        assert_eq!(empty.count, 0);
        assert!(!empty.complete);
        let request =
            serde_json::from_value(json!({"preview_token":empty.token,"accept_partial":true}))
                .unwrap();
        assert!(
            jobs_start(State(app.clone()), HeaderMap::new(), Json(request))
                .await
                .is_err()
        );
        assert!(app.queue.lock().await.job.is_none());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
    #[test]
    fn explicit_selection_preserves_supported_extra_documents() {
        let settings = Settings {
            download_dir: "/tmp/cpaper".into(),
            merge_folders: false,
        };
        for kind in ["ci", "gt", "er", "ir", "in", "sr"] {
            let filename = format!("9709_s23_{kind}.pdf");
            let p = Paper {
                id: filename.clone(),
                filename: filename.clone(),
                kind: kind.into(),
                source_url: format!("https://cie.fraft.cn/obj/Common/Fetch/redir/{filename}"),
            };
            assert!(freeze_file(&p, &settings)
                .unwrap()
                .destination
                .ends_with(&format!("/2023/{}/{filename}", kind.to_uppercase())));
            assert!(!matches_selection(&filename, &[1, 2, 3, 4, 5, 6], true));
        }
    }
    #[test]
    fn p1_means_all_variants_and_only_qp_ms() {
        for name in [
            "9709_s23_qp_11.pdf",
            "9709_s23_qp_12.pdf",
            "9709_s23_qp_13.pdf",
            "9709_s23_ms_12.pdf",
        ] {
            assert!(matches_selection(name, &[1], true))
        }
        assert!(!matches_selection("9709_s23_qp_21.pdf", &[1], true));
        assert!(!matches_selection("9709_s23_ms_12.pdf", &[1], false));
        assert!(!matches_selection(
            "9709_s23_er.pdf",
            &[1, 2, 3, 4, 5, 6],
            true
        ));
    }
    #[tokio::test]
    async fn preview_freezes_destination_and_partial_query_reports() {
        let root = std::env::temp_dir().join(format!(
            "cpaper-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let root = tokio::fs::canonicalize(root).await.unwrap();
        let app = App {
            queue: Default::default(),
            client: reqwest::Client::new(),
            files: Default::default(),
            io: Default::default(),
            root: root.clone(),
        };
        let p = Paper {
            id: "9709_s23_qp_12.pdf".into(),
            filename: "9709_s23_qp_12.pdf".into(),
            kind: "qp".into(),
            source_url: "https://cie.fraft.cn/obj/Common/Fetch/redir/9709_s23_qp_12.pdf".into(),
        };
        let first = Settings {
            download_dir: root.join("first").to_string_lossy().into(),
            merge_folders: false,
        };
        let preview=freeze(&app,first.clone(),vec![p.clone()],vec![json!({"year":2023,"season":"Jun","status":"ok","file_count":1}),json!({"year":2024,"season":"Nov","status":"failed","file_count":0,"error":"upstream"})]).await.unwrap();
        assert!(!preview.complete);
        assert_eq!(preview.queries.len(), 2);
        assert!(preview.files[0]
            .destination
            .ends_with("/first/2023/QP/9709_s23_qp_12.pdf"));
        let second = Settings {
            download_dir: root.join("second").to_string_lossy().into(),
            merge_folders: true,
        };
        write_control(&app.root.join(".settings.json"), &second)
            .await
            .unwrap();
        let loaded: BatchPreview = read_control(&preview_path(&app, &preview.token).unwrap())
            .await
            .unwrap();
        assert_eq!(loaded.save_directory, first.download_dir);
        assert!(freeze_file(&p, &second)
            .unwrap()
            .destination
            .ends_with("/second/9709_s23_qp_12.pdf"));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
