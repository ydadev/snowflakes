use super::{Cursor, Flake, World};
use crate::geometry::Point;

impl World {
    pub(super) fn apply_particle_cursor(
        &self,
        body: &Flake,
        cursor: Cursor,
        scale: f32,
        air: &mut Point,
    ) {
        #[cfg(test)]
        if super::cursor_tests::use_literal() {
            super::cursor_tests::literal(self, body, cursor, scale, air);
            return;
        }
        let dx = (body.position.x - cursor.position.x) / scale;
        let dy = (body.position.y - cursor.position.y) / scale;
        let distance = dx.hypot(dy);
        // The mask predicate is read-only. Bodies outside the original force
        // interval cannot affect air, regardless of their inherited depth.
        if distance < 65.0 && distance > 0.1 && self.falling_cursor_visible(body, cursor.position) {
            let weight = (1.0 - distance / 65.0).powi(2);
            air.x += (dx / distance * 36.0 + cursor.velocity.x.clamp(-120.0, 120.0) * 0.5) * weight;
            air.y +=
                (dy / distance * 24.0 + cursor.velocity.y.clamp(-100.0, 100.0) * 0.35) * weight;
        }
    }
}
