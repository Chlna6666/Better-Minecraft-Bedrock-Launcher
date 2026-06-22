use crate::core::minecraft::nbt::{LevelDatDocument, NbtTag};

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

    pub fn title(self) -> &'static str {
        match self {
            Self::Basic => "基础信息",
            Self::Gameplay => "玩法规则",
            Self::WorldGeneration => "世界生成",
            Self::SpawnTimeWeather => "出生点 / 时间 / 天气",
            Self::Multiplayer => "多人 / 资源包",
            Self::Abilities => "玩家能力",
            Self::Advanced => "高级 / 版本",
            Self::Legacy => "旧版本字段",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Basic => "地图名称、种子和常用标识。",
            Self::Gameplay => "游戏模式、难度、作弊和基础世界开关。",
            Self::WorldGeneration => "生成器、超平坦层和存储相关字段。",
            Self::SpawnTimeWeather => "出生坐标、世界时间、随机刻和天气状态。",
            Self::Multiplayer => "多人游戏、广播和资源包要求。",
            Self::Abilities => "玩家权限和能力，错误修改可能影响正常游玩。",
            Self::Advanced => "客户端兼容、网络版本等不建议频繁修改的字段。",
            Self::Legacy => "旧版世界可能出现的字段；不存在时不会自动写入。",
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
    pub label: &'static str,
    pub key: &'static str,
    pub description: &'static str,
    pub scope: TagScope,
    pub kind: ValueFieldKind,
    pub risk: LevelDatRisk,
}

impl ValueFieldSpec {
    const fn new(
        group: LevelDatFieldGroup,
        label: &'static str,
        key: &'static str,
        description: &'static str,
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
    pub label: &'static str,
    pub key: &'static str,
    pub description: &'static str,
    pub scope: TagScope,
    pub risk: LevelDatRisk,
}

impl BoolFieldSpec {
    const fn new(
        group: LevelDatFieldGroup,
        label: &'static str,
        key: &'static str,
        description: &'static str,
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
    pub label: &'static str,
    pub value: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChoiceFieldSpec {
    pub group: LevelDatFieldGroup,
    pub label: &'static str,
    pub key: &'static str,
    pub description: &'static str,
    pub scope: TagScope,
    pub options: &'static [ChoiceOption],
    pub risk: LevelDatRisk,
}

impl ChoiceFieldSpec {
    const fn new(
        group: LevelDatFieldGroup,
        label: &'static str,
        key: &'static str,
        description: &'static str,
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
        label: "生存",
        value: 0,
    },
    ChoiceOption {
        label: "创造",
        value: 1,
    },
    ChoiceOption {
        label: "冒险",
        value: 2,
    },
    ChoiceOption {
        label: "观察者",
        value: 6,
    },
];

const DIFFICULTY_OPTIONS: &[ChoiceOption] = &[
    ChoiceOption {
        label: "和平",
        value: 0,
    },
    ChoiceOption {
        label: "简单",
        value: 1,
    },
    ChoiceOption {
        label: "普通",
        value: 2,
    },
    ChoiceOption {
        label: "困难",
        value: 3,
    },
];

const GENERATOR_OPTIONS: &[ChoiceOption] = &[
    ChoiceOption {
        label: "旧版",
        value: 0,
    },
    ChoiceOption {
        label: "无限",
        value: 1,
    },
    ChoiceOption {
        label: "平坦",
        value: 2,
    },
];

pub const GAME_TYPE_FIELD: ChoiceFieldSpec = ChoiceFieldSpec::new(
    LevelDatFieldGroup::Gameplay,
    "默认游戏模式",
    "GameType",
    "玩家进入世界时使用的默认模式。",
    TagScope::Root,
    GAME_TYPE_OPTIONS,
    LevelDatRisk::Common,
);

pub const DIFFICULTY_FIELD: ChoiceFieldSpec = ChoiceFieldSpec::new(
    LevelDatFieldGroup::Gameplay,
    "世界难度",
    "Difficulty",
    "控制敌对生物强度和饥饿等玩法规则。",
    TagScope::Root,
    DIFFICULTY_OPTIONS,
    LevelDatRisk::Common,
);

pub const GENERATOR_FIELD: ChoiceFieldSpec = ChoiceFieldSpec::new(
    LevelDatFieldGroup::WorldGeneration,
    "世界生成类型",
    "Generator",
    "旧版、无限或超平坦生成器；旧世界可能使用旧版值。",
    TagScope::Root,
    GENERATOR_OPTIONS,
    LevelDatRisk::Advanced,
);

const VALUE_FIELDS: &[ValueFieldSpec] = &[
    ValueFieldSpec::new(
        LevelDatFieldGroup::Basic,
        "地图显示名称",
        "LevelName",
        "世界列表中显示的名称。",
        TagScope::Root,
        ValueFieldKind::String,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Basic,
        "世界种子",
        "RandomSeed",
        "影响地形生成的整数种子。",
        TagScope::Root,
        ValueFieldKind::Long,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        "超平坦层配置",
        "FlatWorldLayers",
        "超平坦世界的区块层 JSON 字符串。",
        TagScope::Root,
        ValueFieldKind::String,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        "生物群系覆盖",
        "BiomeOverride",
        "强制世界使用的生物群系标识；空值表示不覆盖。",
        TagScope::Root,
        ValueFieldKind::String,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        "物品栏数据版本",
        "InventoryVersion",
        "物品栏序列化使用的内部版本。",
        TagScope::Root,
        ValueFieldKind::String,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        "存储版本",
        "StorageVersion",
        "世界数据库使用的存储版本；通常不需要手动修改。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        "网络版本",
        "NetworkVersion",
        "最后保存该世界的游戏网络协议版本。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        "地形生成版本",
        "GeneratorVersion",
        "生成器内部版本，旧地图可能缺失。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "出生点 X",
        "SpawnX",
        "玩家首次进入世界或重生使用的 X 坐标。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "出生点 Y",
        "SpawnY",
        "玩家首次进入世界或重生使用的 Y 坐标。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "出生点 Z",
        "SpawnZ",
        "玩家首次进入世界或重生使用的 Z 坐标。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "世界时间",
        "Time",
        "昼夜循环使用的世界时间。",
        TagScope::Root,
        ValueFieldKind::Long,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "当前游戏刻",
        "currentTick",
        "世界运行的总 tick 数。",
        TagScope::Root,
        ValueFieldKind::Long,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "随机刻速度",
        "randomtickspeed",
        "影响作物生长、方块更新等随机刻频率。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Common,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "降雨强度",
        "rainLevel",
        "当前降雨强度，通常在 0 到 1 之间。",
        TagScope::Root,
        ValueFieldKind::Float,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "剩余降雨时间",
        "rainTime",
        "当前天气状态持续的 tick 数。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "闪电强度",
        "lightningLevel",
        "当前雷暴强度，通常在 0 到 1 之间。",
        TagScope::Root,
        ValueFieldKind::Float,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::SpawnTimeWeather,
        "剩余闪电时间",
        "lightningTime",
        "当前雷暴状态持续的 tick 数。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "飞行速度",
        "flySpeed",
        "玩家能力中的飞行移动速度。",
        TagScope::Abilities,
        ValueFieldKind::Float,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "步行速度",
        "walkSpeed",
        "玩家能力中的地面移动速度。",
        TagScope::Abilities,
        ValueFieldKind::Float,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Advanced,
        "最低兼容客户端版本",
        "MinimumCompatibleClientVersion",
        "低于该版本的客户端可能无法打开此世界。",
        TagScope::Root,
        ValueFieldKind::Version,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Advanced,
        "最后打开的游戏版本",
        "lastOpenedWithVersion",
        "最近一次打开该世界的客户端版本。",
        TagScope::Root,
        ValueFieldKind::Version,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Advanced,
        "最后游玩时间戳",
        "LastPlayed",
        "世界最后保存时间，通常为 Unix 时间戳。",
        TagScope::Root,
        ValueFieldKind::Long,
        LevelDatRisk::Advanced,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Legacy,
        "旧版维度编号",
        "Dimension",
        "早期世界可能使用的维度字段；不存在时不会写入。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Legacy,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Legacy,
        "有限世界原点 X",
        "LimitedWorldOriginX",
        "旧版有限世界的原点 X 坐标。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Legacy,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Legacy,
        "有限世界原点 Y",
        "LimitedWorldOriginY",
        "旧版有限世界的原点 Y 坐标。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Legacy,
    ),
    ValueFieldSpec::new(
        LevelDatFieldGroup::Legacy,
        "有限世界原点 Z",
        "LimitedWorldOriginZ",
        "旧版有限世界的原点 Z 坐标。",
        TagScope::Root,
        ValueFieldKind::Int,
        LevelDatRisk::Legacy,
    ),
];

const BOOL_FIELDS: &[BoolFieldSpec] = &[
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        "强制进入默认游戏模式",
        "ForceGameType",
        "开启后玩家会被强制切换到世界默认游戏模式。",
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        "启用命令 / 作弊",
        "commandsEnabled",
        "控制命令和作弊功能是否启用。",
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        "不可破坏世界",
        "immutableWorld",
        "通常用于模板或特殊世界，开启后会限制玩家修改。",
        TagScope::Root,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        "自然生成生物",
        "spawnMobs",
        "控制世界是否自然生成生物。",
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        "启用奖励箱",
        "bonusChestEnabled",
        "控制创建世界时的奖励箱选项。",
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        "奖励箱已经生成",
        "bonusChestSpawned",
        "记录奖励箱是否已经生成，旧世界可能缺失。",
        TagScope::Root,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Gameplay,
        "开局地图",
        "startWithMapEnabled",
        "控制新玩家进入世界时是否携带地图。",
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Multiplayer,
        "允许多人游戏",
        "MultiplayerGame",
        "控制世界是否允许多人加入。",
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Multiplayer,
        "需要玩家下载世界资源包",
        "texturePacksRequired",
        "开启后加入者必须接受该世界附带的资源包。",
        TagScope::Root,
        LevelDatRisk::Common,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Multiplayer,
        "确认平台锁定内容",
        "ConfirmedPlatformLockedContent",
        "与平台内容限制相关，旧世界可能不存在。",
        TagScope::Root,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::WorldGeneration,
        "地图居中到原点",
        "CenterMapsToOrigin",
        "旧版地图相关选项，用于地图显示中心。",
        TagScope::Root,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "允许攻击生物",
        "attackmobs",
        "玩家能力：是否允许攻击生物。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "允许攻击玩家",
        "attackplayers",
        "玩家能力：是否允许攻击其他玩家。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "允许建造",
        "build",
        "玩家能力：是否允许放置和建造。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "允许使用门和开关",
        "doorsandswitches",
        "玩家能力：是否允许操作门、拉杆、按钮等交互方块。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "当前正在飞行",
        "flying",
        "玩家能力：记录玩家当前是否处于飞行状态。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "即时建造",
        "instabuild",
        "玩家能力：通常与创造模式即时破坏/建造能力相关。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "无敌",
        "invulnerable",
        "玩家能力：是否免疫伤害。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "闪电能力",
        "lightning",
        "旧版能力字段，普通世界通常不需要修改。",
        TagScope::Abilities,
        LevelDatRisk::Legacy,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "允许飞行",
        "mayfly",
        "玩家能力：是否允许玩家进入飞行。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "允许挖掘",
        "mine",
        "玩家能力：是否允许破坏方块。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "管理员权限",
        "op",
        "玩家能力：是否拥有管理员权限。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "允许打开容器",
        "opencontainers",
        "玩家能力：是否允许打开箱子、容器等。",
        TagScope::Abilities,
        LevelDatRisk::Advanced,
    ),
    BoolFieldSpec::new(
        LevelDatFieldGroup::Abilities,
        "允许传送",
        "teleport",
        "玩家能力：是否允许传送。",
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
                NbtTag::Compound(map) => Some(map),
                _ => None,
            })
            .is_some_and(|abilities| abilities.contains_key(key)),
    }
}

fn root_compound(document: &LevelDatDocument) -> Option<&indexmap::IndexMap<String, NbtTag>> {
    match &document.root {
        NbtTag::Compound(map) => Some(map),
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
