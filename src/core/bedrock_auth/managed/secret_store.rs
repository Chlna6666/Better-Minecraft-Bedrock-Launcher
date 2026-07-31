use super::XboxProfile;
use keyring::{Entry, Error as KeyringError};
use secrecy::{ExposeSecret as _, SecretSlice, SecretString};
use serde::{Deserialize, Serialize};

const SERVICE_NAME: &str = "com.bmcbl.app.bedrock-auth";
const LEGACY_REFRESH_TOKEN_KEY: &str = "microsoft-refresh-token";
const ACCOUNT_INDEX_KEY: &str = "microsoft-account-index";
const ACCOUNT_REFRESH_TOKEN_PREFIX: &str = "microsoft-refresh-token:";
const DEVICE_PRIVATE_KEY: &str = "xbox-device-p256-key";
const ACCOUNT_INDEX_VERSION: u8 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct AccountCatalog {
    #[serde(default = "account_index_version")]
    version: u8,
    #[serde(default)]
    pub(super) active_account_id: Option<String>,
    #[serde(default)]
    pub(super) profiles: Vec<XboxProfile>,
}

impl Default for AccountCatalog {
    fn default() -> Self {
        Self {
            version: ACCOUNT_INDEX_VERSION,
            active_account_id: None,
            profiles: Vec::new(),
        }
    }
}

impl AccountCatalog {
    pub(super) fn profile(&self, account_id: &str) -> Option<&XboxProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.xuid == account_id)
    }

    fn normalize(mut self) -> Result<Self, String> {
        if self.version != ACCOUNT_INDEX_VERSION {
            return Err(format!("不支持的 Microsoft 账号索引版本：{}", self.version));
        }
        for profile in &self.profiles {
            validate_profile(profile)?;
        }
        self.profiles
            .sort_by(|left, right| left.xuid.cmp(&right.xuid));
        self.profiles
            .dedup_by(|left, right| left.xuid == right.xuid);
        if self
            .active_account_id
            .as_deref()
            .is_some_and(|account_id| self.profile(account_id).is_none())
        {
            self.active_account_id = None;
        }
        if self.active_account_id.is_none() {
            self.active_account_id = self.profiles.first().map(|profile| profile.xuid.clone());
        }
        Ok(self)
    }
}

fn account_index_version() -> u8 {
    ACCOUNT_INDEX_VERSION
}

fn entry(name: &str) -> Result<Entry, String> {
    Entry::new(SERVICE_NAME, name).map_err(|error| format!("无法连接系统凭证存储：{error}"))
}

fn validate_account_id(account_id: &str) -> Result<(), String> {
    if account_id.is_empty()
        || account_id.len() > 32
        || !account_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("Xbox 账号标识无效".to_string());
    }
    Ok(())
}

fn validate_profile(profile: &XboxProfile) -> Result<(), String> {
    validate_account_id(&profile.xuid)?;
    if profile.gamertag.trim().is_empty() || profile.display_name.trim().is_empty() {
        return Err("Xbox 账号资料不完整".to_string());
    }
    if profile
        .avatar_url
        .as_deref()
        .is_some_and(|url| !url.starts_with("https://"))
    {
        return Err("Xbox 头像地址不安全".to_string());
    }
    Ok(())
}

fn refresh_token_entry(account_id: &str) -> Result<Entry, String> {
    validate_account_id(account_id)?;
    entry(&format!("{ACCOUNT_REFRESH_TOKEN_PREFIX}{account_id}"))
}

pub(super) fn load_account_catalog() -> Result<AccountCatalog, String> {
    match entry(ACCOUNT_INDEX_KEY)?.get_secret() {
        Ok(secret) if secret.is_empty() => Ok(AccountCatalog::default()),
        Ok(secret) => serde_json::from_slice::<AccountCatalog>(&secret)
            .map_err(|error| format!("Microsoft 账号索引已损坏：{error}"))?
            .normalize(),
        Err(KeyringError::NoEntry) => Ok(AccountCatalog::default()),
        Err(error) => Err(format!("无法读取 Microsoft 账号索引：{error}")),
    }
}

fn store_account_catalog(catalog: &AccountCatalog) -> Result<(), String> {
    let catalog = catalog.clone().normalize()?;
    if catalog.profiles.is_empty() {
        return match entry(ACCOUNT_INDEX_KEY)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => Err(format!("无法清空 Microsoft 账号索引：{error}")),
        };
    }
    let encoded = serde_json::to_vec(&catalog)
        .map_err(|error| format!("无法序列化 Microsoft 账号索引：{error}"))?;
    entry(ACCOUNT_INDEX_KEY)?
        .set_secret(&encoded)
        .map_err(|error| format!("无法加密保存 Microsoft 账号索引：{error}"))
}

pub(super) fn load_account_refresh_token(account_id: &str) -> Result<Option<SecretString>, String> {
    match refresh_token_entry(account_id)?.get_password() {
        Ok(token) if token.is_empty() => Ok(None),
        Ok(token) => Ok(Some(SecretString::from(token))),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(format!("无法读取 Microsoft 登录凭证：{error}")),
    }
}

pub(super) fn load_active_account()
-> Result<Option<(AccountCatalog, XboxProfile, SecretString)>, String> {
    let catalog = load_account_catalog()?;
    let Some(account_id) = catalog.active_account_id.as_deref() else {
        return Ok(None);
    };
    let profile = catalog
        .profile(account_id)
        .cloned()
        .ok_or_else(|| "Microsoft 账号索引缺少当前账号资料".to_string())?;
    let token = load_account_refresh_token(account_id)?
        .ok_or_else(|| format!("账号 {} 的加密登录凭证不存在", profile.gamertag))?;
    Ok(Some((catalog, profile, token)))
}

pub(super) fn store_account(
    profile: &XboxProfile,
    token: &SecretString,
) -> Result<AccountCatalog, String> {
    validate_profile(profile)?;
    if token.expose_secret().is_empty() {
        return Err("拒绝保存空的 Microsoft 登录凭证".to_string());
    }
    refresh_token_entry(&profile.xuid)?
        .set_password(token.expose_secret())
        .map_err(|error| format!("无法加密保存 Microsoft 登录凭证：{error}"))?;

    let mut catalog = load_account_catalog()?;
    catalog.profiles.retain(|saved| saved.xuid != profile.xuid);
    catalog.profiles.push(profile.clone());
    catalog.active_account_id = Some(profile.xuid.clone());
    catalog = catalog.normalize()?;
    store_account_catalog(&catalog)?;
    Ok(catalog)
}

pub(super) fn remove_account(account_id: &str) -> Result<(AccountCatalog, bool), String> {
    validate_account_id(account_id)?;
    let mut catalog = load_account_catalog()?;
    let was_active = catalog.active_account_id.as_deref() == Some(account_id);
    match refresh_token_entry(account_id)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("无法删除 Microsoft 登录凭证：{error}")),
    }?;
    catalog
        .profiles
        .retain(|profile| profile.xuid != account_id);
    if was_active {
        catalog.active_account_id = catalog.profiles.first().map(|profile| profile.xuid.clone());
    }
    catalog = catalog.normalize()?;
    store_account_catalog(&catalog)?;
    Ok((catalog, was_active))
}

pub(super) fn load_legacy_refresh_token() -> Result<Option<SecretString>, String> {
    match entry(LEGACY_REFRESH_TOKEN_KEY)?.get_password() {
        Ok(token) if token.is_empty() => Ok(None),
        Ok(token) => Ok(Some(SecretString::from(token))),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(format!("无法读取旧版 Microsoft 登录凭证：{error}")),
    }
}

pub(super) fn delete_legacy_refresh_token() -> Result<(), String> {
    match entry(LEGACY_REFRESH_TOKEN_KEY)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(format!("无法删除旧版 Microsoft 登录凭证：{error}")),
    }
}

pub(super) fn load_device_private_key() -> Result<Option<SecretSlice<u8>>, String> {
    match entry(DEVICE_PRIVATE_KEY)?.get_secret() {
        Ok(secret) if secret.is_empty() => Ok(None),
        Ok(secret) => Ok(Some(SecretSlice::from(secret))),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(error) => Err(format!("无法读取 Xbox 设备密钥：{error}")),
    }
}

pub(super) fn store_device_private_key(key: &SecretSlice<u8>) -> Result<(), String> {
    if key.expose_secret().len() != 32 {
        return Err("拒绝保存无效的 Xbox P-256 设备密钥".to_string());
    }
    entry(DEVICE_PRIVATE_KEY)?
        .set_secret(key.expose_secret())
        .map_err(|error| format!("无法加密保存 Xbox 设备密钥：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(xuid: &str, gamertag: &str) -> XboxProfile {
        XboxProfile {
            xuid: xuid.to_string(),
            gamertag: gamertag.to_string(),
            display_name: gamertag.to_string(),
            gamerscore: None,
            avatar_url: None,
        }
    }

    #[test]
    fn normalize_catalog_selects_first_account_when_active_is_missing() {
        let catalog = AccountCatalog {
            version: ACCOUNT_INDEX_VERSION,
            active_account_id: Some("999".to_string()),
            profiles: vec![profile("200", "Second"), profile("100", "First")],
        }
        .normalize()
        .expect("valid catalog should normalize");

        assert_eq!(catalog.active_account_id.as_deref(), Some("100"));
        assert_eq!(catalog.profiles[0].xuid, "100");
    }

    #[test]
    fn normalize_catalog_rejects_non_numeric_xuid() {
        let error = AccountCatalog {
            version: ACCOUNT_INDEX_VERSION,
            active_account_id: None,
            profiles: vec![profile("../token", "Unsafe")],
        }
        .normalize()
        .expect_err("unsafe account id must be rejected");

        assert_eq!(error, "Xbox 账号标识无效");
    }
}
