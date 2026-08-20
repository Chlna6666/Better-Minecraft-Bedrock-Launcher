use std::collections::BTreeMap;

use crate::geometry::BlockGeometry;
use crate::material::{BlockComponents, BlockFace, MaterialInstance, TextureReference, TextureSet};
use crate::pack::{BlockDefinition, BlockModelRepository};
use crate::permutation::ConditionEvaluation;
use crate::state::BlockStateQuery;
use crate::texture::TerrainTexture;

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedBlockModel {
    pub block_name: String,
    pub geometry_identifier: Option<String>,
    pub geometry: Option<BlockGeometry>,
    pub materials: BTreeMap<String, ResolvedMaterialInstance>,
    pub face_textures: BTreeMap<BlockFace, ResolvedTexture>,
    pub transformation: Option<crate::material::BlockTransformation>,
    pub matched_permutations: Vec<String>,
    pub warnings: Vec<ModelWarning>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedMaterialInstance {
    pub slot: String,
    pub texture_key: Option<String>,
    pub texture_path: Option<String>,
    pub render_method: Option<String>,
    pub tint_method: Option<String>,
    pub ambient_occlusion: Option<bool>,
    pub face_dimming: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedTexture {
    pub key: String,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelWarning {
    MissingBlockDefinition(String),
    MissingGeometry(String),
    UnsupportedPermutationCondition {
        block: String,
        condition: String,
        reason: String,
    },
}

#[must_use]
pub fn resolve_block(
    repository: &BlockModelRepository,
    state: &BlockStateQuery,
) -> ResolvedBlockModel {
    let Some(block) = repository.blocks.get(&state.name) else {
        return ResolvedBlockModel {
            block_name: state.name.clone(),
            geometry_identifier: None,
            geometry: None,
            materials: BTreeMap::new(),
            face_textures: BTreeMap::new(),
            transformation: None,
            matched_permutations: Vec::new(),
            warnings: vec![ModelWarning::MissingBlockDefinition(state.name.clone())],
        };
    };

    let (components, matched_permutations, mut warnings) = resolved_components(block, state);
    let geometry_identifier = components.geometry.clone();
    let geometry = geometry_identifier
        .as_deref()
        .and_then(|identifier| repository.geometry(identifier).cloned());

    if let Some(identifier) = &geometry_identifier
        && geometry.is_none()
    {
        warnings.push(ModelWarning::MissingGeometry(identifier.clone()));
    }

    let face_textures = resolve_face_textures(repository, components.textures.as_ref());
    let materials = resolve_materials(repository, &components, &face_textures);

    ResolvedBlockModel {
        block_name: block.identifier.clone(),
        geometry_identifier,
        geometry,
        materials,
        face_textures,
        transformation: components.transformation.clone(),
        matched_permutations,
        warnings,
    }
}

fn resolved_components(
    block: &BlockDefinition,
    state: &BlockStateQuery,
) -> (BlockComponents, Vec<String>, Vec<ModelWarning>) {
    let mut components = block.components.clone();
    let mut matched_permutations = Vec::new();
    let mut warnings = Vec::new();

    for permutation in &block.permutations {
        match permutation.matches(state) {
            ConditionEvaluation::Matched => {
                matched_permutations.push(permutation.condition.clone());
                components.merge_from(permutation.components.clone());
            }
            ConditionEvaluation::NotMatched => {}
            ConditionEvaluation::Unsupported(reason) => {
                warnings.push(ModelWarning::UnsupportedPermutationCondition {
                    block: block.identifier.clone(),
                    condition: permutation.condition.clone(),
                    reason,
                });
            }
        }
    }

    (components, matched_permutations, warnings)
}

fn resolve_face_textures(
    repository: &BlockModelRepository,
    textures: Option<&TextureSet>,
) -> BTreeMap<BlockFace, ResolvedTexture> {
    let mut face_textures = BTreeMap::new();
    let Some(textures) = textures else {
        return face_textures;
    };

    for face in [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
    ] {
        if let Some(texture) = textures.texture_for_face(face) {
            face_textures.insert(face, resolved_texture(repository, texture));
        }
    }

    face_textures
}

fn resolve_materials(
    repository: &BlockModelRepository,
    components: &BlockComponents,
    face_textures: &BTreeMap<BlockFace, ResolvedTexture>,
) -> BTreeMap<String, ResolvedMaterialInstance> {
    let mut materials = BTreeMap::new();

    for (slot, material) in &components.material_instances {
        materials.insert(slot.clone(), resolve_material(repository, slot, material));
    }

    if materials.is_empty()
        && let Some(texture) = face_textures
            .get(&BlockFace::All)
            .or_else(|| face_textures.get(&BlockFace::North))
            .or_else(|| face_textures.values().next())
    {
        materials.insert(
            "*".to_owned(),
            ResolvedMaterialInstance {
                slot: "*".to_owned(),
                texture_key: Some(texture.key.clone()),
                texture_path: texture.path.clone(),
                render_method: None,
                tint_method: None,
                ambient_occlusion: None,
                face_dimming: None,
            },
        );
    }

    materials
}

fn resolve_material(
    repository: &BlockModelRepository,
    slot: &str,
    material: &MaterialInstance,
) -> ResolvedMaterialInstance {
    let resolved_texture = material
        .texture
        .as_ref()
        .map(|texture| resolved_texture(repository, texture));

    ResolvedMaterialInstance {
        slot: slot.to_owned(),
        texture_key: resolved_texture.as_ref().map(|texture| texture.key.clone()),
        texture_path: resolved_texture.and_then(|texture| texture.path),
        render_method: material.render_method.clone(),
        tint_method: material.tint_method.clone(),
        ambient_occlusion: material.ambient_occlusion,
        face_dimming: material.face_dimming,
    }
}

fn resolved_texture(
    repository: &BlockModelRepository,
    texture: &TextureReference,
) -> ResolvedTexture {
    let terrain_texture = repository.terrain_textures.resolve(&texture.key);
    resolved_terrain_texture(terrain_texture)
}

fn resolved_terrain_texture(texture: TerrainTexture) -> ResolvedTexture {
    let path = texture.primary_path().map(ToOwned::to_owned);
    ResolvedTexture {
        key: texture.key,
        path,
    }
}
