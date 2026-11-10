//! Package icons published in the tools registry, fetched once and served locally.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

const REGISTRY: &str =
    "https://raw.githubusercontent.com/shba007/rune-tools/refs/heads/main/registry/index.json";
const ICON_HOST_PREFIX: &str = "https://raw.githubusercontent.com/";
const LIMIT: usize = 64 * 1024;

fn cache() -> &'static Mutex<HashMap<String, Arc<Vec<u8>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<u8>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn icon_url(index: &serde_json::Value, name: &str) -> Option<String> {
    let url = index.get(name)?.get("icon")?.as_str()?;
    (url.starts_with(ICON_HOST_PREFIX) && url.ends_with(".svg") && !url.contains(['?', '#']))
        .then(|| url.to_owned())
}

pub fn is_svg(bytes: &[u8]) -> bool {
    bytes.len() <= LIMIT
        && std::str::from_utf8(bytes).is_ok_and(|text| {
            let lowered = text.to_ascii_lowercase();
            lowered.contains("<svg") && !lowered.contains("<script")
        })
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("rune-kit")
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

async fn fetch_index(client: &reqwest::Client) -> Result<serde_json::Value, String> {
    client
        .get(REGISTRY)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

#[derive(Clone, serde::Serialize)]
pub struct RegistryEntry {
    pub name: String,
    pub description: String,
    pub latest: String,
    pub has_icon: bool,
}

pub fn entries(index: &serde_json::Value) -> Vec<RegistryEntry> {
    let Some(map) = index.as_object() else {
        return Vec::new();
    };
    let mut list: Vec<_> = map
        .iter()
        .filter(|(name, value)| valid_name(name) && value.is_object())
        .map(|(name, value)| RegistryEntry {
            name: name.clone(),
            description: value
                .get("description")
                .and_then(|text| text.as_str())
                .unwrap_or_default()
                .trim()
                .chars()
                .take(300)
                .collect(),
            latest: value
                .get("latest")
                .and_then(|text| text.as_str())
                .unwrap_or_default()
                .to_owned(),
            has_icon: icon_url(index, name).is_some(),
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

pub async fn registry() -> Result<Vec<RegistryEntry>, String> {
    Ok(entries(&fetch_index(&client()?).await?))
}

pub async fn package_icon(name: &str) -> Result<Option<Arc<Vec<u8>>>, String> {
    if !valid_name(name) {
        return Ok(None);
    }
    if let Some(hit) = cache().lock().unwrap().get(name) {
        return Ok(Some(hit.clone()));
    }
    let client = client()?;
    let index = fetch_index(&client).await?;
    let Some(url) = icon_url(&index, name) else {
        return Ok(None);
    };
    let bytes = client
        .get(url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    if !is_svg(&bytes) {
        return Ok(None);
    }
    let icon = Arc::new(bytes.to_vec());
    cache()
        .lock()
        .unwrap()
        .insert(name.to_owned(), icon.clone());
    Ok(Some(icon))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_plain_package_names_and_registry_hosted_svgs_are_accepted() {
        assert!(valid_name("rune-time"));
        assert!(!valid_name("../x"));
        assert!(!valid_name("Rune"));
        let good = "https://raw.githubusercontent.com/shba007/rune-tools/refs/heads/main/mcps/rune-time/icon.svg";
        let index = json!({
            "rune-time": {"icon": good},
            "evil": {"icon": "https://example.com/icon.svg"},
            "query": {"icon": "https://raw.githubusercontent.com/a/b/c.svg?x=1"},
            "none": {}
        });
        assert_eq!(icon_url(&index, "rune-time").as_deref(), Some(good));
        for name in ["evil", "query", "none", "missing"] {
            assert!(icon_url(&index, name).is_none(), "{name}");
        }
    }

    #[test]
    fn registry_entries_are_sorted_and_flag_valid_icons() {
        let index = json!({
            "rune-b": {"description": " Second ", "latest": "1.0.0", "icon": "https://raw.githubusercontent.com/a/b/c.svg"},
            "rune-a": {"description": "First", "latest": "0.1.0"},
            "Bad Name": {"description": "x"},
            "notobject": 3
        });
        let list = entries(&index);
        assert_eq!(
            list.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["rune-a", "rune-b"]
        );
        assert_eq!(list[1].description, "Second");
        assert!(list[1].has_icon && !list[0].has_icon);
    }

    #[test]
    fn scripts_oversize_and_non_svg_bodies_are_rejected() {
        assert!(is_svg(b"<svg viewBox='0 0 1 1'></svg>"));
        assert!(!is_svg(b"<svg><script>alert(1)</script></svg>"));
        assert!(!is_svg(b"<html></html>"));
        assert!(!is_svg(&vec![b'a'; LIMIT + 1]));
    }
}
