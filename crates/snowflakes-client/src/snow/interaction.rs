use super::*;

impl World {
    /// Conservative transport within the snow layer. A cursor pushes the
    /// profile itself; only material leaving a ledge becomes a falling slab.
    pub(super) fn push_snow(&mut self, cursor: Option<Cursor>, settings: &Settings) {
        for drift in self.drifts.values_mut() {
            let count = drift.mass.len();
            let active = |i: usize| {
                drift.support.visible_at(drift.x(i))
                    && !self.scene.paused_at(
                        Point::new(drift.x(i), drift.support.y - 1.0),
                        &self.paused_monitors,
                    )
            };
            let cursor = cursor.filter(|_| settings.interaction.cursor_enabled);
            if cursor.is_none() && drift.sweep_velocity.iter().all(|v| v.abs() < 1.0) {
                continue;
            }
            let mut velocity = drift.sweep_velocity.clone();
            for (i, v) in velocity.iter_mut().enumerate() {
                if !active(i) {
                    continue;
                }
                *v = v.signum() * (v.abs() - 24.0 * DT).max(0.0) * (-2.0 * DT).exp();
                if let Some(cursor) = cursor {
                    let x = drift.x(i);
                    let scale = self.scene.scale_at(Point::new(x, drift.support.y - 1.0));
                    let y = drift.support.y - drift.contact_height(i, &self.scene);
                    let distance = (x - cursor.position.x).hypot(y - cursor.position.y) / scale;
                    if distance < 28.0 && cursor.velocity.x.abs() > 10.0 {
                        let weight = (1.0 - distance / 28.0) * 0.8;
                        *v += (cursor.velocity.x.clamp(-360.0, 360.0) - *v) * weight;
                    }
                }
                if v.abs() < 1.0 {
                    *v = 0.0;
                }
            }
            let mut resting = drift.mass.clone();
            let mut pending = drift.pending.clone();
            let mut momentum: Vec<f32> = velocity
                .iter()
                .enumerate()
                .map(|(i, v)| (drift.mass[i] + drift.pending[i]) * v)
                .collect();
            let mut edge = None;
            for (i, &v) in velocity.iter().enumerate() {
                if !active(i) || v.abs() < 1.0 {
                    continue;
                }
                let next = if v > 0.0 {
                    i.checked_add(1).filter(|&n| n < count)
                } else {
                    i.checked_sub(1)
                };
                let next = next.filter(|&n| drift.support.visible_at(drift.x(n)));
                let Some(next) = next else {
                    if !drift.support.ground
                        && v.abs() > 15.0
                        && drift.mass[i] + drift.pending[i] > 0.0
                    {
                        edge = Some((i, v.signum()));
                    }
                    continue;
                };
                if !active(next) {
                    continue;
                }
                let scale = self
                    .scene
                    .scale_at(Point::new(drift.x(i), drift.support.y - 1.0));
                // The upwind flux is evaluated from the original profile and
                // applied simultaneously, never from a just-updated neighbour.
                let fraction = (v.abs() * DT * scale / drift.cell_width).min(0.8);
                let dense = drift.mass[i] * fraction;
                let loose = drift.pending[i] * fraction;
                resting[i] -= dense;
                resting[next] += dense;
                pending[i] -= loose;
                pending[next] += loose;
                let impulse = (dense + loose) * v;
                momentum[i] -= impulse;
                momentum[next] += impulse;
            }
            drift.mass = resting;
            drift.pending = pending;
            for (i, impulse) in momentum.into_iter().enumerate() {
                if self.scene.paused_at(
                    Point::new(drift.x(i), drift.support.y - 1.0),
                    &self.paused_monitors,
                ) {
                    continue;
                }
                let mass = drift.mass[i] + drift.pending[i];
                drift.sweep_velocity[i] = if mass > 0.001 {
                    (impulse / mass).clamp(-360.0, 360.0)
                } else {
                    0.0
                };
            }
            drift.shedding_cooldown = (drift.shedding_cooldown - DT).max(0.0);
            let Some((edge, direction)) = edge else {
                continue;
            };
            if drift.shedding_cooldown > 0.0 || self.spill.len() >= 128 {
                continue;
            }
            // Accumulate at the edge, then release one contiguous short layer.
            // The cooldown and minimum layer thickness prevent a particle spray.
            let mut indices = vec![edge];
            for offset in 1..4 {
                let candidate = if direction > 0.0 {
                    edge.checked_sub(offset)
                } else {
                    edge.checked_add(offset).filter(|&n| n < count)
                };
                let Some(i) = candidate else {
                    break;
                };
                if !drift.support.visible_at(drift.x(i))
                    || self.scene.paused_at(
                        Point::new(drift.x(i), drift.support.y - 1.0),
                        &self.paused_monitors,
                    )
                    || drift.sweep_velocity[i] * direction < 1.0
                {
                    break;
                }
                indices.push(i);
            }
            indices.sort_unstable();
            let first = indices[0];
            let scale = self
                .scene
                .scale_at(Point::new(drift.x(edge), drift.support.y - 1.0));
            let cell_width = drift.cell_width / scale;
            let heights: Vec<f32> = indices
                .iter()
                .map(|&i| (drift.mass[i] + drift.pending[i]) * 0.7 / cell_width)
                .collect();
            let mass: f32 = heights.iter().sum::<f32>() * cell_width;
            if mass < 18.0 || heights.iter().sum::<f32>() / (heights.len() as f32) < 0.4 {
                continue;
            }
            let mut center = Point::default();
            for (i, &height) in heights.iter().enumerate() {
                let weight = height * cell_width / mass;
                center.x += (i as f32 + 0.5) * cell_width * weight;
                center.y -= height * 0.5 * weight;
            }
            for &i in &indices {
                drift.mass[i] *= 0.3;
                drift.pending[i] *= 0.3;
            }
            let position = Point::new(
                drift.x(first) - drift.cell_width * 0.5 + center.x * scale,
                drift.support.y + center.y * scale,
            );
            let mut slab = Flake::new(position, (mass / 1.2).sqrt().clamp(0.2, 32.0));
            slab.mass = mass;
            slab.velocity = Point::new(
                direction * (velocity[edge].abs() * 0.5).clamp(20.0, 180.0),
                18.0,
            );
            slab.age = 1.0;
            slab.spin = direction * 0.65;
            slab.skip_support = Some(drift.support.key);
            slab.slab = Some(SnowSlab {
                heights,
                cell_width,
                center,
            });
            self.spill.push_back(slab);
            drift.shedding_cooldown = 0.25;
        }
    }
}
