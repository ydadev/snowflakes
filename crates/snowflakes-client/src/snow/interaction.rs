use super::*;

fn radius(mass: f32) -> f32 {
    (mass / std::f32::consts::PI).sqrt().max(1.0)
}

impl World {
    /// Returns true when the supported body was fully handled this step.
    pub(super) fn roll_clump(
        &mut self,
        body: &mut Flake,
        cursor: Option<Cursor>,
        settings: &Settings,
    ) -> bool {
        let Some(key) = body.clump.as_ref().and_then(|c| c.support) else {
            return false;
        };
        let Some(drift) = self.drifts.get_mut(&key) else {
            body.clump.as_mut().unwrap().support = None;
            return false;
        };
        let supported = |x: f32| {
            x >= drift.support.left && x < drift.support.right && drift.support.visible_at(x)
        };
        if !supported(body.position.x)
            || (drift.support.ground && !settings.snow.on_ground)
            || (!drift.support.ground && !settings.snow.on_windows)
        {
            body.clump.as_mut().unwrap().support = None;
            body.skip_support = Some(key);
            body.velocity.y = 12.0;
            return false;
        }
        let scale = self.scene.scale_at(body.position);
        let mut pushed = false;
        if settings.interaction.cursor_enabled
            && let Some(cursor) = cursor
        {
            let distance = (body.position.x - cursor.position.x)
                .hypot(body.position.y - cursor.position.y)
                / scale;
            let reach = body.radius + 24.0;
            if distance < reach && cursor.velocity.x.abs() > 10.0 {
                let force = (1.0 - distance / reach) * 9.0;
                body.velocity.x +=
                    (cursor.velocity.x.clamp(-240.0, 240.0) - body.velocity.x) * force * DT;
                pushed = true;
            }
        }
        let index = drift.index(body.position.x);
        let left = index.saturating_sub(1);
        let right = (index + 1).min(drift.mass.len() - 1);
        let slope = (drift.height(right, &self.scene) - drift.height(left, &self.scene))
            / ((right - left).max(1) as f32 * drift.cell_width);
        body.velocity.x += -GRAVITY * slope.clamp(-0.8, 0.8) * 0.6 * DT;
        body.velocity.x = body.velocity.x.signum()
            * (body.velocity.x.abs() - 18.0 * DT).max(0.0)
            * (-1.2 * DT).exp();
        body.velocity.x = body.velocity.x.clamp(-240.0, 240.0);
        if !pushed && body.velocity.x.abs() < 3.0 {
            body.velocity.x = 0.0;
        }
        body.position.x += body.velocity.x * scale * DT;
        body.spin = body.velocity.x / body.radius.max(1.0);
        body.angle = (body.angle + body.spin * DT).rem_euclid(TAU);
        body.age += DT;
        if !supported(body.position.x) {
            body.clump.as_mut().unwrap().support = None;
            body.skip_support = Some(key);
            body.velocity.y = 12.0;
            // Horizontal motion already consumed this fixed step.
            return true;
        }
        // Snow ahead sticks to the rolling body. Incoming resting material adds
        // mass and slows the body through conservation of horizontal momentum.
        if body.velocity.x.abs() >= 3.0 {
            let before = body.mass;
            let reach = (body.radius + 4.0) * scale;
            let from = drift.index(body.position.x - reach);
            let to = drift.index(body.position.x + reach);
            for i in from..=to {
                let x = drift.x(i);
                let ahead = (x - body.position.x) * body.velocity.x.signum() / scale;
                if ahead < 0.0
                    || ahead > body.radius + 4.0
                    || !drift.support.visible_at(x)
                    || self
                        .scene
                        .paused_at(Point::new(x, drift.support.y - 1.0), &self.paused_monitors)
                {
                    continue;
                }
                let fraction =
                    (body.velocity.x.abs() * DT * scale / drift.cell_width * 0.12).min(0.2);
                let mass = (drift.mass[i] + drift.pending[i]) * fraction;
                drift.mass[i] *= 1.0 - fraction;
                drift.pending[i] *= 1.0 - fraction;
                body.mass += mass;
            }
            body.velocity.x *= before / body.mass;
            body.radius = radius(body.mass);
        }
        let index = drift.index(body.position.x);
        body.position.y =
            drift.support.y - drift.contact_height(index, &self.scene) - body.radius * scale;
        body.velocity.y = 0.0;
        let clump = body.clump.as_mut().unwrap();
        clump.idle = if body.velocity.x.abs() < 3.0 && !pushed {
            clump.idle + DT
        } else {
            0.0
        };
        if clump.idle >= 0.45 {
            self.deposit_mound(key, index, body.position.x, body.mass, settings);
            body.mass = 0.0;
        }
        true
    }

    pub(super) fn gather_snow(&mut self, cursor: Cursor) {
        if cursor.velocity.x.abs() < 15.0 {
            return;
        }
        for (&key, drift) in &mut self.drifts {
            let scale = self
                .scene
                .scale_at(Point::new(cursor.position.x, drift.support.y - 1.0));
            let existing = self.flakes.iter().position(|body| {
                body.clump.as_ref().is_some_and(|c| c.support == Some(key))
                    && (body.position.x - cursor.position.x).abs() < (body.radius + 40.0) * scale
                    && !self.scene.paused_at(body.position, &self.paused_monitors)
            });
            if existing.is_none()
                && (self.flakes.len() >= MAX_FLAKES
                    || self.flakes.iter().filter(|f| f.clump.is_some()).count() >= MAX_CLUMPS)
            {
                continue;
            }
            let mut pickup = Vec::new();
            let mut total = 0.0;
            let mut moment = 0.0;
            let from = drift.index(cursor.position.x - 28.0 * scale);
            let to = drift.index(cursor.position.x + 28.0 * scale);
            for i in from..=to {
                let x = drift.x(i);
                if !drift.support.visible_at(x)
                    || self
                        .scene
                        .paused_at(Point::new(x, drift.support.y - 1.0), &self.paused_monitors)
                {
                    continue;
                }
                let surface = drift.support.y - drift.height(i, &self.scene);
                let distance = (x - cursor.position.x).hypot(surface - cursor.position.y) / scale;
                if distance >= 28.0 {
                    continue;
                }
                let fraction = (1.0 - distance / 28.0) * 0.12;
                let mass = (drift.mass[i] + drift.pending[i]) * fraction;
                pickup.push((i, fraction));
                total += mass;
                moment += x * mass;
            }
            if total < if existing.is_some() { 0.01 } else { 12.0 } {
                continue;
            }
            for (i, fraction) in pickup {
                drift.mass[i] *= 1.0 - fraction;
                drift.pending[i] *= 1.0 - fraction;
            }
            let x = moment / total;
            let velocity = cursor.velocity.x.clamp(-240.0, 240.0) * 0.7;
            if let Some(index) = existing {
                let body = &mut self.flakes[index];
                let mass = body.mass + total;
                body.velocity.x = (body.velocity.x * body.mass + velocity * total) / mass;
                body.mass = mass;
                body.radius = radius(mass);
                body.position.y = drift.support.y
                    - drift.contact_height(drift.index(body.position.x), &self.scene)
                    - body.radius * scale;
                body.clump.as_mut().unwrap().idle = 0.0;
            } else {
                let r = radius(total);
                let index = drift.index(x);
                let mut body = Flake::new(
                    Point::new(
                        x,
                        drift.support.y - drift.contact_height(index, &self.scene) - r * scale,
                    ),
                    r,
                );
                body.mass = total;
                body.velocity.x = velocity;
                body.age = 1.0;
                body.skip_support = Some(key);
                body.clump = Some(RollingClump {
                    support: Some(key),
                    idle: 0.0,
                });
                self.flakes.push(body);
            }
        }
    }

    pub(super) fn merge_clumps(&mut self) {
        let mut clumps: Vec<Flake> = Vec::new();
        let mut others = Vec::with_capacity(self.flakes.len());
        for body in std::mem::take(&mut self.flakes) {
            let support = body.clump.as_ref().and_then(|c| c.support);
            if support.is_none() || self.scene.paused_at(body.position, &self.paused_monitors) {
                others.push(body);
                continue;
            }
            let scale = self.scene.scale_at(body.position);
            if let Some(old) = clumps.iter_mut().find(|old| {
                old.clump.as_ref().and_then(|c| c.support) == support
                    && (old.position.x - body.position.x).abs() < (old.radius + body.radius) * scale
            }) {
                let mass = old.mass + body.mass;
                old.position.x = (old.position.x * old.mass + body.position.x * body.mass) / mass;
                old.velocity.x = (old.velocity.x * old.mass + body.velocity.x * body.mass) / mass;
                old.mass = mass;
                old.radius = radius(mass);
                if let Some(drift) = support.and_then(|key| self.drifts.get(&key)) {
                    old.position.y = drift.support.y
                        - drift.contact_height(drift.index(old.position.x), &self.scene)
                        - old.radius * self.scene.scale_at(old.position);
                }
                old.clump.as_mut().unwrap().idle = old
                    .clump
                    .as_ref()
                    .unwrap()
                    .idle
                    .min(body.clump.as_ref().unwrap().idle);
            } else {
                clumps.push(body);
            }
        }
        others.extend(clumps);
        self.flakes = others;
    }
}
