//! Aggregate exposed emitters on the worker; resource-authored colors take precedence.
use super::super::super::types::World3dLocalLight;
use super::*;

#[derive(Default)]
struct Cluster {
    position: Vec3,
    color: [f32; 3],
    energy: f32,
    range: f32,
    min: [f32; 3],
    max: [f32; 3],
}

pub(super) fn collect(
    models: &bedrock_block_model::BlockModelRepository,
    resolver: &ObjTextureResolver,
    probe: &World3dChunkProbe,
    cells: &[LightCell],
    min_y: i32,
    height: u32,
) -> Arc<[World3dLocalLight]> {
    let mut clusters = BTreeMap::<[i32; 4], Cluster>::new();
    for (&sub_y, entry) in &probe.subchunks {
        let SubChunkFormat::Paletted { storages, .. } = &entry.subchunk.format else {
            continue;
        };
        for storage in storages {
            let palette = storage
                .states
                .iter()
                .map(|state| {
                    let emission = models
                        .light_properties(&bedrock_block_model::BlockStateQuery::from_world_state(
                            state,
                        ))
                        .emission;
                    (emission > 0).then(|| {
                        (
                            emission,
                            resolver
                                .local_light_for(&state.name)
                                .unwrap_or_else(|| default_color(&state.name)),
                        )
                    })
                })
                .collect::<Vec<_>>();
            if palette.iter().all(Option::is_none) {
                continue;
            }
            for i in 0..4096u16 {
                let (x, y, z) = ((i % 16) as u8, (i / 16 % 16) as u8, (i / 256) as u8);
                let Some(&(emission, definition)) = storage
                    .palette_index_at(x, y, z)
                    .and_then(|i| palette.get(usize::from(i)))
                    .and_then(Option::as_ref)
                else {
                    continue;
                };
                let y = i32::from(sub_y) * 16 + i32::from(y);
                if y < min_y || y >= min_y + height as i32 {
                    continue;
                }
                let at = (((usize::from(z) + 16) * height as usize + (y - min_y) as usize) * 48
                    + usize::from(x)
                    + 16) as isize;
                let exposed = [1, -1, 48, -48, 48 * height as isize, -48 * height as isize]
                    .into_iter()
                    .filter(|&offset| {
                        !(offset == 48 && y == min_y + height as i32 - 1
                            || offset == -48 && y == min_y)
                    })
                    .any(|offset| {
                        let cell = cells[(at + offset) as usize];
                        cell.attenuation() < 15 && cell.emission() < emission
                    });
                if !exposed {
                    continue;
                }
                let key = [
                    i32::from(x) / 2,
                    y.div_euclid(2),
                    i32::from(z) / 2,
                    i32::from(definition.is_point),
                ];
                add(
                    clusters.entry(key).or_default(),
                    Vec3::new(f32::from(x) + 0.5, y as f32 + 0.5, f32::from(z) + 0.5),
                    emission,
                    definition.color,
                );
            }
        }
    }
    clusters
        .into_iter()
        .map(|(key, cluster)| {
            let color = cluster.color.map(|value| value / cluster.energy);
            let position = cluster.position / cluster.energy;
            let intensity = 16. * cluster.energy;
            let coordinates = [position.x, position.y, position.z];
            let radius = (0..3)
                .map(|axis| {
                    (coordinates[axis] - cluster.min[axis])
                        .max(cluster.max[axis] - coordinates[axis])
                        .powi(2)
                })
                .sum::<f32>()
                .sqrt();
            let light = gpui_3d::Light::Spherical {
                position,
                color,
                intensity,
                range: cluster.range,
                radius,
                diffuse_only: key[3] == 0,
            };
            World3dLocalLight { key, light }
        })
        .collect::<Vec<_>>()
        .into()
}

fn add(cluster: &mut Cluster, position: Vec3, emission: u8, color: [u8; 3]) {
    for (axis, coordinate) in [position.x, position.y, position.z].into_iter().enumerate() {
        cluster.min[axis] = if cluster.energy == 0. {
            coordinate - 0.5
        } else {
            cluster.min[axis].min(coordinate - 0.5)
        };
        cluster.max[axis] = if cluster.energy == 0. {
            coordinate + 0.5
        } else {
            cluster.max[axis].max(coordinate + 0.5)
        };
    }
    let energy = (f32::from(emission) / 15.).powi(2);
    cluster.position = cluster.position + position * energy;
    let linear_color =
        super::super::super::rendering::linear_rgb(color.map(|channel| f32::from(channel) / 255.));
    for (out, linear) in cluster.color.iter_mut().zip(linear_color) {
        *out += linear * energy;
    }
    cluster.energy += energy;
    cluster.range = cluster.range.max(f32::from(emission) + 1.75);
}

/// Editor fallback colors; overrides are read from the active resource pack.
fn default_color(block: &str) -> bedrock_block_model::LocalLightDefinition {
    let color = match block {
        "minecraft:torch" | "minecraft:lantern" => [239, 227, 157],
        "minecraft:soul_torch" | "minecraft:soul_lantern" | "minecraft:soul_campfire" => {
            [94, 225, 255]
        }
        "minecraft:redstone_torch" | "minecraft:lit_redstone_lamp" => [255, 70, 35],
        "minecraft:lava" | "minecraft:flowing_lava" | "minecraft:fire" | "minecraft:campfire" => {
            [255, 151, 62]
        }
        _ => [255; 3],
    };
    bedrock_block_model::LocalLightDefinition {
        color,
        is_point: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clustering_preserves_linear_color_and_energy_weighted_position() {
        let mut cluster = Cluster::default();
        add(&mut cluster, Vec3::ZERO, 15, [255, 0, 0]);
        add(&mut cluster, Vec3::new(2., 0., 0.), 15, [0, 0, 255]);
        assert_eq!(cluster.energy, 2.);
        assert_eq!(cluster.position / cluster.energy, Vec3::new(1., 0., 0.));
        assert_eq!(
            cluster.color.map(|value| value / cluster.energy),
            [0.5, 0., 0.5]
        );
    }
}
