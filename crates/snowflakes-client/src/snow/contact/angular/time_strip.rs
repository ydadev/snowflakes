//! A necessary projection test before building equations for an earlier hit.
use super::{AngularEdge, HALF};

impl AngularEdge<'_> {
    pub(super) fn may_reach_before(
        &self,
        y: f64,
        span: [f32; 2],
        initial: [[f64; 2]; 2],
        time: f32,
    ) -> bool {
        let m = self.motion;
        if m.scale <= m.scale_delta.abs() * HALF {
            // The chord-deviation contract uses positive endpoint scales.
            return true;
        }
        let [a, b] = initial;
        // Keep the old refinement/float-time uncertainty inside the interval.
        let cutoff = (f64::from(time) + 64.0 * f64::EPSILON).min(1.0) - HALF;
        let c = self.sample(self.local[0], cutoff).0;
        let d = self.sample(self.local[1], cutoff).0;
        let normal = [-(b[1] - a[1]), b[0] - a[0]];
        let norm = normal[0].abs() + normal[1].abs();
        if norm == 0.0 {
            return true;
        }
        let project = |p: [f64; 2]| normal[0] * (p[0] - a[0]) + normal[1] * (p[1] - a[1]);
        let values = [0.0, project(b), project(c), project(d)];
        let target = span.map(|x| project([f64::from(x), y]));
        let local = self.local.map(|p| p[0].abs() + p[1].abs());
        let local = local[0].max(local[1]);
        // The existing vertex chord-deviation bound is conservative on the
        // shorter interval too. Projection multiplies its coordinate error by
        // the L1 norm of this (not necessarily unit) normal.
        let curvature = m.bounds_offset + local * m.bounds_factor;
        let magnitude = m.center[0].abs()
            + m.center[1].abs()
            + m.velocity[0].abs()
            + m.velocity[1].abs()
            + m.scale.abs() * local
            + y.abs()
            + f64::from(span[0]).abs().max(f64::from(span[1]).abs());
        // This deliberately uses binary32 epsilon: it covers the rounded hit
        // coordinate, the solver's 256*binary64-epsilon residual allowance,
        // local interpolation and cancellation in the projection arithmetic.
        let margin = norm * (curvature + 8.0 * f64::from(f32::EPSILON) * magnitude.max(1.0));
        if !values.iter().chain(target.iter()).all(|v| v.is_finite())
            || !margin.is_finite()
            || curvature < 0.0
            || span[0] > span[1]
        {
            return true;
        }
        let low = values.into_iter().fold(f64::INFINITY, f64::min);
        let high = values.into_iter().fold(f64::NEG_INFINITY, f64::max);
        target[0].min(target[1]) <= high + margin && target[0].max(target[1]) >= low - margin
    }
}
