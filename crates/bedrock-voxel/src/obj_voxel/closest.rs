use super::{dot, subtract};

/// Barycentric weights at the closest triangle point and its squared distance.
pub(super) fn sample(vertices: [[f32; 3]; 3], point: [f32; 3]) -> ([f32; 3], f32) {
    let weights = weights(vertices, point);
    let closest = [0, 1, 2].map(|axis| {
        (0..3)
            .map(|index| vertices[index][axis] * weights[index])
            .sum()
    });
    let delta = subtract(closest, point);
    (weights, dot(delta, delta))
}

fn weights([a, b, c]: [[f32; 3]; 3], point: [f32; 3]) -> [f32; 3] {
    let ab = subtract(b, a);
    let ac = subtract(c, a);
    let ap = subtract(point, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return [1.0, 0.0, 0.0];
    }
    let bp = subtract(point, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return [0.0, 1.0, 0.0];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return [1.0 - v, v, 0.0];
    }
    let cp = subtract(point, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return [0.0, 0.0, 1.0];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return [1.0 - w, 0.0, w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 >= d3 && d5 >= d6 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0.0, 1.0 - w, w];
    }
    let denominator = va + vb + vc;
    let v = vb / denominator;
    let w = vc / denominator;
    [1.0 - v - w, v, w]
}

#[cfg(test)]
mod tests {
    use super::sample;

    #[test]
    fn outside_projection_uses_the_closest_edge_of_a_nonuniform_triangle() {
        let (weights, distance) = sample(
            [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [1.5, 1.0, 0.0],
        );
        for (actual, expected) in weights.into_iter().zip([0.0, 0.6, 0.4]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        assert!((distance - 0.45).abs() < 1e-6);
    }

    #[test]
    fn sample_covers_vertices_edges_and_the_triangle_interior() {
        let triangle = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        for (point, expected) in [
            ([-1.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
            ([2.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 2.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.5, -1.0, 0.0], [0.5, 0.5, 0.0]),
            ([-1.0, 0.5, 0.0], [0.5, 0.0, 0.5]),
            ([0.25, 0.25, 2.0], [0.5, 0.25, 0.25]),
        ] {
            let (actual, _) = sample(triangle, point);
            assert_eq!(actual, expected);
        }
    }
}
