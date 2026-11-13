use super::*;

pub(in crate::ui::views::manage) struct Observation {
    instance: Arc<watch::Root>,
    game: Arc<watch::Root>,
    instance_generations: [u64; Kind::COUNT],
    game_generations: [u64; Kind::COUNT],
}

#[derive(Default)]
pub(in crate::ui::views::manage) struct Refresh {
    pub config: bool,
    pub users: bool,
    pub assets: bool,
    pub screenshots: bool,
    pub servers: bool,
}

impl Observation {
    pub(in crate::ui::views::manage) async fn new(
        version: &ManagedVersionEntry,
    ) -> Result<Self, String> {
        let instance = PathBuf::from(version.path.as_ref());
        let options = GamePathOptions {
            build_type: version.build_type(),
            edition: version.edition(),
            version_name: version.folder.to_string(),
            enable_isolation: false,
            user_id: None,
            allow_shared_fallback: false,
        };
        run_io(move || {
            let game = resolve_game_root(&options)
                .ok_or_else(|| "无法解析 Minecraft 根目录".to_string())?;
            let (instance, game) = {
                let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
                (cache.root(&instance, true), cache.root(&game, false))
            };
            instance.refresh();
            game.refresh();
            instance.begin_scan(Kind::Config);
            game.begin_scan(Kind::Users);
            Ok(Self::capture(instance, game))
        })
        .await?
    }

    fn capture(instance: Arc<watch::Root>, game: Arc<watch::Root>) -> Self {
        Self {
            instance_generations: instance.generations(),
            game_generations: game.generations(),
            instance,
            game,
        }
    }

    pub(in crate::ui::views::manage) fn take(
        &mut self,
        tab: ManageTab,
        subtype: ManagePackSubtype,
    ) -> Refresh {
        let instance = changed(&self.instance, &mut self.instance_generations);
        let game = changed(&self.game, &mut self.game_generations);
        if game & Kind::Users.bit() != 0 {
            self.game.begin_scan(Kind::Users);
        }
        self.refresh(instance, game, tab, subtype)
    }

    pub(in crate::ui::views::manage) fn has_changes(&self) -> bool {
        self.instance.generations() != self.instance_generations
            || self.game.generations() != self.game_generations
    }

    pub(in crate::ui::views::manage) fn preview(
        &self,
        tab: ManageTab,
        subtype: ManagePackSubtype,
    ) -> Refresh {
        let instance = difference(&self.instance.generations(), &self.instance_generations);
        let game = difference(&self.game.generations(), &self.game_generations);
        self.refresh(instance, game, tab, subtype)
    }

    fn refresh(
        &self,
        instance: u64,
        game: u64,
        tab: ManageTab,
        subtype: ManagePackSubtype,
    ) -> Refresh {
        let mut refresh = Refresh::new(instance, game, tab, subtype);
        // A failed native subscription must be replaced along with its root handle.
        refresh.config |= (instance != 0 && !self.instance.is_healthy())
            || (game != 0 && !self.game.is_healthy());
        refresh
    }
}

impl Refresh {
    pub(in crate::ui::views::manage) fn has_visible_changes(&self, tab: ManageTab) -> bool {
        self.config
            || self.assets
            || (self.screenshots && tab == ManageTab::Screenshot)
            || (self.servers && tab == ManageTab::Server)
            || (self.users
                && matches!(
                    tab,
                    ManageTab::Map | ManageTab::Screenshot | ManageTab::Server
                ))
    }
    fn new(instance: u64, game: u64, tab: ManageTab, subtype: ManagePackSubtype) -> Self {
        let asset_kind = match tab {
            ManageTab::Mod => Kind::Mod,
            ManageTab::ResourcePack => match subtype {
                ManagePackSubtype::Resource => Kind::ResourcePack,
                ManagePackSubtype::Behavior => Kind::BehaviorPack,
            },
            ManageTab::SkinPack => Kind::SkinPack,
            ManageTab::Cape => Kind::Cape,
            ManageTab::Map => Kind::World,
            _ => Kind::Statistics,
        };
        Self {
            config: instance & Kind::Config.bit() != 0,
            users: game & Kind::Users.bit() != 0,
            assets: (if tab == ManageTab::Mod {
                instance
            } else {
                game
            }) & asset_kind.bit()
                != 0,
            screenshots: game & Kind::Screenshot.bit() != 0,
            servers: game & Kind::Server.bit() != 0,
        }
    }
}

fn changed(root: &watch::Root, previous: &mut [u64; Kind::COUNT]) -> u64 {
    let current = root.generations();
    let mask = difference(&current, previous);
    *previous = current;
    mask
}

fn difference(current: &[u64; Kind::COUNT], previous: &[u64; Kind::COUNT]) -> u64 {
    current
        .iter()
        .zip(previous)
        .enumerate()
        .fold(0, |mask, (index, (current, previous))| {
            mask | if current != previous { 1 << index } else { 0 }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_roots_and_content_do_not_refresh_the_visible_lists() {
        let instance = Arc::new(watch::Root::new(std::path::Path::new("version"), true));
        let game = Arc::new(watch::Root::new(std::path::Path::new("game"), false));
        let mut observer = Observation::capture(instance, game.clone());
        let other = watch::Root::new(std::path::Path::new("other"), false);
        other.invalidate();
        let refresh = observer.take(ManageTab::Map, ManagePackSubtype::Resource);
        assert!(
            !refresh.config
                && !refresh.users
                && !refresh.assets
                && !refresh.servers
                && !refresh.screenshots
        );
        game.invalidate();
        assert!(
            observer
                .take(ManageTab::Map, ManagePackSubtype::Resource)
                .assets
        );
        assert!(
            !observer
                .take(ManageTab::Map, ManagePackSubtype::Resource)
                .assets
        );
    }

    #[test]
    fn world_changes_do_not_reload_resource_packs_users_or_servers() {
        let refresh = Refresh::new(
            0,
            Kind::World.bit(),
            ManageTab::ResourcePack,
            ManagePackSubtype::Resource,
        );
        assert!(
            !refresh.config
                && !refresh.users
                && !refresh.assets
                && !refresh.servers
                && !refresh.screenshots
        );
        assert!(
            Refresh::new(
                0,
                Kind::World.bit(),
                ManageTab::Map,
                ManagePackSubtype::Resource
            )
            .assets
        );
        assert!(
            Refresh::new(
                Kind::Config.bit(),
                0,
                ManageTab::Map,
                ManagePackSubtype::Resource
            )
            .config
        );
    }
}
