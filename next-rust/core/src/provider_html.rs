//! Bounded listing adapters. Listing results are candidates; the download layer verifies PDF bytes.
use crate::{bad, parse, query_parts, upstream, ErrApi, Paper, Search};
use futures_util::StreamExt;
use serde::Deserialize;
use std::collections::BTreeSet;

pub(crate) async fn body(client: &reqwest::Client, url: reqwest::Url) -> Result<Vec<u8>, ErrApi> {
    let mut response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| upstream("来源请求失败"))?
        .error_for_status()
        .map_err(|e| {
            upstream(format!(
                "来源返回 HTTP {}；未绕过限制",
                e.status().map_or(0, |s| s.as_u16())
            ))
        })?
        .bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.next().await {
        let chunk = chunk.map_err(|_| upstream("来源响应中断"))?;
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(upstream("来源目录超过 2 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn challenge(html: &str) -> bool {
    let s = html.to_ascii_lowercase();
    [
        "just a moment",
        "cf-chl-",
        "cf-turnstile",
        "challenge-platform",
        "verify you are human",
    ]
    .iter()
    .any(|v| s.contains(v))
}

pub(crate) fn safe_segments(path: &str, separator: char) -> bool {
    !path.is_empty()
        && path.len() < 2048
        && path.split(separator).all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && !s.chars().any(|c| {
                    c.is_control()
                        || matches!(c, '/' | '\\' | '%' | '?' | '#' | ':') && c != separator
                })
        })
}

pub(crate) fn candidate(
    q: &Search,
    source: &str,
    filename: &str,
    path: String,
    url: String,
) -> Option<Paper> {
    let (subject, session, kind, _) = parse(filename)?;
    let (_, prefix) = query_parts(q).ok()?;
    if subject != q.subject || session != format!("{}{:02}", prefix, q.year % 100) {
        return None;
    }
    Some(Paper {
        id: format!("{source}~{filename}"),
        filename: filename.into(),
        kind,
        source_url: url,
        source: source.into(),
        logical_path: Some(path),
    })
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
struct Entry {
    name: String,
    #[serde(rename = "relPath")]
    path: String,
    #[serde(rename = "isDir")]
    is_dir: bool,
}

fn entries(html: &str) -> Vec<Entry> {
    let objects = regex::Regex::new(r"\{[^{}]*\}").unwrap();
    let normalized = html
        .replace("\\\"", "\"")
        .replace("\\/", "/")
        .replace("\\u002F", "/")
        .replace("\\u0026", "&");
    let mut found = BTreeSet::new();
    for text in [html, normalized.as_str()] {
        for m in objects.find_iter(text) {
            if let Ok(e) = serde_json::from_str::<Entry>(m.as_str()) {
                if safe_segments(&e.path, '/') {
                    found.insert(e);
                }
            }
        }
    }
    found.into_iter().collect()
}

pub(crate) fn subject_name(name: &str) -> Option<crate::Subject> {
    let re = regex::Regex::new(r"^(.+?)(?:\s*\((\d{4})\)|-(\d{4}))$").unwrap();
    let c = re.captures(name)?;
    Some(crate::Subject {
        code: c.get(2).or_else(|| c.get(3))?.as_str().into(),
        name: c[1].trim().replace('-', " "),
    })
}

pub(crate) async fn subjects(client: &reqwest::Client) -> Result<Vec<crate::Subject>, ErrApi> {
    let mut subjects = std::collections::BTreeMap::new();
    for level in ["a-level", "igcse", "o-level"] {
        let page = html(client, format!("https://pastpapers.co/caie/{level}")).await?;
        for e in entries(&page)
            .into_iter()
            .filter(|e| e.is_dir && e.path.split('/').count() == 2)
        {
            if let Some(s) = subject_name(&e.name)
                .or_else(|| subject_name(e.path.rsplit('/').next().unwrap_or("")))
            {
                subjects.insert(s.code.clone(), s);
            }
        }
    }
    if subjects.is_empty() {
        return Err(upstream("PastPapers 未返回可解析科目目录；可手动输入 9709"));
    }
    Ok(subjects.into_values().collect())
}

async fn html(client: &reqwest::Client, url: String) -> Result<String, ErrApi> {
    let bytes = body(
        client,
        reqwest::Url::parse(&url).map_err(|_| bad("来源地址无效"))?,
    )
    .await?;
    let s = String::from_utf8(bytes).map_err(|_| upstream("目录不是有效 UTF-8"))?;
    if challenge(&s) {
        return Err(upstream("来源被 Cloudflare/访问验证拦截，未绕过验证"));
    }
    Ok(s)
}

fn pastpapers_candidates(html: &str, q: &Search, subject_path: &str) -> Vec<Paper> {
    entries(html)
        .into_iter()
        .filter(|e| {
            !e.is_dir
                && e.path.starts_with(&format!("{subject_path}/"))
                && e.path.rsplit('/').next() == Some(e.name.as_str())
        })
        .filter_map(|e| {
            candidate(
                q,
                "pastpapers",
                &e.name,
                e.path.clone(),
                format!("https://pastpapers.co/caie/{}", e.path),
            )
        })
        .filter(validate_paper)
        .collect()
}

pub(crate) async fn search_pastpapers(
    client: &reqwest::Client,
    q: &Search,
) -> Result<Vec<Paper>, ErrApi> {
    let (_, prefix) = query_parts(q)?;
    let mut subject = if q.subject == "9709" {
        Some(("a-level", "A-Level/Mathematics-9709".to_owned()))
    } else {
        None
    };
    if subject.is_none() {
        for level in ["a-level", "igcse", "o-level"] {
            let page = html(client, format!("https://pastpapers.co/caie/{level}")).await?;
            if let Some(e) = entries(&page).into_iter().find(|e| {
                e.is_dir
                    && e.path.split('/').count() == 2
                    && (e.path.ends_with(&format!("-{}", q.subject))
                        || e.path.ends_with(&format!("({})", q.subject)))
            }) {
                subject = Some((level, e.path));
                break;
            }
        }
    }
    let (level, path) = subject.ok_or_else(|| upstream("PastPapers 没有该科目的目录映射"))?;
    let season = match prefix {
        'm' => "march",
        's' => "may-june",
        _ => "oct-nov",
    };
    let slug = path.rsplit('/').next().unwrap().to_ascii_lowercase();
    let page = html(
        client,
        format!(
            "https://pastpapers.co/caie/{level}/{slug}/{}-{season}",
            q.year
        ),
    )
    .await?;
    let papers = pastpapers_candidates(&page, q, &path);
    if papers.is_empty() {
        return Err(upstream(
            "PastPapers 目录未返回匹配 PDF；未进行批量猜测探测",
        ));
    }
    Ok(papers)
}

fn papa_candidates(html: &str, q: &Search) -> Vec<Paper> {
    let re =
        regex::Regex::new(r"(?i)\b\d{4}_[mws]\d{2}_(?:qp|ms|ci|gt|er|ir|in|sr)(?:_\d+)?\.pdf\b")
            .unwrap();
    let names: BTreeSet<_> = re
        .find_iter(html)
        .map(|m| m.as_str().to_ascii_lowercase())
        .collect();
    names
        .into_iter()
        .filter_map(|name| {
            let path = format!("/directories/CAIE/CAIE-pastpapers/upload/{name}");
            candidate(
                q,
                "papacambridge",
                &name,
                path.clone(),
                format!("https://pastpapers.papacambridge.com{path}"),
            )
        })
        .collect()
}

pub(crate) async fn search_papacambridge(
    client: &reqwest::Client,
    q: &Search,
) -> Result<Vec<Paper>, ErrApi> {
    let (_, prefix) = query_parts(q)?;
    if q.subject != "9709" {
        return Err(upstream("PapaCambridge 暂只支持已确认目录的 9709 科目"));
    }
    let seasons: &[&str] = match prefix {
        'm' => &["march", "feb-march"],
        's' => &["may-june"],
        _ => &["oct-nov", "october-november"],
    };
    for season in seasons {
        let page = html(client, format!("https://pastpapers.papacambridge.com/papers/caie/as-and-a-level-mathematics-9709-{}-{season}", q.year)).await?;
        let papers = papa_candidates(&page, q);
        if !papers.is_empty() {
            return Ok(papers);
        }
    }
    Err(upstream(
        "PapaCambridge 目录没有匹配 PDF；下载时将验证实际 PDF 内容",
    ))
}

pub(crate) fn validate_paper(p: &Paper) -> bool {
    let Some(path) = &p.logical_path else {
        return false;
    };
    if parse(&p.filename).is_none() {
        return false;
    }
    match p.source.as_str() {
        "pastpapers" => {
            safe_segments(path, '/')
                && path.split('/').count() == 4
                && path.rsplit('/').next() == Some(p.filename.as_str())
                && p.source_url == format!("https://pastpapers.co/caie/{path}")
        }
        "papacambridge" => {
            path == &format!("/directories/CAIE/CAIE-pastpapers/upload/{}", p.filename)
                && p.source_url == format!("https://pastpapers.papacambridge.com{path}")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn q() -> Search {
        Search {
            subject: "9709".into(),
            year: 2023,
            season: "Jun".into(),
        }
    }
    #[tokio::test]
    async fn bounded_transport_and_http_errors_do_not_leak_urls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn fixture(status: &str, content: Vec<u8>) -> reqwest::Url {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                content.len()
            );
            tokio::spawn(async move {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let _ = s.read(&mut request).await;
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&content).await;
            });
            format!("http://{addr}/not-to-log-token").parse().unwrap()
        }
        let client = reqwest::Client::new();
        assert_eq!(
            body(&client, fixture("200 OK", b"listing".to_vec()).await)
                .await
                .unwrap(),
            b"listing"
        );
        let (_, axum::Json(error)) = body(&client, fixture("403 Forbidden", vec![]).await)
            .await
            .unwrap_err();
        let message = error["error"].as_str().unwrap();
        assert!(message.contains("403"));
        assert!(!message.contains("not-to-log-token"));
        assert!(body(
            &client,
            fixture("200 OK", vec![b'x'; 2 * 1024 * 1024 + 1]).await
        )
        .await
        .is_err());
    }
    #[test]
    fn parses_subject_names_without_inventing_codes() {
        assert_eq!(subject_name("Mathematics (9709)").unwrap().code, "9709");
        assert_eq!(subject_name("Chemistry-0620").unwrap().code, "0620");
        assert!(subject_name("Chemistry").is_none());
    }
    #[test]
    fn rsc_order_case_and_additional_documents() {
        let page = r#"<script>{\"isDir\":false,\"relPath\":\"A-Level\/Mathematics-9709\/2023-May-June\/9709_s23_gt.pdf\",\"name\":\"9709_s23_gt.pdf\"}</script>"#;
        let p = pastpapers_candidates(page, &q(), "A-Level/Mathematics-9709");
        assert_eq!(p.len(), 1);
        assert!(p[0].source_url.contains("A-Level/Mathematics"));
        assert!(validate_paper(&p[0]));
    }
    #[test]
    fn strict_query_and_path_validation() {
        let page =
            r#"{"name":"9709_s23_qp_12.pdf","relPath":"../9709_s23_qp_12.pdf","isDir":false}"#;
        assert!(entries(page).is_empty());
        assert!(papa_candidates("9709_s22_qp_12.pdf 0620_s23_qp_12.pdf", &q()).is_empty());
        let p = papa_candidates(
            "9709_S23_QP_12.PDF 9709_s23_qp_12.pdf 9709_s23_er.pdf",
            &q(),
        );
        assert_eq!(p.len(), 2);
        assert!(p.iter().all(validate_paper));
        for bad in ["A/../B", "A/%2e/B", "A//B", "A/https:/B", "A/\\B"] {
            assert!(!safe_segments(bad, '/'));
        }
    }
    #[test]
    fn challenge_detection() {
        assert!(challenge("<title>Just a moment...</title>"));
        assert!(challenge("CF-CHL-test"));
        assert!(!challenge("normal listing"));
    }
}
