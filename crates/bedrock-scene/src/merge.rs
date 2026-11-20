use super::*;

pub(super) struct FaceBudget {
    max_faces: usize,
    emitted_faces: usize,
    pub(super) omitted_faces: usize,
}

impl FaceBudget {
    pub(super) const fn new(max_faces: usize) -> Self {
        Self {
            max_faces,
            emitted_faces: 0,
            omitted_faces: 0,
        }
    }

    pub(super) fn push_or_omit(&mut self, faces: &mut Vec<Face>, face: Face) {
        if self.emitted_faces < self.max_faces {
            faces.push(face);
            self.emitted_faces = self.emitted_faces.saturating_add(1);
        } else {
            self.omitted_faces = self.omitted_faces.saturating_add(1);
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct FacePlaneKey {
    axis: u8,
    normal_positive: bool,
    plane: i32,
    /// Distance the merged plane sits below its integer boundary. Fluid
    /// surfaces render at level/9 (source 8/9), so their top faces carry
    /// `1 - surface` here; cells with equal surfaces still merge into one
    /// rectangle. Zero for every block-boundary face.
    plane_drop_bits: u32,
    color: [u32; 4],
    shade_bits: u32,
    material: MaterialName,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct FaceCell {
    u: i32,
    v: i32,
}

#[derive(Default)]
pub(super) struct FaceMerger {
    planes: BTreeMap<FacePlaneKey, BTreeSet<FaceCell>>,
}

impl FaceMerger {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn merge(mut self, other: Self) -> Self {
        for (key, cells) in other.planes {
            self.planes.entry(key).or_default().extend(cells);
        }
        self
    }

    pub(super) fn push(
        &mut self,
        block: BlockKey,
        face: FaceDefinition,
        color: [f32; 4],
        material: MaterialName,
        plane_drop: f32,
    ) {
        let Some(axis) = face.axis() else {
            return;
        };
        let normal_positive = face.normal[axis] > 0;
        let plane = match axis {
            0 => block.x + i32::from(normal_positive),
            1 => block.y + i32::from(normal_positive),
            2 => block.z + i32::from(normal_positive),
            _ => return,
        };
        let (u, v) = match axis {
            0 => (block.z, block.y),
            1 => (block.x, block.z),
            2 => (block.x, block.y),
            _ => return,
        };
        let material = face_material_name(material, face.normal);
        let key = FacePlaneKey {
            axis: axis as u8,
            normal_positive,
            plane,
            plane_drop_bits: plane_drop.to_bits(),
            color: color.map(f32::to_bits),
            shade_bits: face.shade.to_bits(),
            material,
        };
        self.planes
            .entry(key)
            .or_default()
            .insert(FaceCell { u, v });
    }

    pub(super) fn emit_into(self, faces: &mut Vec<Face>, budget: &mut FaceBudget) {
        for (key, cells) in self.planes {
            for rectangle in merge_face_cells(cells) {
                budget.push_or_omit(faces, rectangle.into_face(key.clone()));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MergedFaceRect {
    u0: i32,
    v0: i32,
    u1: i32,
    v1: i32,
}

impl MergedFaceRect {
    pub(super) fn into_face(self, key: FacePlaneKey) -> Face {
        let color = key.color.map(f32::from_bits);
        let shade = f32::from_bits(key.shade_bits);
        let p = f64::from(key.plane) - f64::from(f32::from_bits(key.plane_drop_bits));
        let u0 = f64::from(self.u0);
        let u1 = f64::from(self.u1);
        let v0 = f64::from(self.v0);
        let v1 = f64::from(self.v1);
        let corners = match (key.axis, key.normal_positive) {
            (0, true) => [[p, v0, u0], [p, v0, u1], [p, v1, u1], [p, v1, u0]],
            (0, false) => [[p, v0, u1], [p, v0, u0], [p, v1, u0], [p, v1, u1]],
            (1, true) => [[u0, p, v0], [u1, p, v0], [u1, p, v1], [u0, p, v1]],
            (1, false) => [[u0, p, v1], [u1, p, v1], [u1, p, v0], [u0, p, v0]],
            (2, true) => [[u1, v0, p], [u0, v0, p], [u0, v1, p], [u1, v1, p]],
            (2, false) => [[u0, v0, p], [u1, v0, p], [u1, v1, p], [u0, v1, p]],
            _ => [[0.0; 3]; 4],
        };
        Face {
            corners,
            color,
            shade,
            material: key.material,
            normal: match (key.axis, key.normal_positive) {
                (0, true) => [1, 0, 0],
                (0, false) => [-1, 0, 0],
                (1, true) => [0, 1, 0],
                (1, false) => [0, -1, 0],
                (2, true) => [0, 0, 1],
                (2, false) => [0, 0, -1],
                _ => [0, 0, 0],
            },
            uv: None,
        }
    }
}

pub(super) fn merge_face_cells(mut cells: BTreeSet<FaceCell>) -> Vec<MergedFaceRect> {
    let mut rectangles = Vec::new();
    while let Some(start) = cells.iter().next().copied() {
        let mut u1 = start.u + 1;
        while cells.contains(&FaceCell { u: u1, v: start.v }) {
            u1 += 1;
        }

        let mut v1 = start.v + 1;
        'rows: loop {
            for u in start.u..u1 {
                if !cells.contains(&FaceCell { u, v: v1 }) {
                    break 'rows;
                }
            }
            v1 += 1;
        }

        for v in start.v..v1 {
            for u in start.u..u1 {
                cells.remove(&FaceCell { u, v });
            }
        }
        rectangles.push(MergedFaceRect {
            u0: start.u,
            v0: start.v,
            u1,
            v1,
        });
    }
    rectangles
}
