//! Public Minecraft Bedrock world creation APIs.

use super::{BedrockWorld, BedrockWorldOpenOptions, WorldFormatHint};
use crate::database::create_bedrock_leveldb;
use crate::error::{BedrockWorldError, Result};
use crate::level::{LevelDatDocument, write_level_dat_document};
use crate::nbt::NbtTag;
use indexmap::IndexMap;
use std::fs;
use std::path::Path;

/// Spawn point stored in a newly-created Minecraft Bedrock world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BedrockWorldSpawn {
    /// Absolute block X coordinate.
    pub x: i32,
    /// Absolute block Y coordinate.
    pub y: i32,
    /// Absolute block Z coordinate.
    pub z: i32,
}

impl Default for BedrockWorldSpawn {
    fn default() -> Self {
        Self { x: 0, y: 64, z: 0 }
    }
}

/// Game mode written to the `GameType` field of a newly-created Bedrock world.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BedrockGameMode {
    /// Survival mode.
    #[default]
    Survival,
    /// Creative mode.
    Creative,
    /// Adventure mode.
    Adventure,
}

impl BedrockGameMode {
    const fn value(self) -> i32 {
        match self {
            Self::Survival => 0,
            Self::Creative => 1,
            Self::Adventure => 2,
        }
    }
}

/// Difficulty written to a newly-created Minecraft Bedrock world.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BedrockDifficulty {
    /// Peaceful difficulty.
    Peaceful,
    /// Easy difficulty.
    Easy,
    /// Normal difficulty.
    #[default]
    Normal,
    /// Hard difficulty.
    Hard,
}

impl BedrockDifficulty {
    const fn value(self) -> i32 {
        match self {
            Self::Peaceful => 0,
            Self::Easy => 1,
            Self::Normal => 2,
            Self::Hard => 3,
        }
    }
}

/// Metadata and gameplay defaults used to create a new Minecraft Bedrock world folder.
#[derive(Debug, Clone)]
pub struct BedrockWorldCreateOptions {
    /// User-visible map name stored in `LevelName` and `levelname.txt`.
    pub level_name: String,
    /// Authoritative world generation seed stored in `RandomSeed`.
    pub seed: i64,
    /// Initial player spawn point.
    pub spawn: BedrockWorldSpawn,
    /// Target Bedrock network version. `None` omits `NetworkVersion` from the new `level.dat`.
    pub network_version: Option<u32>,
    /// Bedrock storage version stored in `StorageVersion`.
    pub storage_version: i32,
    /// Header version for the generated `level.dat` document.
    pub level_dat_version: u32,
    /// Initial game mode.
    pub game_mode: BedrockGameMode,
    /// Initial game difficulty.
    pub difficulty: BedrockDifficulty,
    /// Whether commands are enabled by default.
    pub commands_enabled: bool,
    /// Whether daylight cycling is enabled by default.
    pub daylight_cycle: bool,
    /// Optional five-component `lastOpenedWithVersion` value.
    pub last_opened_with_version: Option<[i32; 5]>,
}

impl BedrockWorldCreateOptions {
    /// Creates standard modern-world options with a caller-owned map name and seed.
    #[must_use]
    pub fn new(level_name: impl Into<String>, seed: i64) -> Self {
        Self {
            level_name: level_name.into(),
            seed,
            spawn: BedrockWorldSpawn::default(),
            network_version: None,
            storage_version: 9,
            level_dat_version: 10,
            game_mode: BedrockGameMode::Survival,
            difficulty: BedrockDifficulty::Normal,
            commands_enabled: true,
            daylight_cycle: true,
            last_opened_with_version: None,
        }
    }
}

impl BedrockWorld {
    /// Creates a new writable Minecraft Bedrock LevelDB world and opens it.
    ///
    /// The destination must be missing or an empty directory. LevelDB creation,
    /// `level.dat`, and `levelname.txt` are all owned by `bedrock-world`; callers
    /// do not need to depend on `bedrock-leveldb` directly.
    pub fn create_blocking(
        path: impl AsRef<Path>,
        options: BedrockWorldCreateOptions,
    ) -> Result<Self> {
        let path = path.as_ref();
        validate_create_options(path, &options)?;
        let root_existed = path.exists();
        fs::create_dir_all(path)?;

        let create_result = (|| {
            create_bedrock_leveldb(path.join("db"))?;
            let level_dat = build_level_dat(&options)?;
            write_level_dat_document(&path.join("level.dat"), &level_dat)?;
            fs::write(path.join("levelname.txt"), format!("{}\n", options.level_name))?;
            Self::open_blocking(
                path,
                BedrockWorldOpenOptions {
                    read_only: false,
                    format: WorldFormatHint::LevelDb,
                },
            )
        })();

        if create_result.is_err() && !root_existed {
            let _ = fs::remove_dir_all(path);
        }
        create_result
    }

    /// Async wrapper for [`Self::create_blocking`].
    #[cfg(feature = "async")]
    pub async fn create(
        path: impl AsRef<Path>,
        options: BedrockWorldCreateOptions,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::create_blocking(path, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }
}

fn validate_create_options(path: &Path, options: &BedrockWorldCreateOptions) -> Result<()> {
    if options.level_name.trim().is_empty() {
        return Err(BedrockWorldError::Validation(
            "Bedrock world level_name must not be empty".to_string(),
        ));
    }
    if let Some(network_version) = options.network_version {
        i32::try_from(network_version).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "Bedrock network version {network_version} exceeds level.dat Int range"
            ))
        })?;
    }
    if path.exists()
        && fs::read_dir(path)?
            .next()
            .transpose()?
            .is_some()
    {
        return Err(BedrockWorldError::Validation(format!(
            "refusing to create Bedrock world in non-empty directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn build_level_dat(options: &BedrockWorldCreateOptions) -> Result<LevelDatDocument> {
    let mut root = IndexMap::from([
        (
            "LevelName".to_string(),
            NbtTag::String(options.level_name.clone()),
        ),
        ("RandomSeed".to_string(), NbtTag::Long(options.seed)),
        ("SpawnX".to_string(), NbtTag::Int(options.spawn.x)),
        ("SpawnY".to_string(), NbtTag::Int(options.spawn.y)),
        ("SpawnZ".to_string(), NbtTag::Int(options.spawn.z)),
        (
            "LimitedWorldOriginX".to_string(),
            NbtTag::Int(options.spawn.x),
        ),
        (
            "LimitedWorldOriginY".to_string(),
            NbtTag::Int(options.spawn.y),
        ),
        (
            "LimitedWorldOriginZ".to_string(),
            NbtTag::Int(options.spawn.z),
        ),
        (
            "StorageVersion".to_string(),
            NbtTag::Int(options.storage_version),
        ),
        ("GameType".to_string(), NbtTag::Int(options.game_mode.value())),
        (
            "Difficulty".to_string(),
            NbtTag::Int(options.difficulty.value()),
        ),
        (
            "commandsEnabled".to_string(),
            NbtTag::Byte(if options.commands_enabled { 1 } else { 0 }),
        ),
        (
            "dodaylightcycle".to_string(),
            NbtTag::Byte(if options.daylight_cycle { 1 } else { 0 }),
        ),
        ("Time".to_string(), NbtTag::Long(0)),
        ("currentTick".to_string(), NbtTag::Long(0)),
        ("rainLevel".to_string(), NbtTag::Float(0.0)),
        ("rainTime".to_string(), NbtTag::Int(0)),
        ("lightningLevel".to_string(), NbtTag::Float(0.0)),
        ("lightningTime".to_string(), NbtTag::Int(0)),
    ]);

    if let Some(network_version) = options.network_version {
        root.insert(
            "NetworkVersion".to_string(),
            NbtTag::Int(i32::try_from(network_version).map_err(|_| {
                BedrockWorldError::Validation(format!(
                    "Bedrock network version {network_version} exceeds level.dat Int range"
                ))
            })?),
        );
    }
    if let Some(version) = options.last_opened_with_version {
        root.insert(
            "lastOpenedWithVersion".to_string(),
            NbtTag::List(version.into_iter().map(NbtTag::Int).collect()),
        );
    }

    Ok(LevelDatDocument::new(
        options.level_dat_version,
        NbtTag::Compound(root),
    ))
}
