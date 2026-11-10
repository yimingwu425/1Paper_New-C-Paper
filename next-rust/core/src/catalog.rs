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
pub(super) async fn subjects(State(app): State<App>) -> Result<Json<Value>, ErrApi> {
    let response = subject_request(&app.client)
        .send()
        .await
        .map_err(upstream)?;
    let raw = capped_json(response, 1024 * 1024).await?;
    let subjects = parse_subjects(&raw);
    if subjects.is_empty() {
        return Err(upstream("题源未返回有效科目目录；仍可手动输入四位代码"));
    }
    Ok(Json(json!({"source":"frankcie","subjects":subjects})))
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
    let mut seasons = vec![];
    let mut latest: Option<&str> = None;
    let mut complete = true;
    for (season, prefix) in [("Mar", 'm'), ("Jun", 's'), ("Nov", 'w')] {
        let result = async {
            let response = app
                .client
                .post("https://cie.fraft.cn/obj/Common/Fetch/renum")
                .form(&[
                    ("subject", q.subject.clone()),
                    ("year", q.year.to_string()),
                    ("season", season.into()),
                ])
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(upstream)?;
            let v = capped_json(response, 4 * 1024 * 1024).await?;
            if !v.get("rows").is_some_and(Value::is_array) {
                return Err(upstream("目录响应缺少rows"));
            }
            let sy = format!("{}{:02}", prefix, q.year % 100);
            let mut names = vec![];
            collect(&v, &mut names);
            names.sort();
            names.dedup();
            Ok::<usize, ErrApi>(
                names
                    .iter()
                    .filter(|name| {
                        parse(name).is_some_and(|(subject, session, _, _)| {
                            subject == q.subject && session == sy
                        })
                    })
                    .count(),
            )
        }
        .await;
        match result {
            Ok(count) => {
                if count > 0 {
                    latest = Some(season)
                }
                seasons.push(json!({"season":season,"status":if count>0{"available"}else{"empty"},"file_count":count}));
            }
            Err((_, Json(e))) => {
                complete = false;
                seasons.push(json!({"season":season,"status":"unknown","file_count":null,"error":e["error"]}));
            }
        }
    }
    Ok(Json(
        json!({"source":"frankcie","subject":q.subject,"year":q.year,"complete":complete,"seasons":seasons,"latest_available_season":latest,"coverage":"requested-year-only","basis":"source-listing; individual PDFs not verified"}),
    ))
}
