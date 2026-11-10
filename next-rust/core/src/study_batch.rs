use super::*;
#[derive(Deserialize)]
pub(super) struct StudyBatch {
    pub(super) port: u16,
    pub(super) confirmed: bool,
    pub(super) ids: Vec<String>,
}
#[derive(Clone, Serialize)]
pub(super) struct ExpectedImport {
    pub(super) id: String,
    pub(super) filename: String,
    pub(super) hash: String,
    pub(super) source_url: String,
}
pub(super) fn uncertain(message: &str, receipt_id: &str) -> ErrApi {
    (
        StatusCode::BAD_GATEWAY,
        Json(
            json!({"error":message,"outcome":"uncertain","receipt_id":receipt_id,"retry_allowed":false}),
        ),
    )
}
fn blocked_uncertain(message: &str, id: &str) -> ErrApi {
    let (_, v) = uncertain(message, id);
    (StatusCode::CONFLICT, v)
}
pub(super) fn validate_batch_receipt(
    value: &Value,
    expected: &[ExpectedImport],
) -> Result<(), String> {
    let records = value["imports"].as_array().ok_or("缺少imports")?;
    if records.len() != expected.len() {
        return Err("返回文件数不符".into());
    }
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    let mut good = HashMap::new();
    for r in records {
        let filename = r["filename"].as_str().ok_or("缺少文件名")?;
        let id = r["id"]
            .as_str()
            .filter(|s| valid_job_id(s))
            .ok_or("文件id无效")?;
        let e = expected
            .iter()
            .find(|e| e.filename == filename)
            .ok_or("未知文件")?;
        if r["hash"] != e.hash || !ids.insert(id) || !names.insert(filename) {
            return Err("哈希或唯一标识不符".into());
        }
        match r["status"].as_str() {
            Some("failed") => {}
            Some("awaiting_agent" | "published") => {
                good.insert(id, (e.hash.as_str(), e.filename.as_str()));
            }
            _ => return Err("未知预检状态".into()),
        }
    }
    if good.is_empty() {
        if !value["recognition_job"].is_null() {
            return Err("全部失败却返回识别任务".into());
        }
        return Ok(());
    }
    let job = &value["recognition_job"];
    if !job["id"].as_str().is_some_and(valid_job_id)
        || job["status"].as_str().unwrap_or("").is_empty()
    {
        return Err("识别任务缺失".into());
    }
    let sources = job["sources"].as_array().ok_or("任务缺少来源")?;
    if sources.len() != good.len() {
        return Err("任务来源数量不符".into());
    }
    let mut seen = std::collections::HashSet::new();
    for source in sources {
        let id = source["id"].as_str().ok_or("任务来源id缺失")?;
        if !seen.insert(id)
            || good.get(id).copied() != source["sha256"].as_str().zip(source["filename"].as_str())
        {
            return Err("任务来源错配".into());
        }
    }
    Ok(())
}
pub(super) fn valid_job_id(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}
fn receipt_name(s: &str) -> bool {
    (s.starts_with("batch-")
        && s.len() == 75
        && s.ends_with(".json")
        && s.as_bytes()[6..70].iter().all(|c| c.is_ascii_hexdigit()))
        || s.split_once(".study-").is_some_and(|(f, t)| {
            parse(f).is_some()
                && t.strip_suffix(".json")
                    .is_some_and(|x| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit()))
        })
}
async fn read_receipt(app: &App, id: &str) -> Result<Value, ErrApi> {
    if !receipt_name(id) {
        return Err(bad("回执标识无效"));
    }
    let path = app.root.join(id);
    let meta = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| err(StatusCode::NOT_FOUND, "回执不存在"))?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 4 * 1024 * 1024 {
        return Err(bad("回执不是受控普通文件"));
    }
    let mut receipt: Value =
        serde_json::from_slice(&tokio::fs::read(path).await.map_err(upstream)?)
            .map_err(upstream)?;
    if !receipt.is_object() {
        return Err(bad("回执JSON必须是对象"));
    }
    receipt["receipt_id"] = json!(id);
    Ok(receipt)
}
pub(super) async fn receipts(State(app): State<App>) -> Result<Json<Value>, ErrApi> {
    if !app.root.exists() {
        return Ok(Json(json!({"receipts":[]})));
    }
    let mut dir = tokio::fs::read_dir(&app.root).await.map_err(upstream)?;
    let mut all = vec![];
    while let Some(entry) = dir.next_entry().await.map_err(upstream)? {
        let id = entry.file_name().to_string_lossy().to_string();
        if receipt_name(&id) {
            all.push(read_receipt(&app, &id).await?);
        }
    }
    all.sort_by(|a, b| b["created_at"].as_u64().cmp(&a["created_at"].as_u64()));
    Ok(Json(json!({"receipts":all})))
}
async fn replace_receipt(path: &std::path::Path, value: &Value) -> Result<(), ErrApi> {
    let meta = tokio::fs::symlink_metadata(path).await.map_err(upstream)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(bad("拒绝替换非普通回执"));
    }
    let (tmp, mut f) = new_staging(path, "update").await?;
    use tokio::io::AsyncWriteExt;
    let result = async {
        f.write_all(&serde_json::to_vec_pretty(value).unwrap())
            .await?;
        f.sync_all().await?;
        tokio::fs::rename(&tmp, path).await
    }
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;
    result.map_err(upstream)
}
fn original_name(id: &str) -> &str {
    id.rsplit_once('~').map_or(id, |(_, name)| name)
}
// Call only while holding app.io, including the final check+create transaction.
async fn prior_operation(app: &App, port: u16, ids: &[String]) -> Result<Option<Value>, ErrApi> {
    if !app.root.exists() {
        return Ok(None);
    }
    let mut requested = ids.to_vec();
    requested.sort();
    let mut directory = tokio::fs::read_dir(&app.root).await.map_err(upstream)?;
    let mut replay = None;
    while let Some(entry) = directory.next_entry().await.map_err(upstream)? {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("batch-") || !receipt_name(&name) {
            continue;
        }
        let mut old = read_receipt(app, &name).await?;
        if old["port"] != port {
            continue;
        }
        let files = old["expected_files"]
            .as_array()
            .ok_or_else(|| blocked_uncertain("未决记录缺少原件映射；请核对该回执", &name))?;
        let mut stored = Vec::new();
        for item in files {
            let id = item["id"]
                .as_str()
                .ok_or_else(|| blocked_uncertain("记录缺少原件标识；请核对该回执", &name))?;
            stored.push(id.to_owned());
        }
        stored.sort();
        // A provider change cannot evade an unresolved submission for the same original.
        // Keep exact IDs for completed replays, but conservatively compare filenames for pending work.
        let overlap = stored.iter().any(|id| {
            requested
                .iter()
                .any(|r| original_name(id) == original_name(r))
        });
        if old["operation_state"] != "complete" && overlap {
            return Err(blocked_uncertain(
                "所选原件与该端口未决提交存在交集；请核对原回执，不重发",
                &name,
            ));
        }
        if old["operation_state"] == "complete" && stored == requested {
            old["replayed"] = json!(true);
            replay = Some(old);
        }
    }
    Ok(replay)
}
pub(super) async fn study_import_batch(
    State(app): State<App>,
    h: HeaderMap,
    Json(target): Json<StudyBatch>,
) -> Result<Json<Value>, ErrApi> {
    check_origin(&h)?;
    if !target.confirmed {
        return Err(bad("请确认将所选原件发送预检；不会直接发布题目"));
    }
    if target.ids.is_empty() || target.ids.len() > 20 {
        return Err(bad("一次预检选择1–20份文件"));
    }
    let mut unique = std::collections::HashSet::new();
    if target
        .ids
        .iter()
        .any(|id| !unique.insert(original_name(id)))
    {
        return Err(bad("同一批次不能重复选择文件"));
    }
    study_base(target.port)?;
    // Scan all pending intersections before returning any completed replay.
    {
        let _guard = app.io.lock().await;
        if let Some(old) = prior_operation(&app, target.port, &target.ids).await? {
            return Ok(Json(old));
        }
    }

    let mut expected = vec![];
    let mut payloads = vec![];
    let mut total = 0usize;
    for id in &target.ids {
        let p = lookup(&app, id).await?;
        let saved = save(&app, &p).await?;
        let bytes = tokio::fs::read(
            saved["path"]
                .as_str()
                .ok_or_else(|| bad("下载结果缺少路径"))?,
        )
        .await
        .map_err(upstream)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        if bytes.len() > 25 * 1024 * 1024 || saved["sha256"] != hash {
            return Err(err(StatusCode::CONFLICT, "发送前文件内容变化或超限"));
        }
        total += bytes.len();
        if total > 90 * 1024 * 1024 {
            return Err(bad("整批PDF合计不能超过90MiB"));
        }
        expected.push(ExpectedImport {
            id: id.clone(),
            filename: p.filename.clone(),
            hash,
            source_url: p.source_url,
        });
        payloads.push((p.filename, bytes));
    }
    expected.sort_by(|a, b| a.filename.cmp(&b.filename));
    let key = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!({"port":target.port,"files":expected})).unwrap())
    );
    let receipt_id = format!("batch-{key}.json");
    let path = app.root.join(&receipt_id);
    probe_study(target.port).await?;
    let mut form = reqwest::multipart::Form::new();
    for (name, bytes) in payloads {
        form = form.part(
            "file",
            reqwest::multipart::Part::bytes(bytes)
                .file_name(name)
                .mime_str("application/pdf")
                .map_err(upstream)?,
        );
    }
    let request = study_client()?
        .post(format!("{}/api/imports", study_base(target.port)?))
        .header("X-Study-Client", "cpaper/0.2")
        .multipart(form)
        .build()
        .map_err(upstream)?;
    let mut receipt = json!({"schema":"cpaper.study-receipt.v2","receipt_id":receipt_id,"receipt_path":path,"port":target.port,"expected_files":expected,"operation_state":"submitted","outcome":"uncertain","retry_allowed":false,"published":false,"created_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()});
    {
        let _guard = app.io.lock().await;
        // Atomic with creation: concurrent overlapping sets cannot both submit.
        if let Some(old) = prior_operation(&app, target.port, &target.ids).await? {
            return Ok(Json(old));
        }
        atomic_new(&path, &serde_json::to_vec_pretty(&receipt).unwrap()).await?;
    }
    // From this point, all uncertain outcomes keep the journal and never resend automatically.
    let result = async {
        let response = study_client()?
            .execute(request)
            .await
            .map_err(|_| uncertain("提交结果不确定，请在对侧核对", &receipt_id))?;
        let result = capped_json(response, 4 * 1024 * 1024).await.map_err(|_| {
            uncertain(
                "回执响应不确定，可能部分文件已保存，请在对侧核对",
                &receipt_id,
            )
        })?;
        validate_batch_receipt(&result, &expected)
            .map_err(|e| uncertain(&format!("回执校验失败：{e}；请在对侧核对"), &receipt_id))?;
        Ok::<Value, ErrApi>(result)
    }
    .await;
    match result {
        Err(e) => {
            receipt["operation_state"] = json!("uncertain");
            receipt["error"] = e.1 .0["error"].clone();
            let _guard = app.io.lock().await;
            let _ = replace_receipt(&path, &receipt).await;
            Err(e)
        }
        Ok(result) => {
            let imports = result["imports"].as_array().unwrap();
            receipt["imports"]=json!(imports.iter().map(|r|json!({"id":r["id"],"filename":r["filename"],"hash":r["hash"],"status":r["status"],"message":r["message"],"duplicate":r.get("duplicate").cloned().unwrap_or(json!(false))})).collect::<Vec<_>>());
            receipt["recognition_job"] = if result["recognition_job"].is_null() {
                Value::Null
            } else {
                json!({"id":result["recognition_job"]["id"],"status":result["recognition_job"]["status"]})
            };
            receipt["duplicate_count"] =
                json!(imports.iter().filter(|r| r["duplicate"] == true).count());
            receipt["precheck_failed"] =
                json!(imports.iter().filter(|r| r["status"] == "failed").count());
            receipt["operation_state"] = json!("complete");
            receipt["outcome"] = json!("confirmed");
            receipt["sent_for_precheck"] = json!(true);
            let _guard = app.io.lock().await;
            replace_receipt(&path, &receipt).await.map_err(|_| {
                uncertain(
                    "对侧已返回结果，但本地回执保存失败；请核对对侧，不重发",
                    &receipt_id,
                )
            })?;
            Ok(Json(receipt))
        }
    }
}
#[derive(Deserialize)]
pub(super) struct ReceiptQuery {
    port: u16,
}
pub(super) async fn receipt_status(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(target): Query<ReceiptQuery>,
) -> Result<Json<Value>, ErrApi> {
    let receipt = read_receipt(&app, &id).await?;
    let job_id = receipt["recognition_job"]["id"]
        .as_str()
        .filter(|id| valid_job_id(id))
        .ok_or_else(|| {
            err(
                StatusCode::CONFLICT,
                "回执没有已确认任务标识；请在学习软件中核对原件",
            )
        })?;
    probe_study(target.port).await?;
    let response = study_client()?
        .get(format!(
            "{}/api/recognition/jobs/{job_id}",
            study_base(target.port)?
        ))
        .header("X-Study-Client", "cpaper/0.2")
        .send()
        .await
        .map_err(upstream)?;
    let job = capped_json(response, 4 * 1024 * 1024).await?;
    if job["status"].as_str().unwrap_or("").is_empty() {
        return Err(upstream("任务状态缺失，状态未知"));
    }
    if job["id"] != job_id {
        return Err(upstream("任务响应标识不匹配"));
    }
    let imports = receipt["imports"]
        .as_array()
        .ok_or_else(|| upstream("本地回执缺少文件映射，状态未知"))?;
    let sources = job["sources"]
        .as_array()
        .ok_or_else(|| upstream("任务缺少来源，状态未知"))?;
    let expected: Vec<_> = imports.iter().filter(|v| v["status"] != "failed").collect();
    if sources.len() != expected.len() {
        return Err(upstream("任务来源数量不符，状态未知"));
    }
    let mut seen = std::collections::HashSet::new();
    for source in sources {
        let sid = source["id"]
            .as_str()
            .ok_or_else(|| upstream("任务来源id缺失，状态未知"))?;
        let item = expected
            .iter()
            .find(|r| r["id"] == sid)
            .ok_or_else(|| upstream("任务含未知原件，状态未知"))?;
        let hash = item.get("hash").unwrap_or(&receipt["sha256"]);
        if !seen.insert(sid) || source["sha256"] != *hash || source["filename"] != item["filename"]
        {
            return Err(upstream("任务原件校验不符，状态未知"));
        }
    }
    Ok(Json(
        json!({"verified":true,"port":target.port,"receipt_id":id,"original_port":receipt["port"],"recognition_job":job}),
    ))
}
