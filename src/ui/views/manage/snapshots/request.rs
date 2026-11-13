use super::*;

pub(super) struct Request<'a> {
    pub version: &'a ManagedVersionEntry,
    pub config: &'a ManageVersionConfig,
    pub tab: ManageTab,
    pub subtype: ManagePackSubtype,
    pub user: Option<&'a str>,
    pub locale: &'a str,
}

impl Request<'_> {
    pub(super) async fn key(&self) -> Result<Key, String> {
        let source = format!(
            "{:?}:{:?}",
            self.version.build_type(),
            self.version.edition()
        );
        let user_scoped = matches!(
            self.tab,
            ManageTab::Map | ManageTab::Screenshot | ManageTab::Server
        );
        let localized = matches!(
            self.tab,
            ManageTab::ResourcePack | ManageTab::SkinPack | ManageTab::Cape
        );
        Ok(Key {
            root: self.root().await?,
            kind: self.kind(),
            locale: if localized {
                self.locale.to_owned()
            } else {
                String::new()
            },
            user: if user_scoped {
                self.user.map(str::to_owned)
            } else {
                None
            },
            source,
        })
    }

    fn kind(&self) -> Kind {
        match self.tab {
            ManageTab::Mod => Kind::Mod,
            ManageTab::ResourcePack => match self.subtype {
                ManagePackSubtype::Resource => Kind::ResourcePack,
                ManagePackSubtype::Behavior => Kind::BehaviorPack,
            },
            ManageTab::SkinPack => Kind::SkinPack,
            ManageTab::Cape => Kind::Cape,
            ManageTab::Map => Kind::World,
            ManageTab::Screenshot => Kind::Screenshot,
            ManageTab::Server => Kind::Server,
            ManageTab::Statistics => Kind::Statistics,
        }
    }

    async fn root(&self) -> Result<PathBuf, String> {
        let options = GamePathOptions {
            build_type: self.version.build_type(),
            edition: self.version.edition(),
            version_name: self.version.folder.to_string(),
            enable_isolation: self.config.enable_redirection,
            user_id: None,
            allow_shared_fallback: false,
        };
        let mods = self.tab == ManageTab::Mod;
        let mod_root = PathBuf::from(self.version.path.as_ref());
        run_io(move || {
            if mods {
                Ok(mod_root)
            } else {
                resolve_game_root(&options).ok_or_else(|| "无法解析 Minecraft 根目录".to_owned())
            }
        })
        .await?
    }

    pub(super) async fn validate(&self, expected: &Key) -> Result<(), String> {
        // The resolver reloads instance redirection settings. A configuration change
        // between lookup and parsing must not store data under the former root.
        if self.root().await? != expected.root {
            let cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(cached) = cache.entries.get(expected) {
                cached.entry.root.invalidate();
            }
        }
        Ok(())
    }
}
