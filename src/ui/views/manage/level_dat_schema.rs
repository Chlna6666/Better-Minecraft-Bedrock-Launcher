use crate::core::minecraft::level::LevelDatDocument;
use crate::i18n::I18nKey;
use bedrock_nbt::NbtTag;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum LevelDatFieldGroup {
    Basic,
    Gameplay,
    WorldGeneration,
    SpawnTimeWeather,
    Multiplayer,
    Abilities,
    Advanced,
    Legacy,
}

impl LevelDatFieldGroup {
    pub const ORDER: &'static [Self] = &[
        Self::Basic,
        Self::Gameplay,
        Self::WorldGeneration,
        Self::SpawnTimeWeather,
        Self::Multiplayer,
        Self::Abilities,
        Self::Advanced,
        Self::Legacy,
    ];

    pub fn title(self) -> I18nKey {
        match self {
            Self::Basic => crate::i18n_key!("LevelDat.group.basic"),
            Self::Gameplay => crate::i18n_key!("LevelDat.group.gameplay"),
            Self::WorldGeneration => crate::i18n_key!("LevelDat.group.world_generation"),
            Self::SpawnTimeWeather => crate::i18n_key!("LevelDat.group.spawn_time_weather"),
            Self::Multiplayer => crate::i18n_key!("LevelDat.group.multiplayer"),
            Self::Abilities => crate::i18n_key!("LevelDat.group.abilities"),
            Self::Advanced => crate::i18n_key!("LevelDat.group.advanced"),
            Self::Legacy => crate::i18n_key!("LevelDat.group.legacy"),
        }
    }

    pub fn description(self) -> I18nKey {
        match self {
            Self::Basic => crate::i18n_key!("LevelDat.group.basic.desc"),
            Self::Gameplay => crate::i18n_key!("LevelDat.group.gameplay.desc"),
            Self::WorldGeneration => crate::i18n_key!("LevelDat.group.world_generation.desc"),
            Self::SpawnTimeWeather => crate::i18n_key!("LevelDat.group.spawn_time_weather.desc"),
            Self::Multiplayer => crate::i18n_key!("LevelDat.group.multiplayer.desc"),
            Self::Abilities => crate::i18n_key!("LevelDat.group.abilities.desc"),
            Self::Advanced => crate::i18n_key!("LevelDat.group.advanced.desc"),
            Self::Legacy => crate::i18n_key!("LevelDat.group.legacy.desc"),
        }
    }

    pub fn collapsed_by_default(self) -> bool {
        matches!(self, Self::Advanced | Self::Legacy)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LevelDatRisk {
    Common,
    Advanced,
    Legacy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TagScope {
    Root,
    Abilities,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueFieldKind {
    String,
    Int,
    Long,
    Float,
    Version,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueFieldSpec {
    pub group: LevelDatFieldGroup,
    pub label: I18nKey,
    pub key: &'static str,
    pub description: I18nKey,
    pub scope: TagScope,
    pub kind: ValueFieldKind,
    pub risk: LevelDatRisk,
}

impl ValueFieldSpec {
    const fn new(
        group: LevelDatFieldGroup,
        label: I18nKey,
        key: &'static str,
        description: I18nKey,
        scope: TagScope,
        kind: ValueFieldKind,
        risk: LevelDatRisk,
    ) -> Self {
        Self {
            group,
            label,
            key,
            description,
            scope,
            kind,
            risk,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoolFieldSpec {
    pub group: LevelDatFieldGroup,
    pub label: I18nKey,
    pub key: &'static str,
    pub description: I18nKey,
    pub scope: TagScope,
    pub risk: LevelDatRisk,
}

impl BoolFieldSpec {
    const fn new(
        group: LevelDatFieldGroup,
        label: I18nKey,
        key: &'static str,
        description: I18nKey,
        scope: TagScope,
        risk: LevelDatRisk,
    ) -> Self {
        Self {
            group,
            label,
            key,
            description,
            scope,
            risk,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChoiceOption {
    pub label: I18nKey,
    pub value: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChoiceFieldSpec {
    pub group: LevelDatFieldGroup,
    pub label: I18nKey,
    pub key: &'static str,
    pub description: I18nKey,
    pub scope: TagScope,
    pub options: &'static [ChoiceOption],
    pub risk: LevelDatRisk,
}

impl ChoiceFieldSpec {
    const fn new(
        group: LevelDatFieldGroup,
        label: I18nKey,
        key: &'static str,
        description: I18nKey,
        scope: TagScope,
        options: &'static [ChoiceOption],
        risk: LevelDatRisk,
    ) -> Self {
        Self {
            group,
            label,
            key,
            description,
            scope,
            options,
            risk,
        }
    }
}

pub struct LevelDatFieldSection {
    pub group: LevelDatFieldGroup,
    pub values: Vec<ValueFieldSpec>,
    pub bools: Vec<BoolFieldSpec>,
    pub choices: Vec<ChoiceFieldSpec>,
}

const GAME_TYPE_OPTIONS: &[ChoiceOption] = &[
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.survival"),
        value: 0,
    },
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.creative"),
        value: 1,
    },
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.adventure"),
        value: 2,
    },
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.spectator"),
        value: 6,
    },
];

const DIFFICULTY_OPTIONS: &[ChoiceOption] = &[
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.peaceful"),
        value: 0,
    },
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.easy"),
        value: 1,
    },
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.normal"),
        value: 2,
    },
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.hard"),
        value: 3,
    },
];

const GENERATOR_OPTIONS: &[ChoiceOption] = &[
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.generator_legacy"),
        value: 0,
    },
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.generator_infinite"),
        value: 1,
    },
    ChoiceOption {
        label: crate::i18n_key!("LevelDat.option.generator_flat"),
        value: 2,
    },
];

pub const GAME_TYPE_FIELD: ChoiceFieldSpec = ChoiceFieldSpec::new(
    LevelDatFieldGroup::Gameplay,
    crate::i18n_key!("LevelDat.field.GameType"),
    "GameType",
    crate::i18n_key!("LevelDat.field.GameType.desc"),
    TagScope::Root,
    GAME_TYPE_OPTIONS,
    LevelDatRisk::Common,
);

pub const DIFFICULTY_FIELD: ChoiceFieldSpec = ChoiceFieldSpec::new(
    LevelDatFieldGroup::Gameplay,
    crate::i18n_key!("LevelDat.field.Difficulty"),
    "Difficulty",
    crate::i18n_key!("LevelDat.field.Difficulty.desc"),
    TagScope::Root,
    DIFFICULTY_OPTIONS,
    LevelDatRisk::Common,
);

pub const GENERATOR_FIELD: ChoiceFieldSpec = ChoiceFieldSpec::new(
    LevelDatFieldGroup::WorldGeneration,
    crate::i18n_key!("LevelDat.field.Generator"),
    "Generator",
    crate::i18n_key!("LevelDat.field.Generator.desc"),
    TagScope::Root,
    GENERATOR_OPTIONS,
    LevelDatRisk::Advanced,
);

const VALUE_FIELDS: &[ValueFieldSpec] = &[
    ValueFieldSpec::new(
        LevelDatFieldGroup::Basic,
        crate::i18n_key!("LevelDat.field.LevelName"),
        "LevelName",
        crate::i18n_key!("LevelDat.field.LevelName.desc"),
        TagScope::Root,
        ValueFieldKind::String,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Basic,
        crate::i18n_key!("LevelDat.field.RandomSeed"),
        "RandomSeed",
        crate::i18n_key!("LevelDat.field.RandomSeed.desc"),
        TagScope::Root,
        ValueFieldKind::Long,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        crate::i18n_key!("LevelDat.field.FlatWorldLayers"),
        "FlatWorldLayers",
        crate::i18n_key!("LevelDat.field.FlatWorldLayers.desc"),
        TagScope::Root,
        ValueFieldKind::String,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        crate::i18n_key!("LevelDat.field.BiomeOverride"),
        "BiomeOverride",
        crate::i18n_key!("LevelDat.field.BiomeOverride.desc"),
        TagScope::Root,
        ValueFieldKind::String,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        crate::i18n_key!("LevelDat.field.InventoryVersion"),
        "InventoryVersion",
        crate::i18n_key!("LevelDat.field.InventoryVersion.desc"),
        TagScope::Root,
        ValueFieldKind::String,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        crate::i18n_key!("LevelDat.field.StorageVersion"),
        "StorageVersion",
        crate::i18n_key!("LevelDat.field.StorageVersion.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        crate::i18n_key!("LevelDat.field.NetworkVersion"),
        "NetworkVersion",
        crate::i18n_key!("LevelDat.field.NetworkVersion.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        crate::i18n_key!("LevelDat.field.GeneratorVersion"),
        "GeneratorVersion",
        crate::i18n_key!("LevelDat.field.GeneratorVersion.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.SpawnX"),
        "SpawnX",
        crate::i18n_key!("LevelDat.field.SpawnX.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.SpawnY"),
        "SpawnY",
        crate::i18n_key!("LevelDat.field.SpawnY.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.SpawnZ"),
        "SpawnZ",
        crate::i18n_key!("LevelDat.field.SpawnZ.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.Time"),
        "Time",
        crate::i18n_key!("LevelDat.field.Time.desc"),
        TagScope::Root,
        ValueFieldKind::Long,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.currentTick"),
        "currentTick",
        crate::i18n_key!("LevelDat.field.currentTick.desc"),
        TagScope::Root,
        ValueFieldKind::Long,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.randomtickspeed"),
        "randomtickspeed",
        crate::i18n_key!("LevelDat.field.randomtickspeed.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.rainLevel"),
        "rainLevel",
        crate::i18n_key!("LevelDat.field.rainLevel.desc"),
        TagScope::Root,
        ValueFieldKind::Float,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.rainTime"),
        "rainTime",
        crate::i18n_key!("LevelDat.field.rainTime.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.lightningLevel"),
        "lightningLevel",
        crate::i18n_key!("LevelDat.field.lightningLevel.desc"),
        TagScope::Root,
        ValueFieldKind::Float,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        crate::i18n_key!("LevelDat.field.lightningTime"),
        "lightningTime",
        crate::i18n_key!("LevelDat.field.lightningTime.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.flySpeed"),
        "flySpeed",
        crate::i18n_key!("LevelDat.field.flySpeed.desc"),
        TagScope::Abilities,
        ValueFieldKind::Float,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.walkSpeed"),
        "walkSpeed",
        crate::i18n_key!("LevelDat.field.walkSpeed.desc"),
        TagScope::Abilities,
        ValueFieldKind::Float,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Advanced,
        crate::i18n_key!("LevelDat.field.MinimumCompatibleClientVersion"),
        "MinimumCompatibleClientVersion",
        crate::i18n_key!("LevelDat.field.MinimumCompatibleClientVersion.desc"),
        TagScope::Root,
        ValueFieldKind::Version,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Advanced,
        crate::i18n_key!("LevelDat.field.lastOpenedWithVersion"),
        "lastOpenedWithVersion",
        crate::i18n_key!("LevelDat.field.lastOpenedWithVersion.desc"),
        TagScope::Root,
        ValueFieldKind::Version,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Advanced,
        crate::i18n_key!("LevelDat.field.LastPlayed"),
        "LastPlayed",
        crate::i18n_key!("LevelDat.field.LastPlayed.desc"),
        TagScope::Root,
        ValueFieldKind::Long,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Legacy,
        crate::i18n_key!("LevelDat.field.Dimension"),
        "Dimension",
        crate::i18n_key!("LevelDat.field.Dimension.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Legacy,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Legacy,
        crate::i18n_key!("LevelDat.field.LimitedWorldOriginX"),
        "LimitedWorldOriginX",
        crate::i18n_key!("LevelDat.field.LimitedWorldOriginX.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Legacy,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Legacy,
        crate::i18n_key!("LevelDat.field.LimitedWorldOriginY"),
        "LimitedWorldOriginY",
        crate::i18n_key!("LevelDat.field.LimitedWorldOriginY.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Legacy,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Legacy,
        crate::i18n_key!("LevelDat.field.LimitedWorldOriginZ"),
        "LimitedWorldOriginZ",
        crate::i18n_key!("LevelDat.field.LimitedWorldOriginZ.desc"),
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Legacy,
    ),
];

const BOOL_FIELDS: &[BoolFieldSpec] = &[
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        crate::i18n_key!("LevelDat.field.ForceGameType"),
        "ForceGameType",
        crate::i18n_key!("LevelDat.field.ForceGameType.desc"),
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        crate::i18n_key!("LevelDat.field.commandsEnabled"),
        "commandsEnabled",
        crate::i18n_key!("LevelDat.field.commandsEnabled.desc"),
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        crate::i18n_key!("LevelDat.field.immutableWorld"),
        "immutableWorld",
        crate::i18n_key!("LevelDat.field.immutableWorld.desc"),
        TagScope::Root,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        crate::i18n_key!("LevelDat.field.spawnMobs"),
        "spawnMobs",
        crate::i18n_key!("LevelDat.field.spawnMobs.desc"),
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        crate::i18n_key!("LevelDat.field.bonusChestEnabled"),
        "bonusChestEnabled",
        crate::i18n_key!("LevelDat.field.bonusChestEnabled.desc"),
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        crate::i18n_key!("LevelDat.field.bonusChestSpawned"),
        "bonusChestSpawned",
        crate::i18n_key!("LevelDat.field.bonusChestSpawned.desc"),
        TagScope::Root,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        crate::i18n_key!("LevelDat.field.startWithMapEnabled"),
        "startWithMapEnabled",
        crate::i18n_key!("LevelDat.field.startWithMapEnabled.desc"),
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Multiplayer,
        crate::i18n_key!("LevelDat.field.MultiplayerGame"),
        "MultiplayerGame",
        crate::i18n_key!("LevelDat.field.MultiplayerGame.desc"),
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Multiplayer,
        crate::i18n_key!("LevelDat.field.texturePacksRequired"),
        "texturePacksRequired",
        crate::i18n_key!("LevelDat.field.texturePacksRequired.desc"),
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Multiplayer,
        crate::i18n_key!("LevelDat.field.ConfirmedPlatformLockedContent"),
        "ConfirmedPlatformLockedContent",
        crate::i18n_key!("LevelDat.field.ConfirmedPlatformLockedContent.desc"),
        TagScope::Root,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        crate::i18n_key!("LevelDat.field.CenterMapsToOrigin"),
        "CenterMapsToOrigin",
        crate::i18n_key!("LevelDat.field.CenterMapsToOrigin.desc"),
        TagScope::Root,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.attackmobs"),
        "attackmobs",
        crate::i18n_key!("LevelDat.field.attackmobs.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.attackplayers"),
        "attackplayers",
        crate::i18n_key!("LevelDat.field.attackplayers.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.build"),
        "build",
        crate::i18n_key!("LevelDat.field.build.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.doorsandswitches"),
        "doorsandswitches",
        crate::i18n_key!("LevelDat.field.doorsandswitches.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.flying"),
        "flying",
        crate::i18n_key!("LevelDat.field.flying.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.instabuild"),
        "instabuild",
        crate::i18n_key!("LevelDat.field.instabuild.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.invulnerable"),
        "invulnerable",
        crate::i18n_key!("LevelDat.field.invulnerable.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.lightning"),
        "lightning",
        crate::i18n_key!("LevelDat.field.lightning.desc"),
        TagScope::Abilities,
        LevelDatRisk::Legacy,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.mayfly"),
        "mayfly",
        crate::i18n_key!("LevelDat.field.mayfly.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.mine"),
        "mine",
        crate::i18n_key!("LevelDat.field.mine.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.op"),
        "op",
        crate::i18n_key!("LevelDat.field.op.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.opencontainers"),
        "opencontainers",
        crate::i18n_key!("LevelDat.field.opencontainers.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        crate::i18n_key!("LevelDat.field.teleport"),
        "teleport",
        crate::i18n_key!("LevelDat.field.teleport.desc"),
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
];

const CHOICE_FIELDS: &[ChoiceFieldSpec] = &[GAME_TYPE_FIELD, DIFFICULTY_FIELD, GENERATOR_FIELD];

pub fn default_collapsed_groups() -> Vec<LevelDatFieldGroup> {
    LevelDatFieldGroup::ORDER
        .iter()
        .copied()
        .filter(|group| group.collapsed_by_default())
        .collect()
}

pub fn form_value_fields(_document: &LevelDatDocument) -> Vec<ValueFieldSpec> {
    VALUE_FIELDS.to_vec()
}

pub fn form_sections(document: &LevelDatDocument) -> Vec<LevelDatFieldSection> {
    LevelDatFieldGroup::ORDER
        .iter()
        .copied()
        .filter_map(|group| {
            let values = VALUE_FIELDS
                .iter()
                .copied()
                .filter(|field| field.group == group && should_show_value_field(document, *field))
                .collect::<Vec<_>>();
            let bools = BOOL_FIELDS
                .iter()
                .copied()
                .filter(|field| field.group == group)
                .collect::<Vec<_>>();
            let choices = CHOICE_FIELDS
                .iter()
                .copied()
                .filter(|field| field.group == group)
                .collect::<Vec<_>>();

            (!values.is_empty() || !bools.is_empty() || !choices.is_empty()).then_some(
                LevelDatFieldSection {
                    group,
                    values,
                    bools,
                    choices,
                },
            )
        })
        .collect()
}

fn should_show_value_field(_document: &LevelDatDocument, _field: ValueFieldSpec) -> bool {
    true
}

pub fn field_exists(document: &LevelDatDocument, scope: TagScope, key: &str) -> bool {
    match scope {
        TagScope::Root => root_compound(document).is_some_and(|root| root.contains_key(key)),
        TagScope::Abilities => root_compound(document)
            .and_then(|root| root.get("abilities"))
            .and_then(|tag| match tag {
                &NbtTag::Compound(ref map) => Some(map),
                _ => None,
            })
            .is_some_and(|abilities| abilities.contains_key(key)),
    }
}

fn root_compound(document: &LevelDatDocument) -> Option<&indexmap::IndexMap<String, NbtTag>> {
    match &document.root {
        &NbtTag::Compound(ref map) => Some(map),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn legacy_fields_are_available_without_creating_tags() {
        let document = LevelDatDocument::new(10, NbtTag::Compound(IndexMap::new()));

        let sections = form_sections(&document);
        let legacy = sections
            .iter()
            .find(|section| section.group == LevelDatFieldGroup::Legacy)
            .expect("legacy section should be present");

        assert!(legacy.values.iter().any(|field| field.key == "Dimension"));
        assert!(!field_exists(&document, TagScope::Root, "Dimension"));
    }

    #[test]
    fn advanced_and_legacy_groups_start_collapsed() {
        let collapsed = default_collapsed_groups();

        assert!(collapsed.contains(&LevelDatFieldGroup::Advanced));
        assert!(collapsed.contains(&LevelDatFieldGroup::Legacy));
        assert!(!collapsed.contains(&LevelDatFieldGroup::Basic));
    }
}
