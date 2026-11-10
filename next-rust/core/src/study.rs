use super::*;
#[derive(Deserialize)]
pub(super) struct StudyTarget {
    pub(super) port: u16,
}
#[derive(Deserialize)]
pub(super) struct StudyConfirm {
    pub(super) port: u16,
    pub(super) confirmed: bool,
}
pub(super) fn study_base(port: u16) -> Result<String, ErrApi> {
    if port < 1024 || matches!(port, 47831 | 47832) {
        return Err(bad(
            "请填写学习软件明确提供的本地端口（1024以上且不是C-Paper端口）",
        ));
    }
    Ok(format!("http://127.0.0.1:{port}"))
}
pub(super) fn study_client() -> Result<reqwest::Client, ErrApi> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(upstream)
}
pub(super) async fn capped_json(
    response: reqwest::Response,
    limit: usize,
) -> Result<Value, ErrApi> {
    let response = response.error_for_status().map_err(upstream)?;
    let mut stream = response.bytes_stream();
    let mut bytes = vec![];
    while let Some(c) = stream.next().await {
        let c = c.map_err(upstream)?;
        if bytes.len() + c.len() > limit {
            return Err(upstream("本地学习服务响应超限"));
        }
        bytes.extend_from_slice(&c)
    }
    serde_json::from_slice(&bytes).map_err(upstream)
}
pub(super) async fn probe_study(port: u16) -> Result<Value, ErrApi> {
    let base = study_base(port)?;
    let response = study_client()?
        .get(format!("{base}/api/health"))
        .header("X-Study-Client", "cpaper/0.1")
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map_err(upstream)?;
    let value = capped_json(response, 16384).await?;
    if value["status"] != "ok"
        || value["core"] != "rust"
        || value["mode"] != "local"
        || !value["pdf_engine"]
            .as_str()
            .unwrap_or("")
            .starts_with("PdfCraft ")
    {
        return Err(err(StatusCode::CONFLICT, "此端口不是已识别的本地学习服务"));
    }
    Ok(value)
}
pub(super) async fn study_status(Query(target): Query<StudyTarget>) -> Result<Json<Value>, ErrApi> {
    let service = probe_study(target.port).await?;
    Ok(Json(
        json!({"verified":true,"port":target.port,"service":service}),
    ))
}
#[cfg(test)]
pub(super) fn validate_receipt(value: &Value, filename: &str, hash: &str) -> Result<(), ErrApi> {
    let records = value["imports"]
        .as_array()
        .ok_or_else(|| upstream("预检结果不确定：响应缺少imports，请在对侧核对"))?;
    if records.len() != 1
        || records[0]["hash"] != hash
        || records[0]["filename"] != filename
        || records[0]["id"].as_str().unwrap_or("").is_empty()
        || !matches!(
            records[0]["status"].as_str(),
            Some("awaiting_agent" | "failed" | "published")
        )
    {
        return Err(upstream(
            "预检结果不确定：文件标识、哈希或状态不匹配，请在对侧核对，不自动重试",
        ));
    }
    Ok(())
}
pub(super) async fn study_import(
    State(app): State<App>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(target): Json<StudyConfirm>,
) -> Result<Json<Value>, ErrApi> {
    study_import_batch(
        State(app),
        h,
        Json(StudyBatch {
            port: target.port,
            confirmed: target.confirmed,
            ids: vec![id],
        }),
    )
    .await
}
