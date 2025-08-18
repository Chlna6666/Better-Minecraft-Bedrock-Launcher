use fluent_bundle::{FluentResource, FluentArgs};
use fluent_bundle::concurrent::FluentBundle;
use unic_langid::LanguageIdentifier;
use once_cell::sync::OnceCell;
use std::sync::RwLock;
use std::fs;
use std::path::PathBuf;
use tracing::debug;

// 嵌入 FTL 文件内容为静态常量
const EN_US_FTL: &str = include_str!("locales/en-US.ftl");
const ZH_CN_FTL: &str = include_str!("locales/zh-CN.ftl");

// 支持的语言及其对应的 FTL 内容
const LOCALES: &[(&str, &str)] = &[
    ("en-US", EN_US_FTL),
    ("zh-CN", ZH_CN_FTL),
];

/// 全局 i18n 管理器
pub struct I18n {
    bundle: RwLock<FluentBundle<FluentResource>>,
    current_lang: RwLock<LanguageIdentifier>,
    locales_dir: PathBuf,
}

pub static I18N: OnceCell<I18n> = OnceCell::new();

impl I18n {
    /// 初始化，加载默认语言
    pub fn init(default: &str) {
        let langid: LanguageIdentifier = default.parse().expect("Invalid locale");
        debug!("I18n init: default locale = {}", langid);

        let bundle = FluentBundle::new_concurrent(vec![langid.clone()]);
        let i18n = I18n {
            bundle: RwLock::new(bundle),
            current_lang: RwLock::new(langid.clone()),
            locales_dir: PathBuf::new(),
        };

        if I18N.set(i18n).is_err() {
            panic!("I18n already initialized");
        }

        I18n::reload();
    }


    /// 切换语言并 reload 资源
    pub fn set_locale(locale: &str) {
        debug!("I18n set_locale: switching to {}", locale);
        let langid: LanguageIdentifier = locale.parse().expect("Invalid locale");
        let i18n = I18N.get().expect("I18n not initialized");
        *i18n.current_lang.write().unwrap() = langid.clone();

        // 重建 bundle
        let new_bundle = FluentBundle::new_concurrent(vec![langid.clone()]);
        *i18n.bundle.write().unwrap() = new_bundle;

        I18n::reload();
    }

    /// 读取对应 FTL 文件并加载到 bundle，若文件缺失则回退到 en-US
    /// 重新加载内嵌资源
    pub fn reload() {
        let i18n = I18N.get().expect("I18n not initialized");
        let lang = i18n.current_lang.read().unwrap().to_string();

        debug!("I18n reload: current_lang = {}", lang);

        let source = LOCALES.iter()
            .find(|(code, _)| *code == lang)
            .map(|(_, content)| *content)
            .or_else(|| {
                debug!("Fallback to en-US");
                LOCALES.iter().find(|(code, _)| *code == "en-US").map(|(_, content)| *content)
            })
            .unwrap_or_else(|| panic!("Missing i18n data for {} and fallback en-US", lang));

        let res = FluentResource::try_new(source.to_string()).expect("Failed to parse FTL content");

        let mut bundle = i18n.bundle.write().unwrap();
        bundle.add_resource(res).expect("Failed to add Fluent resource");

        debug!("I18n reload: loaded resources for {}", lang);
    }

    /// 获取翻译字符串
    pub fn t(key: &str, args: Option<&FluentArgs>) -> String {
        debug!("I18n t(): key = {}", key);
        let i18n = I18N.get().expect("I18n not initialized");
        let bundle = i18n.bundle.read().unwrap();

        let m = bundle
            .get_message(key)
            .and_then(|msg| msg.value());

        match m {
            Some(message) => {
                let mut errors = vec![];
                let value = bundle.format_pattern(message, args, &mut errors).to_string();
                if !errors.is_empty() {
                    debug!("  formatting errors = {:?}", errors);
                }
                debug!("  formatted value = {}", value);
                value
            }
            None => {
                debug!("I18n: key '{}' not found", key);
                // 这里返回 key 本身，避免 panic
                key.to_string()
            }
        }
    }
}
