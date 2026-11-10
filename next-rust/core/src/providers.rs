use super::*;
use std::future::Future;
pub(super) fn default_paper_source() -> String {
    "frankcie".into()
}
pub(super) const SOURCES: [&str; 4] = ["frankcie", "easypaper", "pastpapers", "papacambridge"];
pub(super) fn allowed_provider_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some(
                "cie.fraft.cn"
                    | "server.easy-paper.com"
                    | "pastpapers.co"
                    | "pastpapers.papacambridge.com"
            )
        )
}
pub(super) fn paper_id(source: &str, filename: &str) -> String {
    if source == "frankcie" {
        filename.into()
    } else {
        format!("{source}~{filename}")
    }
}
pub(super) fn validate_paper(p: &Paper) -> bool {
    if parse(&p.filename).is_none() || p.id != paper_id(&p.source, &p.filename) {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(&p.source_url) else {
        return false;
    };
    if !allowed_provider_url(&url) || url.query().is_some() || url.fragment().is_some() {
        return false;
    }
    match p.source.as_str() {
        "frankcie" => {
            p.source_url == format!("https://cie.fraft.cn/obj/Common/Fetch/redir/{}", p.filename)
        }
        "pastpapers" | "papacambridge" => crate::provider_html::validate_paper(p),
        "easypaper" => crate::provider_easypaper::validate_paper(p),
        _ => false,
    }
}
pub(super) fn download_url(p: &Paper) -> Result<String, ErrApi> {
    #[cfg(test)]
    if p.source_url.starts_with("http://127.0.0.1:") {
        return Ok(p.source_url.clone());
    }
    if !validate_paper(p) {
        return Err(bad("文件来源或逻辑路径未通过校验"));
    }
    if p.source == "easypaper" {
        return crate::provider_easypaper::resolve_download_url(
            p.logical_path
                .as_deref()
                .ok_or_else(|| bad("缺少EasyPaper逻辑路径"))?,
        )
        .map(|url| url.to_string());
    }
    Ok(p.source_url.clone())
}
pub(super) fn matching_paper(source: &str, name: &str, url: String, q: &Search) -> Option<Paper> {
    let (_, prefix) = query_parts(q).ok()?;
    let (subject, session, kind, _) = parse(name)?;
    if subject != q.subject || session != format!("{}{:02}", prefix, q.year % 100) {
        return None;
    }
    let p = Paper {
        id: paper_id(source, name),
        filename: name.into(),
        kind,
        source_url: url,
        source: source.into(),
        logical_path: if source == "papacambridge" {
            Some(format!("/directories/CAIE/CAIE-pastpapers/upload/{name}"))
        } else {
            None
        },
    };
    validate_paper(&p).then_some(p)
}
pub(super) async fn provider_text(response: reqwest::Response) -> Result<String, ErrApi> {
    let status = response.status();
    if !status.is_success() {
        return Err(upstream(format!(
            "题源HTTP {}；未绕过限制",
            status.as_u16()
        )));
    }
    if response.content_length().unwrap_or(0) > 4 * 1024 * 1024 {
        return Err(upstream("题源响应超过4MiB"));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = vec![];
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| upstream("题源响应读取失败"))?;
        if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
            return Err(upstream("题源响应超过4MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes).map_err(|_| upstream("题源响应不是UTF-8"))?;
    let lower = text.to_ascii_lowercase();
    if [
        "cf-chl-",
        "challenge-platform",
        "just a moment",
        "cf-turnstile",
        "verify you are human",
    ]
    .iter()
    .any(|s| lower.contains(s))
    {
        return Err(upstream("题源验证挑战，未绕过；请稍后手动重试"));
    }
    Ok(text)
}
async fn frankcie(app: &App, q: &Search) -> Result<Vec<Paper>, ErrApi> {
    let (season, _) = query_parts(q)?;
    let response = app
        .client
        .post("https://cie.fraft.cn/obj/Common/Fetch/renum")
        .form(&[
            ("subject", q.subject.clone()),
            ("year", q.year.to_string()),
            ("season", season.into()),
        ])
        .send()
        .await
        .map_err(|_| upstream("FrankCIE请求失败"))?;
    let v: Value = serde_json::from_str(&provider_text(response).await?)
        .map_err(|_| upstream("FrankCIE响应不是有效JSON"))?;
    if !v.get("rows").is_some_and(Value::is_array) {
        return Err(upstream("FrankCIE响应缺少rows列表"));
    }
    let mut names = vec![];
    collect(&v, &mut names);
    names.sort();
    names.dedup();
    Ok(names
        .into_iter()
        .filter_map(|name| {
            matching_paper(
                "frankcie",
                &name,
                format!("https://cie.fraft.cn/obj/Common/Fetch/redir/{name}"),
                q,
            )
        })
        .collect())
}
async fn fetch_source(app: &App, q: &Search, source: &str) -> Result<Vec<Paper>, ErrApi> {
    match source {
        "frankcie" => frankcie(app, q).await,
        "easypaper" => crate::provider_easypaper::search(&app.client, q).await,
        "pastpapers" => crate::provider_html::search_pastpapers(&app.client, q).await,
        "papacambridge" => crate::provider_html::search_papacambridge(&app.client, q).await,
        _ => Err(upstream(format!("{source}适配器尚未完成；未伪造结果"))),
    }
}
pub(super) async fn select_source<T, F, Fut>(
    mode: &str,
    fetch: F,
) -> Result<(String, Vec<T>, Vec<Value>), ErrApi>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Vec<T>, ErrApi>>,
{
    select_source_deadline(mode, fetch, std::time::Duration::from_secs(12)).await
}
async fn select_source_deadline<T, F, Fut>(
    mode: &str,
    mut fetch: F,
    deadline: std::time::Duration,
) -> Result<(String, Vec<T>, Vec<Value>), ErrApi>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Vec<T>, ErrApi>>,
{
    if mode != "auto" && !SOURCES.contains(&mode) {
        return Err(bad("来源选择无效"));
    }
    let sources: Vec<&str> = if mode == "auto" {
        SOURCES.to_vec()
    } else {
        vec![mode]
    };
    let mut attempts = vec![];
    for source in sources {
        let began = std::time::Instant::now();
        let result = if mode == "auto" {
            tokio::time::timeout(deadline, fetch(source.into()))
                .await
                .unwrap_or_else(|_| Err(upstream("题源自动回退查询超时")))
        } else {
            fetch(source.into()).await
        };
        let duration_ms = began.elapsed().as_millis() as u64;
        match result {
            Ok(files) if !files.is_empty() => {
                attempts.push(json!({"source":source,"status":"ok","duration_ms":duration_ms,"file_count":files.len()}));
                return Ok((source.into(), files, attempts));
            }
            Ok(_) => attempts.push(json!({"source":source,"status":"empty","duration_ms":duration_ms,"file_count":0})),
            Err((_, Json(error))) => {
                attempts.push(json!({"source":source,"status":"failed","duration_ms":duration_ms,"error":error["error"]}))
            }
        }
    }
    if attempts.iter().any(|a| a["status"] == "failed") {
        Err((
            StatusCode::BAD_GATEWAY,
            Json(
                json!({"error":"所选来源未返回可用结果；请查看逐源记录","source_mode":mode,"attempts":attempts}),
            ),
        ))
    } else {
        Ok((
            if mode == "auto" {
                "none".into()
            } else {
                mode.into()
            },
            vec![],
            attempts,
        ))
    }
}
pub(super) async fn search_with_mode(
    app: &App,
    q: &Search,
    mode: &str,
) -> Result<Json<Value>, ErrApi> {
    let (season, _) = query_parts(q)?;
    let (source, files, attempts) =
        select_source(
            mode,
            |source| async move { fetch_source(app, q, &source).await },
        )
        .await?;
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut registry = app.files.lock().await;
    for paper in files {
        let (subject, session, _, number) = parse(&paper.filename).unwrap();
        let id = format!("{subject}_{session}_{number}");
        groups
            .entry(id.clone())
            .or_insert(Group {
                id,
                number,
                files: vec![],
            })
            .files
            .push(paper.clone());
        registry.insert(paper.id.clone(), paper);
    }
    Ok(Json(
        json!({"source":source,"source_mode":mode,"attempts":attempts,"groups":groups.into_values().collect::<Vec<_>>(),"query":{"subject":q.subject,"year":q.year,"season":season},"basis":"source-listing; PDFs verified on download"}),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn fallback_order_stops_on_success_and_manual_never_falls_back() {
        let calls = Arc::new(std::sync::Mutex::new(vec![]));
        let seen = calls.clone();
        let (source, files, attempts) = select_source("auto", move |source| {
            seen.lock().unwrap().push(source.clone());
            async move {
                match source.as_str() {
                    "frankcie" => Err(upstream("503")),
                    "easypaper" => Ok(vec![]),
                    _ => Ok(vec![1]),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(source, "pastpapers");
        assert_eq!(files, vec![1]);
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["frankcie", "easypaper", "pastpapers"]
        );
        assert_eq!(attempts[0]["status"], "failed");
        assert_eq!(attempts[1]["status"], "empty");
        let (_, Json(e)) =
            select_source::<i32, _, _>("easypaper", |_| async { Err(upstream("403")) })
                .await
                .unwrap_err();
        assert_eq!(e["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(e["source_mode"], "easypaper");
    }
    #[tokio::test]
    async fn empty_success_and_failed_attempts_are_distinct() {
        let (_, files, attempts) = select_source::<i32, _, _>("auto", |_| async { Ok(vec![]) })
            .await
            .unwrap();
        assert!(files.is_empty());
        assert_eq!(attempts.len(), 4);
        let (_, Json(e)) =
            select_source::<i32, _, _>("auto", |_| async { Err(upstream("challenge")) })
                .await
                .unwrap_err();
        assert_eq!(e["attempts"].as_array().unwrap().len(), 4);
    }
    #[tokio::test]
    async fn auto_deadline_falls_back_but_manual_uses_bounded_adapter_timeout() {
        let (source, _, attempts) = select_source_deadline(
            "auto",
            |source| async move {
                if source == "frankcie" {
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }
                Ok(vec![1])
            },
            std::time::Duration::from_millis(3),
        )
        .await
        .unwrap();
        assert_eq!(source, "easypaper");
        assert_eq!(attempts[0]["status"], "failed");
        assert!(attempts[0]["duration_ms"].as_u64().unwrap() >= 3);
        let (source, _, attempts) = select_source_deadline(
            "frankcie",
            |_| async {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                Ok(vec![1])
            },
            std::time::Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert_eq!(source, "frankcie");
        assert_eq!(attempts.len(), 1);
    }
    #[test]
    fn provenance_is_source_bound_and_urls_reject_foreign_hosts() {
        let q = Search {
            subject: "9709".into(),
            year: 2023,
            season: "Jun".into(),
        };
        let p=matching_paper("papacambridge","9709_s23_qp_12.pdf","https://pastpapers.papacambridge.com/directories/CAIE/CAIE-pastpapers/upload/9709_s23_qp_12.pdf".into(),&q).unwrap();
        assert_eq!(p.id, "papacambridge~9709_s23_qp_12.pdf");
        assert!(validate_paper(&p));
        let mut wrong = p.clone();
        wrong.source = "frankcie".into();
        assert!(!validate_paper(&wrong));
        assert!(!allowed_provider_url(
            &"https://pastpapers.co.evil.test/caie/a.pdf"
                .parse()
                .unwrap()
        ));
        assert!(!allowed_provider_url(
            &"https://user@pastpapers.co/caie/a.pdf".parse().unwrap()
        ));
    }
}
