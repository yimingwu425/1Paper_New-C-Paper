mod provider_easypaper;
mod provider_html;
mod providers;
use providers::*;
mod legacy_data;
use legacy_data::*;
mod download_jobs;
use download_jobs::*;
mod preferences;
use preferences::*;
mod batch_preview;
use batch_preview::*;
mod catalog;
use catalog::*;
mod study_batch;
use study_batch::*;
mod storage;
mod study;
use axum::{
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
};
use storage::*;
use study::*;
use tokio::sync::Mutex;
type ErrApi = (StatusCode, Json<Value>);
fn err(status: StatusCode, s: impl ToString) -> ErrApi {
    (status, Json(json!({"error":s.to_string()})))
}
fn bad(s: impl ToString) -> ErrApi {
    err(StatusCode::BAD_REQUEST, s)
}
fn upstream(s: impl ToString) -> ErrApi {
    err(StatusCode::BAD_GATEWAY, s)
}
#[derive(Clone)]
struct App {
    client: reqwest::Client,
    files: Arc<Mutex<HashMap<String, Paper>>>,
    io: Arc<Mutex<()>>,
    root: PathBuf,
    queue: Arc<Mutex<QueueRuntime>>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
struct Paper {
    id: String,
    filename: String,
    kind: String,
    source_url: String,
    #[serde(default = "default_paper_source")]
    source: String,
    #[serde(default)]
    logical_path: Option<String>,
}
#[derive(Serialize)]
struct Group {
    id: String,
    number: String,
    files: Vec<Paper>,
}
#[derive(Deserialize)]
struct Search {
    subject: String,
    year: u16,
    season: String,
}
fn parse(name: &str) -> Option<(String, String, String, String)> {
    let re = regex::Regex::new(r"^(\d{4})_([mws]\d{2})_(qp|ms|ci|gt|er|ir|in|sr)(?:_(\d+))?\.pdf$")
        .unwrap();
    let c = re.captures(name)?;
    Some((
        c[1].into(),
        c[2].into(),
        c[3].into(),
        c.get(4).map_or("", |m| m.as_str()).into(),
    ))
}
fn collect(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if parse(s).is_some() {
                out.push(s.clone())
            }
        }
        Value::Array(a) => {
            for x in a {
                collect(x, out)
            }
        }
        Value::Object(o) => {
            for x in o.values() {
                collect(x, out)
            }
        }
        _ => {}
    }
}
fn allowed(url: &reqwest::Url) -> bool {
    allowed_provider_url(url)
}
fn query_parts(q: &Search) -> Result<(&'static str, char), ErrApi> {
    if q.subject.len() != 4
        || !q.subject.bytes().all(|b| b.is_ascii_digit())
        || !(2000..=2099).contains(&q.year)
    {
        return Err(bad("科目须为四位代码，年份须在 2000–2099"));
    }
    match q.season.as_str() {
        "s" | "Jun" => Ok(("Jun", 's')),
        "m" | "Mar" => Ok(("Mar", 'm')),
        "w" | "Nov" => Ok(("Nov", 'w')),
        _ => Err(bad("考季须为 Mar/Jun/Nov")),
    }
}
async fn health() -> Json<Value> {
    Json(
        json!({"status":"ok","version":"0.1.0","sources":["frankcie","easypaper","pastpapers","papacambridge"],"study_connection":"manual-port-required"}),
    )
}
async fn search(State(app): State<App>, Query(q): Query<Search>) -> Result<Json<Value>, ErrApi> {
    let mode = read_settings(&app).await?.source_mode;
    search_with_mode(&app, &q, &mode).await
}
async fn lookup(app: &App, id: &str) -> Result<Paper, ErrApi> {
    if parse(id.rsplit_once('~').map_or(id, |(_, name)| name)).is_none() {
        return Err(bad("无效文件标识"));
    }
    app.files
        .lock()
        .await
        .get(id)
        .cloned()
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "请先搜索此文件"))
}
async fn pdf(app: &App, p: &Paper) -> Result<Vec<u8>, ErrApi> {
    let response = app
        .client
        .get(download_url(p)?)
        .send()
        .await
        .map_err(|_| upstream("PDF请求失败"))?
        .error_for_status()
        .map_err(|e| {
            upstream(format!(
                "PDF HTTP错误：{}",
                e.status().map(|s| s.as_u16()).unwrap_or(0)
            ))
        })?;
    if response.content_length().unwrap_or(0) > 25 * 1024 * 1024 {
        return Err(upstream("PDF 超过 25 MiB"));
    }
    let mut chunks = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| upstream("PDF流读取失败"))?;
        if bytes.len() + chunk.len() > 25 * 1024 * 1024 {
            return Err(upstream("PDF 超过 25 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !bytes.starts_with(b"%PDF-") || !bytes.windows(5).rev().take(2048).any(|s| s == b"%%EOF") {
        return Err(upstream("上游响应不是完整 PDF"));
    }
    Ok(bytes)
}
async fn preview(
    State(app): State<App>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ErrApi> {
    let p = lookup(&app, &id).await?;
    let b = pdf(&app, &p).await?;
    Ok((
        [
            ("Content-Type", "application/pdf"),
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
        ],
        b,
    ))
}
async fn local_only(request: Request, next: Next) -> axum::response::Response {
    let host = request
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !matches!(
        host,
        "127.0.0.1:47831" | "localhost:47831" | "127.0.0.1:47832" | "localhost:47832"
    ) {
        return err(StatusCode::FORBIDDEN, "Host 不在本地允许列表").into_response();
    }
    if request.uri().path().starts_with("/api/")
        && request
            .headers()
            .get("sec-fetch-site")
            .and_then(|h| h.to_str().ok())
            == Some("cross-site")
    {
        return err(StatusCode::FORBIDDEN, "拒绝跨站请求").into_response();
    }
    if request.uri().path().starts_with("/api/") {
        if let Err(e) = check_origin(request.headers()) {
            return e.into_response();
        }
    }
    next.run(request).await
}
fn check_origin(h: &HeaderMap) -> Result<(), ErrApi> {
    if let Some(o) = h.get("origin") {
        let o = o.to_str().map_err(bad)?;
        if !matches!(
            o,
            "http://127.0.0.1:47831"
                | "http://localhost:47831"
                | "http://127.0.0.1:47832"
                | "http://localhost:47832"
                | "tauri://localhost"
                | "http://tauri.localhost"
        ) {
            return Err(err(StatusCode::FORBIDDEN, "不接受其他来源的写入"));
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() {
    let root = std::env::var_os("CPAPER_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../data/downloads"));
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .timeout(std::time::Duration::from_secs(45))
        .user_agent("C-Paper/0.1 local-migration")
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 || !allowed(attempt.url()) {
                attempt.error("题源重定向目标不在允许列表")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .unwrap();
    let app = App {
        client,
        files: Default::default(),
        io: Default::default(),
        root,
        queue: Default::default(),
    };
    restore_queue(&app)
        .await
        .expect("恢复本地队列失败；保留原记录");
    let web = std::env::var_os("CPAPER_WEB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web/dist"));
    let router = Router::new()
        .route("/api/health", get(health))
        .route("/api/legacy/preview", get(legacy_preview))
        .route("/api/legacy/import", post(legacy_import))
        .route("/api/legacy/history", get(legacy_history))
        .route("/api/jobs", get(jobs_get).post(jobs_start))
        .route("/api/jobs/{id}/cancel", post(jobs_cancel))
        .route("/api/jobs/{id}/retry", post(jobs_retry))
        .route("/api/settings", get(settings_get).put(settings_put))
        .route("/api/preferences/choose-directory", post(choose_directory))
        .route("/api/preferences/open-directory", post(open_directory))
        .route("/api/favorites", get(favorites_get).post(favorites_add))
        .route(
            "/api/favorites/{code}",
            axum::routing::delete(favorites_delete),
        )
        .route("/api/batch/preview", post(batch_preview))
        .route("/api/batch/selection", post(selection_preview))
        .route("/api/catalog/subjects", get(subjects))
        .route("/api/catalog/availability", get(availability))
        .route("/api/downloads", get(downloads))
        .route("/api/study/status", get(study_status))
        .route("/api/study/import-batch", post(study_import_batch))
        .route("/api/study/receipts", get(receipts))
        .route("/api/study/receipts/{id}/status", get(receipt_status))
        .route("/api/study/import/{id}", post(study_import))
        .route("/api/download-batch", post(batch))
        .route("/api/search", get(search))
        .route("/api/preview/{id}", get(preview))
        .route("/api/download/{id}", post(download))
        .route("/api/handoff/{id}", post(handoff))
        .with_state(app)
        .fallback_service(tower_http::services::ServeDir::new(web))
        .layer(middleware::from_fn(local_only));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:47831")
        .await
        .expect("端口 47831 已占用；未停止其他进程");
    println!("C-Paper core http://127.0.0.1:47831");
    axum::serve(listener, router).await.unwrap();
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod study_batch_tests;
