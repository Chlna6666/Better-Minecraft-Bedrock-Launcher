use super::{CurseForgeClient, CurseForgeFile, CurseForgeMod};

#[derive(Clone, Debug)]
pub struct CurseForgeModSummaryData {
    pub id: i32,
    pub name: String,
    pub summary: Option<String>,
    pub author_names: Vec<String>,
    pub logo_url: Option<String>,
    pub download_count: f64,
    pub date_modified: String,
    pub class_id: Option<i32>,
    pub category_ids: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct CurseForgeModPageData {
    pub mod_entry: CurseForgeModSummaryData,
    pub description_html: String,
}

#[derive(Clone, Debug)]
pub struct CurseForgeFileData {
    pub id: i32,
    pub display_name: String,
    pub file_name: String,
    pub file_length: u64,
    pub download_url: Option<String>,
    pub game_versions: Vec<String>,
    pub file_date: String,
}

impl From<CurseForgeMod> for CurseForgeModSummaryData {
    fn from(value: CurseForgeMod) -> Self {
        Self {
            id: value.id,
            name: value.name,
            summary: value.summary,
            author_names: value
                .authors
                .into_iter()
                .map(|author| author.name)
                .collect(),
            logo_url: value
                .logo
                .map(|logo| logo.thumbnail_url.unwrap_or(logo.url)),
            download_count: value.download_count,
            date_modified: value.date_modified,
            class_id: value.class_id,
            category_ids: value
                .categories
                .into_iter()
                .map(|category| category.id)
                .collect(),
        }
    }
}

impl From<CurseForgeFile> for CurseForgeFileData {
    fn from(value: CurseForgeFile) -> Self {
        Self {
            id: value.id,
            display_name: value.display_name,
            file_name: value.file_name,
            file_length: value.file_length,
            download_url: value.download_url,
            game_versions: value.game_versions,
            file_date: value.file_date,
        }
    }
}

pub async fn load_mod_page(
    mod_id: i32,
    cached_mod_entry: Option<CurseForgeModSummaryData>,
) -> Result<CurseForgeModPageData, String> {
    let client = CurseForgeClient::new()?;
    let mod_entry = match cached_mod_entry {
        Some(entry) => entry,
        None => client.get_mod(mod_id).await?.into(),
    };
    let description_html = client.get_mod_description(mod_id).await.unwrap_or_default();

    Ok(CurseForgeModPageData {
        mod_entry,
        description_html,
    })
}

pub async fn load_mod_files(
    mod_id: i32,
    game_version: Option<String>,
) -> Result<Vec<CurseForgeFileData>, String> {
    let client = CurseForgeClient::new()?;
    let response = client
        .get_mod_files(mod_id, normalize_game_version(game_version))
        .await?;

    Ok(response.data.into_iter().map(Into::into).collect())
}

fn normalize_game_version(game_version: Option<String>) -> Option<String> {
    game_version.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("all") {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}
