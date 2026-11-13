//! Compact only pending, unrotated slabs; live bodies never change position.
use crate::{
    geometry::{Point, Scene},
    snow::{Flake, SnowSlab},
};

pub(crate) fn merge(a: &Flake, b: &Flake, scene: &Scene) -> Flake {
    debug_assert!(a.angle == 0.0 && b.angle == 0.0);
    let total = f64::from(a.mass) + f64::from(b.mass);
    let weight = f64::from(b.mass) / total;
    let position = Point::new(
        (f64::from(a.position.x) * (1.0 - weight) + f64::from(b.position.x) * weight) as f32,
        (f64::from(a.position.y) * (1.0 - weight) + f64::from(b.position.y) * weight) as f32,
    );
    let scale = scene.scale_at(position);
    let mut left = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    for body in [a, b] {
        let slab = body.slab.as_ref().unwrap();
        let body_scale = scene.scale_at(body.position);
        let start = body.position.x - slab.center.x * body_scale;
        left = left.min(start);
        right = right.max(start + slab.cell_width * slab.heights.len() as f32 * body_scale);
    }
    let width = (right - left).max(0.001);
    let cell_width = width / scale / 16.0;
    let mut amounts = [0.0_f64; 16];
    for body in [a, b] {
        let slab = body.slab.as_ref().unwrap();
        let body_scale = scene.scale_at(body.position);
        let start = body.position.x - slab.center.x * body_scale;
        let physical_cell = slab.cell_width * body_scale;
        let area: f64 = slab
            .heights
            .iter()
            .map(|&h| f64::from(h) * f64::from(slab.cell_width))
            .sum();
        if area == 0.0 {
            // A subnormal remainder can have rounded its height to zero.
            // Keep its mass without dividing by a degenerate profile.
            let bin = (((body.position.x - left) / width * 16.0) as usize).min(15);
            amounts[bin] += f64::from(body.mass);
            continue;
        }
        for (i, &height) in slab.heights.iter().enumerate() {
            let x0 = start + i as f32 * physical_cell;
            let x1 = x0 + physical_cell;
            let mass = f64::from(height) * f64::from(slab.cell_width) / area * f64::from(body.mass);
            for (j, amount) in amounts.iter_mut().enumerate() {
                let lo = left + j as f32 * width / 16.0;
                let hi = left + (j + 1) as f32 * width / 16.0;
                let overlap = (hi.min(x1) - lo.max(x0)).max(0.0);
                *amount += mass * f64::from(overlap / physical_cell);
            }
        }
    }
    let sum: f64 = amounts.iter().sum();
    if sum == 0.0 {
        amounts[8] = total;
    }
    let sum: f64 = amounts.iter().sum();
    let mass = total as f32;
    let heights: Vec<f32> = amounts
        .iter()
        .map(|amount| (amount / sum * f64::from(mass) / f64::from(cell_width)) as f32)
        .collect();
    let mut center = Point::default();
    for (i, &height) in heights.iter().enumerate() {
        let fraction = height * cell_width / mass;
        center.x += (i as f32 + 0.5) * cell_width * fraction;
        center.y -= height * 0.5 * fraction;
    }
    let mut body = Flake::new(position, (mass / 1.20).sqrt().clamp(0.2, 32.0));
    body.mass = mass;
    let a_scale = scene.scale_at(a.position);
    let b_scale = scene.scale_at(b.position);
    body.velocity = Point::new(
        ((f64::from(a.velocity.x * a_scale) * (1.0 - weight)
            + f64::from(b.velocity.x * b_scale) * weight)
            / f64::from(scale)) as f32,
        ((f64::from(a.velocity.y * a_scale) * (1.0 - weight)
            + f64::from(b.velocity.y * b_scale) * weight)
            / f64::from(scale)) as f32,
    );
    body.spin = (f64::from(a.spin) * (1.0 - weight) + f64::from(b.spin) * weight) as f32;
    body.age = 1.0;
    body.slab = Some(SnowSlab {
        heights,
        cell_width,
        center,
    });
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Rect, Screen};

    #[test]
    fn pending_compaction_preserves_mass_center_and_linear_momentum() {
        let scene = Scene {
            screens: vec![Screen {
                key: 1,
                persistent_id: true,
                rect: Rect {
                    left: 0.0,
                    top: 0.0,
                    right: 800.0,
                    bottom: 600.0,
                },
                floor: 600.0,
                scale: 1.0,
            }],
            windows: vec![],
        };
        let mut a = Flake::new(Point::new(200.0, 100.0), 1.0);
        a.mass = 24.0;
        a.velocity = Point::new(-30.0, 12.0);
        a.slab = Some(SnowSlab {
            heights: vec![0.0, 2.0, 4.0, 2.0, 0.0],
            cell_width: 3.0,
            center: Point::new(7.5, -1.5),
        });
        let mut b = Flake::new(Point::new(230.0, 110.0), 1.0);
        b.mass = 6.0;
        b.velocity = Point::new(60.0, 42.0);
        b.slab = Some(SnowSlab {
            heights: vec![0.0, 1.0, 1.0, 0.0],
            cell_width: 3.0,
            center: Point::new(6.0, -0.5),
        });
        let body = merge(&a, &b, &scene);
        assert_eq!(body.mass, 30.0);
        assert_eq!(body.position, Point::new(206.0, 102.0));
        assert_eq!(body.velocity, Point::new(-12.0, 18.0));
        let slab = body.slab.unwrap();
        assert_eq!(slab.heights.len(), 16);
        assert!(slab.heights.iter().all(|h| h.is_finite() && *h >= 0.0));
        assert!((slab.heights.iter().sum::<f32>() * slab.cell_width - body.mass).abs() < 0.0001);
        a.mass = f32::from_bits(1);
        b.mass = f32::from_bits(2);
        a.slab.as_mut().unwrap().heights.fill(0.0);
        b.slab.as_mut().unwrap().heights.fill(0.0);
        let tiny = merge(&a, &b, &scene);
        assert!(tiny.mass > 0.0 && tiny.mass.is_finite());
        assert!(tiny.position.x.is_finite() && tiny.position.y.is_finite());
        assert!(tiny.velocity.x.is_finite() && tiny.velocity.y.is_finite());
        assert!(tiny.slab.unwrap().heights.iter().all(|h| h.is_finite()));
    }
}
