use super::*;
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(super) struct Subject {
    pub(super) code: String,
    pub(super) name: String,
}
pub(super) fn parse_subjects(value: &Value) -> Vec<Subject> {
    fn visit(v: &Value, out: &mut BTreeMap<String, String>) {
        match v {
            Value::Object(o) => {
                let code = ["value", "code", "id"]
                    .into_iter()
                    .find_map(|key| o.get(key).and_then(Value::as_str));
                let name = ["text", "name", "title"]
                    .into_iter()
                    .find_map(|key| o.get(key).and_then(Value::as_str));
                if let (Some(code), Some(name)) = (code, name) {
                    if code.len() == 4
                        && code.bytes().all(|b| b.is_ascii_digit())
                        && !name.trim().is_empty()
                    {
                        out.entry(code.into()).or_insert(name.into());
                    }
                }
                for v in o.values() {
                    visit(v, out)
                }
            }
            Value::Array(a) => {
                for v in a {
                    visit(v, out)
                }
            }
            _ => {}
        }
    }
    let mut found = BTreeMap::new();
    visit(value, &mut found);
    found
        .into_iter()
        .map(|(code, name)| Subject { code, name })
        .collect()
}
pub(super) fn subject_request(client: &reqwest::Client) -> reqwest::RequestBuilder {
    client
        .post("https://cie.fraft.cn/obj/Common/Subject/combo")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Content-Length", "0")
        .body("")
}
async fn catalog_source(app: &App, source: &str) -> Result<Vec<Subject>, ErrApi> {
    match source {
        "easypaper" => return crate::provider_easypaper::subjects(&app.client).await,
        "pastpapers" => return crate::provider_html::subjects(&app.client).await,
        _ => {}
    }
    if source != "frankcie" {
        return Err(upstream(format!(
            "{source}科目目录尚不可用；可手动输入四位代码搜索"
        )));
    }
    let response = subject_request(&app.client)
        .send()
        .await
        .map_err(|_| upstream("科目目录请求失败"))?;
    let raw: Value = serde_json::from_str(&provider_text(response).await?)
        .map_err(|_| upstream("科目目录JSON无效"))?;
    Ok(parse_subjects(&raw))
}
pub(super) async fn subjects(State(app): State<App>) -> Result<Json<Value>, ErrApi> {
    let mode = read_settings(&app).await?.source_mode;
    let (source, subjects, attempts) = select_source(&mode, |source| {
        let app = &app;
        async move { catalog_source(app, &source).await }
    })
    .await?;
    Ok(Json(
        json!({"source":source,"source_mode":mode,"attempts":attempts,"subjects":subjects,"basis":"source-listing"}),
    ))
}
#[derive(Deserialize)]
pub(super) struct Availability {
    subject: String,
    year: u16,
}
pub(super) async fn availability(
    State(app): State<App>,
    Query(q): Query<Availability>,
) -> Result<Json<Value>, ErrApi> {
    query_parts(&Search {
        subject: q.subject.clone(),
        year: q.year,
        season: "Jun".into(),
    })?;
    let mode = read_settings(&app).await?.source_mode;
    let mut seasons = vec![];
    let mut latest = None;
    let mut complete = true;
    for season in ["Mar", "Jun", "Nov"] {
        match search_with_mode(
            &app,
            &Search {
                subject: q.subject.clone(),
                year: q.year,
                season: season.into(),
            },
            &mode,
        )
        .await
        {
            Ok(Json(v)) => {
                let count: usize = v["groups"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|g| g["files"].as_array().map_or(0, Vec::len))
                    .sum();
                if count > 0 {
                    latest = Some(season);
                }
                seasons.push(json!({"season":season,"status":if count>0{"available"}else{"empty"},"file_count":count,"source":v["source"],"attempts":v["attempts"]}));
            }
            Err((_, Json(e))) => {
                complete = false;
                seasons.push(json!({"season":season,"status":"unknown","file_count":null,"error":e["error"],"attempts":e["attempts"]}));
            }
        }
    }
    Ok(Json(
        json!({"source":mode,"source_mode":mode,"subject":q.subject,"year":q.year,"complete":complete,"seasons":seasons,"latest_available_season":latest,"coverage":"requested-year-only","basis":"source-listing; individual PDFs not verified"}),
    ))
}
