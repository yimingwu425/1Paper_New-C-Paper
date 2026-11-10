//! EasyPaper protocol constants mirror the Swift adapter, not user credentials.
//! Persist only logical paths; signed URLs are generated just before transport.
use crate::{bad, query_parts, upstream, ErrApi, Paper, Search};
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::Rng;
use serde::Deserialize;

const ROOTS: [&str; 4] = [
    "CAIE|AS and A Level",
    "CAIE|IGCSE",
    "CAIE|O Level",
    "CAIE|Pre-U",
];
const BASE: &str = "https://server.easy-paper.com/paperdownload/dir_v3";
const REPLACE: &str = "%fo@~[C.4L1.ZDcp";
const REQUEST_KEY: &[u8; 32] = b"hQgPeYGoPJAHatNyBejnINyJ7Ffb5fjV";
const REQUEST_IV: &[u8; 16] = b"8!cH-3SyWhZ9I&/1";
const RESPONSE_KEY: &[u8; 32] = b"w`:oI%@`N*=Aod~aVKR5`jO:zK&S#x~1";
const RESPONSE_IV: &[u8; 16] = b"uf5h6Kall8g0o3GL";

#[derive(Debug, Deserialize)]
struct Directory {
    status: bool,
    #[serde(default)]
    folders: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    current_dir: Option<String>,
    #[serde(default)]
    locked_resources: Vec<String>,
}

fn token_at(path: &str, seconds: u64, prefix: &str) -> Result<String, ErrApi> {
    if prefix.len() != 10 || !prefix.is_ascii() || !crate::provider_html::safe_segments(path, '|') {
        return Err(bad("EasyPaper 逻辑路径无效"));
    }
    let payload = serde_json::json!({"dir":path,"source":"website","time":seconds.to_string()});
    let plaintext = format!("{prefix}{payload}");
    let encrypted = cbc::Encryptor::<aes::Aes256>::new(REQUEST_KEY.into(), REQUEST_IV.into())
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());
    let replaced = STANDARD.encode(encrypted).replace('/', REPLACE);
    // URL path component, not form encoding (spaces and plus signs are not interchangeable).
    Ok(replaced
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect())
}

fn token(path: &str) -> Result<String, ErrApi> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKMNP9gqQRSToOLVvI1lWXYZabcdefhijkmnprstwxyz2345678";
    let mut rng = rand::thread_rng();
    let prefix: String = (0..10)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| bad("系统时间无效"))?
        .as_secs();
    token_at(path, now, &prefix)
}

fn decrypt(data: &[u8]) -> Result<Directory, ErrApi> {
    let text = std::str::from_utf8(data).map_err(|_| upstream("EasyPaper 加密响应无效"))?;
    let ciphertext = STANDARD
        .decode(text.trim().replace(REPLACE, "/"))
        .map_err(|_| upstream("EasyPaper 加密响应无效"))?;
    let clear = cbc::Decryptor::<aes::Aes256>::new(RESPONSE_KEY.into(), RESPONSE_IV.into())
        .decrypt_padded_vec_mut::<Pkcs7>(&ciphertext)
        .map_err(|_| upstream("EasyPaper 响应解密失败"))?;
    let json = clear
        .get(10..)
        .ok_or_else(|| upstream("EasyPaper 响应前缀缺失"))?;
    serde_json::from_slice(json).map_err(|_| upstream("EasyPaper 目录格式无效"))
}

async fn directory(client: &reqwest::Client, path: &str) -> Result<Directory, ErrApi> {
    let url = reqwest::Url::parse(&format!("{BASE}/{}", token(path)?))
        .map_err(|_| bad("EasyPaper 地址无效"))?;
    let data = crate::provider_html::body(client, url).await?;
    let dir = decrypt(&data)?;
    if !dir.status {
        return Err(upstream("EasyPaper 目录 API 返回失败"));
    }
    if dir.current_dir.as_deref().is_some_and(|p| p != path) {
        return Err(upstream("EasyPaper 返回的目录不匹配"));
    }
    Ok(dir)
}

fn papers(dir: &Directory, path: &str, q: &Search) -> Vec<Paper> {
    let mut result = std::collections::BTreeMap::new();
    for name in &dir.files {
        let logical = format!("{path}|{name}");
        if dir.locked_resources.contains(name) || dir.locked_resources.contains(&logical) {
            continue;
        }
        if let Some(p) = crate::provider_html::candidate(q, "easypaper", name, logical, BASE.into())
        {
            if validate_paper(&p) {
                result.insert(p.filename.clone(), p);
            }
        }
    }
    result.into_values().collect()
}

pub(crate) async fn search(client: &reqwest::Client, q: &Search) -> Result<Vec<Paper>, ErrApi> {
    let (_, prefix) = query_parts(q)?;
    let mut subject = None;
    for root in ROOTS {
        let dir = directory(client, root).await?;
        if let Some(name) = dir.folders.iter().find(|s| {
            s.ends_with(&format!("({})", q.subject))
                && crate::provider_html::safe_segments(s, '|')
                && !s.contains('|')
        }) {
            subject = Some(format!("{root}|{name}"));
            break;
        }
    }
    let subject = subject.ok_or_else(|| upstream("EasyPaper 找不到该科目目录"))?;
    let season = match prefix {
        'm' => "March",
        's' => "Summer",
        _ => "Winter",
    };
    let path = format!("{subject}|{}|{season}", q.year);
    let dir = directory(client, &path).await?;
    let result = papers(&dir, &path, q);
    if result.is_empty() {
        return Err(upstream("EasyPaper 没有匹配的可下载试卷（可能为空或受限）"));
    }
    Ok(result)
}

pub(crate) async fn subjects(client: &reqwest::Client) -> Result<Vec<crate::Subject>, ErrApi> {
    let mut subjects = std::collections::BTreeMap::new();
    for root in ROOTS {
        let dir = directory(client, root).await?;
        for name in dir.folders {
            if let Some(s) = crate::provider_html::subject_name(&name) {
                subjects.insert(s.code.clone(), s);
            }
        }
    }
    if subjects.is_empty() {
        return Err(upstream("EasyPaper 科目目录为空"));
    }
    Ok(subjects.into_values().collect())
}

pub(crate) fn validate_paper(p: &Paper) -> bool {
    let Some(path) = &p.logical_path else {
        return false;
    };
    if p.source != "easypaper"
        || p.source_url != BASE
        || !crate::provider_html::safe_segments(path, '|')
    {
        return false;
    }
    let parts: Vec<_> = path.split('|').collect();
    if parts.len() != 6
        || !ROOTS.contains(&format!("{}|{}", parts[0], parts[1]).as_str())
        || parts[5] != p.filename
    {
        return false;
    }
    let Some((subject, session, _, _)) = crate::parse(&p.filename) else {
        return false;
    };
    let Ok(year) = parts[3].parse::<u16>() else {
        return false;
    };
    let season = match parts[4] {
        "March" => 'm',
        "Summer" => 's',
        "Winter" => 'w',
        _ => return false,
    };
    (2000..=2099).contains(&year)
        && parts[2].ends_with(&format!("({subject})"))
        && session == format!("{season}{:02}", year % 100)
}

pub(crate) fn resolve_download_url(path: &str) -> Result<reqwest::Url, ErrApi> {
    let filename = path.rsplit('|').next().unwrap_or("");
    let p = Paper {
        id: String::new(),
        filename: filename.into(),
        kind: String::new(),
        source_url: BASE.into(),
        source: "easypaper".into(),
        logical_path: Some(path.into()),
    };
    if !validate_paper(&p) {
        return Err(bad("EasyPaper 下载路径无效"));
    }
    reqwest::Url::parse(&format!("{BASE}/{}", token(path)?))
        .map_err(|_| bad("EasyPaper 下载地址无效"))
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROOT_FIXTURE: &str = "iQ9xgnNO3+fFvnA+5LNnpwktR8VRKwQIAeZV+sqpHltExpiP9r1eEkMVXkF%fo@~[C.4L1.ZDcppk0Ow7Y%fo@~[C.4L1.ZDcp3kS5AUFLQI4RT7J6rJcl0AbExmVJn1L612XmYtYQeUDjDPxJvHPjcLIuPpmaOLPc%fo@~[C.4L1.ZDcpKxrszyh36JWH3ofbUG5AiMIY7PupPAvDS4MkYc=";
    #[test]
    fn swift_response_fixture_decrypts() {
        let d = decrypt(ROOT_FIXTURE.as_bytes()).unwrap();
        assert!(d.status);
        assert!(d.folders.contains(&"Mathematics (9709)".into()));
    }
    #[test]
    fn deterministic_token_roundtrip_and_encoding() {
        let path = "CAIE|AS and A Level|Mathematics (9709)|2023|Summer|9709_s23_qp_12.pdf";
        let t = token_at(path, 1_700_000_000, "AAAAAAAAAA").unwrap();
        assert_eq!(t, token_at(path, 1_700_000_000, "AAAAAAAAAA").unwrap());
        assert!(!t.contains('/'));
        assert!(!t.contains('+'));
        let u = reqwest::Url::parse(&format!("https://example.test/?v={t}")).unwrap();
        let encoded = u.query_pairs().next().unwrap().1.replace(REPLACE, "/");
        let raw = STANDARD.decode(encoded).unwrap();
        let clear = cbc::Decryptor::<aes::Aes256>::new(REQUEST_KEY.into(), REQUEST_IV.into())
            .decrypt_padded_vec_mut::<Pkcs7>(&raw)
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&clear[10..]).unwrap();
        assert_eq!(value["dir"], path);
        assert_eq!(value["time"], "1700000000");
        assert_eq!(value["source"], "website");
    }
    #[test]
    fn refresh_and_invalid_paths() {
        let path = "CAIE|AS and A Level|Mathematics (9709)|2023|Summer|9709_s23_qp_12.pdf";
        assert_ne!(
            resolve_download_url(path).unwrap(),
            resolve_download_url(path).unwrap()
        );
        for invalid in [
            path.replace("2023", "2022"),
            path.replace("9709)", "0620)"),
            path.replace("Mathematics", "../Mathematics"),
            path.replace("Summer", "Winter"),
        ] {
            assert!(resolve_download_url(&invalid).is_err());
        }
        assert!(decrypt(b"<html>challenge</html>").is_err());
    }
    #[test]
    fn locked_wrong_query_and_metadata_are_filtered() {
        let q = Search {
            subject: "9709".into(),
            year: 2023,
            season: "Jun".into(),
        };
        let d = Directory {
            status: true,
            folders: vec![],
            current_dir: None,
            locked_resources: vec!["9709_s23_ms_12.pdf".into()],
            files: vec![
                "9709_s23_ms_12.pdf".into(),
                "9709_s23_gt.pdf".into(),
                "9709_w23_qp_12.pdf".into(),
            ],
        };
        let p = papers(&d, "CAIE|AS and A Level|Mathematics (9709)|2023|Summer", &q);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, "gt");
        assert_eq!(p[0].source_url, BASE);
    }
}
