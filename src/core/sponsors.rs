use crate::http::proxy::get_client_for_proxy;
use serde::Deserialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SPONSORS_API_URL: &str = "https://api.chlna6666.com/api/v1/sponsors";
const SPONSOR_API_HOST: &str = "api.chlna6666.com";
const SPONSOR_AVATAR_CACHE_DIR_NAME: &str = "bmbl_sponsors_avatar_cache";

#[derive(Clone, Debug)]
pub(crate) struct SponsorRecord {
    pub(crate) user_id: String,
    pub(crate) name: String,
    pub(crate) avatar_path: String,
    pub(crate) total_amount: String,
}

#[derive(Deserialize)]
struct SponsorsApiResponse {
    data: Option<Vec<SponsorItem>>,
}

#[derive(Deserialize)]
struct SponsorItem {
    all_sum_amount: Option<String>,
    user: Option<SponsorUser>,
}

#[derive(Deserialize)]
struct SponsorUser {
    avatar: Option<String>,
    name: Option<String>,
    user_id: Option<String>,
}

pub(crate) fn clear_avatar_cache() -> io::Result<()> {
    let cache_dir = sponsor_avatar_cache_dir();
    match fs::remove_dir_all(&cache_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::create_dir_all(cache_dir)
}

pub(crate) async fn load_sponsors() -> Result<Vec<SponsorRecord>, String> {
    let parsed_url = reqwest::Url::parse(SPONSORS_API_URL).map_err(|error| error.to_string())?;
    if parsed_url.host_str() != Some(SPONSOR_API_HOST) {
        return Err("host not allowed".to_string());
    }

    let client = get_client_for_proxy().map_err(|error| error.to_string())?;
    let response = client
        .get(parsed_url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }

    let response_text = response.text().await.map_err(|error| error.to_string())?;
    let mut parsed_json: SponsorsApiResponse =
        serde_json::from_str(&response_text).map_err(|error| error.to_string())?;
    let mut items = parsed_json.data.take().unwrap_or_default();

    items.sort_by(|left, right| {
        let left_timestamp = left
            .user
            .as_ref()
            .and_then(|user| user.user_id.as_deref())
            .and_then(uuid_v1_to_unix_ms);
        let right_timestamp = right
            .user
            .as_ref()
            .and_then(|user| user.user_id.as_deref())
            .and_then(uuid_v1_to_unix_ms);

        match (left_timestamp, right_timestamp) {
            (Some(left_timestamp), Some(right_timestamp)) => right_timestamp.cmp(&left_timestamp),
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, None) => {
                let left_amount = left
                    .all_sum_amount
                    .as_deref()
                    .unwrap_or("0")
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let right_amount = right
                    .all_sum_amount
                    .as_deref()
                    .unwrap_or("0")
                    .parse::<f64>()
                    .unwrap_or(0.0);
                right_amount
                    .partial_cmp(&left_amount)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        }
    });

    let cache_dir = sponsor_avatar_cache_dir();
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|error| error.to_string())?;

    let mut sponsors = Vec::with_capacity(items.len());
    for item in items {
        let Some(user) = item.user else {
            continue;
        };

        let user_id = user.user_id.unwrap_or_default();
        let name = user.name.unwrap_or_default();
        let avatar_remote = user.avatar.unwrap_or_default();
        let avatar_path = download_avatar_to_local(&client, &avatar_remote, &user_id, &cache_dir)
            .await
            .unwrap_or_default();

        sponsors.push(SponsorRecord {
            user_id,
            name,
            avatar_path,
            total_amount: item.all_sum_amount.unwrap_or_default(),
        });
    }

    Ok(sponsors)
}

fn sponsor_avatar_cache_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        crate::utils::file_ops::cache_subdir(SPONSOR_AVATAR_CACHE_DIR_NAME)
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::temp_dir().join(SPONSOR_AVATAR_CACHE_DIR_NAME)
    }
}

fn sanitize_filename(input: &str) -> String {
    let output: String = input
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();

    if output.is_empty() {
        "unknown".to_string()
    } else {
        output
    }
}

async fn download_avatar_to_local(
    client: &reqwest::Client,
    url: &str,
    user_id: &str,
    cache_dir: &Path,
) -> Option<String> {
    if url.is_empty() {
        return None;
    }

    let parsed_url = reqwest::Url::parse(url).ok()?;
    if !matches!(parsed_url.scheme(), "http" | "https") {
        return None;
    }

    let file_name = parsed_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .unwrap_or("avatar.png");

    let extension = Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("png");
    let local_path = cache_dir.join(format!(
        "{}_avatar.{}",
        sanitize_filename(user_id),
        extension
    ));

    if tokio::fs::try_exists(&local_path).await.ok()? {
        return Some(local_path.to_string_lossy().into_owned());
    }

    let response = client.get(parsed_url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    let bytes = response.bytes().await.ok()?;
    tokio::fs::write(&local_path, &bytes).await.ok()?;

    Some(local_path.to_string_lossy().into_owned())
}

fn uuid_v1_to_unix_ms(input: &str) -> Option<i64> {
    let mut cleaned = String::with_capacity(32);
    for character in input.chars() {
        if character.is_ascii_hexdigit() {
            cleaned.push(character.to_ascii_lowercase());
        }
    }

    if cleaned.len() != 32 {
        return None;
    }

    let time_low = u32::from_str_radix(&cleaned[0..8], 16).ok()? as u64;
    let time_mid = u16::from_str_radix(&cleaned[8..12], 16).ok()? as u64;
    let time_high_and_version = u16::from_str_radix(&cleaned[12..16], 16).ok()? as u64;

    if (time_high_and_version >> 12) & 0xF != 1 {
        return None;
    }

    let time_high = time_high_and_version & 0x0FFF;
    let timestamp_100ns = (time_high << 48) | (time_mid << 32) | time_low;

    const GREGORIAN_TO_UNIX_100NS: i128 = 0x01B21DD213814000;
    let unix_100ns = i128::from(timestamp_100ns) - GREGORIAN_TO_UNIX_100NS;
    if unix_100ns <= 0 {
        return None;
    }

    i64::try_from(unix_100ns / 10_000).ok()
}
