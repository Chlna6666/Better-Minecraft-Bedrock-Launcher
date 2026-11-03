//! Fixed input coverage for opt-in, repeatable local-world rendering measurements.
use super::*;
use std::hash::{Hash, Hasher};
use std::path::Path;

pub(super) struct Coverage {
    columns: BTreeSet<(i32, i32)>,
    fingerprint: u64,
}

impl Coverage {
    pub(super) fn requested(
        path: &Path,
        spawn: [f64; 3],
        radius: i32,
        dimension: Dimension,
    ) -> Option<Self> {
        if std::env::var("BMCBL_WORLD3D_TEST_COMPLETE").as_deref() != Ok("1") {
            return None;
        }
        let world = World::open(
            path,
            OpenOptions {
                read_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        let center = [
            (spawn[0] / 16.).floor() as i32,
            (spawn[2] / 16.).floor() as i32,
        ];
        let mut columns = BTreeSet::new();
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for pos in world.render_chunk_positions(Default::default()).unwrap() {
            if pos.dimension != dimension
                || !super::super::chunk_range::contains(
                    i64::from(pos.x) - i64::from(center[0]),
                    i64::from(pos.z) - i64::from(center[1]),
                    radius,
                )
            {
                continue;
            }
            columns.insert((pos.x, pos.z));
        }
        for &(x, z) in &columns {
            (x, z).hash(&mut hash);
            let chunk = world.chunk(ChunkPos { x, z, dimension }).unwrap();
            for record in chunk.records {
                format!("{:?}", record.key).hash(&mut hash);
                record.value.hash(&mut hash);
            }
        }
        assert!(
            !columns.is_empty(),
            "fixed benchmark has no saved render columns"
        );
        Some(Self {
            columns,
            fingerprint: hash.finish(),
        })
    }

    pub(super) fn is_ready(&self, world: &SnapshotWorld) -> bool {
        self.columns.is_subset(&world.completed_columns)
    }

    pub(super) fn finish(&self, world: &mut SnapshotWorld) {
        let absent = self
            .columns
            .difference(&world.completed_columns)
            .copied()
            .collect::<Vec<_>>();
        assert!(
            absent.is_empty(),
            "fixed benchmark did not complete columns: {absent:?}"
        );
        world
            .meshes
            .retain(|&(x, z, _), _| self.columns.contains(&(x, z)));
        world.probes.retain(|key, _| self.columns.contains(key));
        world.lights.retain(|key, _| self.columns.contains(key));
        eprintln!(
            "world3d fixed_input columns={} chunk_fingerprint={:016x} sections={}",
            self.columns.len(),
            self.fingerprint,
            world.meshes.len()
        );
    }
}
