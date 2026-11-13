//! Desktop coordinates are physical pixels. Material/force units stay in DIP.
//! No window titles, process names or native handles enter the simulation.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}
impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}
impl Rect {
    pub fn width(self) -> f32 {
        (self.right - self.left).max(0.0)
    }
    pub fn height(self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.left && p.x < self.right && p.y >= self.top && p.y < self.bottom
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub left: f32,
    pub right: f32,
}
impl Span {
    pub fn contains(self, x: f32) -> bool {
        x >= self.left && x < self.right
    }
}

#[derive(Debug, Clone)]
pub struct Screen {
    pub key: u64,
    pub persistent_id: bool,
    pub rect: Rect,
    pub floor: f32,
    pub scale: f32,
}
impl Screen {
    pub fn id(&self) -> String {
        format!("display-{:x}", self.key)
    }
}
#[derive(Debug, Clone)]
pub struct Window {
    pub key: u64,
    pub rect: Rect,
    /// Cloaked windows keep material but neither collect nor draw it.
    pub visible: bool,
    pub collect: bool,
}
#[derive(Debug, Default, Clone)]
pub struct Scene {
    pub screens: Vec<Screen>,
    pub windows: Vec<Window>,
}
#[derive(Debug, Clone)]
pub struct Support {
    pub key: u64,
    pub left: f32,
    pub right: f32,
    pub y: f32,
    pub ground: bool,
    pub visible: Vec<Span>,
    /// Cached, non-overlapping physical rectangles available for accumulation.
    pub draw_clips: Vec<Rect>,
}
impl Support {
    /// Require the entire rectangle to lie in the free mask, including across
    /// adjacent strips. Sampling its corners would miss a narrow occluder.
    pub fn exposes_rect(&self, rect: Rect) -> bool {
        if rect.width() <= 0.0
            || rect.height() <= 0.0
            || ![rect.left, rect.top, rect.right, rect.bottom]
                .iter()
                .all(|v| v.is_finite())
        {
            return false;
        }
        let mut edges = vec![rect.left, rect.right];
        for clip in &self.draw_clips {
            for x in [clip.left, clip.right] {
                if x > rect.left && x < rect.right {
                    edges.push(x);
                }
            }
        }
        edges.sort_unstable_by(f32::total_cmp);
        edges.dedup();
        edges.windows(2).all(|span| {
            self.draw_clips.iter().any(|clip| {
                clip.left <= span[0]
                    && clip.right >= span[1]
                    && clip.top <= rect.top
                    && clip.bottom >= rect.bottom
            })
        })
    }
    pub fn visible_at(&self, x: f32) -> bool {
        self.visible.iter().any(|s| s.contains(x))
    }
    pub fn exposed(&self, point: Point) -> bool {
        self.draw_clips.iter().any(|r| {
            point.x >= r.left && point.x < r.right && point.y >= r.top && point.y <= r.bottom
        })
    }
    pub fn exposed_headroom(&self, point: Point) -> f32 {
        self.draw_clips
            .iter()
            .filter(|r| {
                point.x >= r.left && point.x < r.right && point.y >= r.top && point.y <= r.bottom
            })
            .map(|r| point.y - r.top)
            .fold(0.0, f32::max)
    }
}

fn accumulation_clips(
    spans: &[Span],
    y: f32,
    depth: f32,
    covers: impl Iterator<Item = Rect>,
) -> Vec<Rect> {
    let covers: Vec<_> = covers
        .filter(|r| r.top < y && r.bottom > y - depth && r.width() > 0.0)
        .collect();
    let mut clips = Vec::new();
    for span in spans {
        let mut edges = vec![span.left, span.right];
        for cover in &covers {
            if cover.left < span.right && cover.right > span.left {
                edges.push(cover.left.max(span.left));
                edges.push(cover.right.min(span.right));
            }
        }
        edges.sort_unstable_by(f32::total_cmp);
        edges.dedup();
        // Each vertical strip contains maximal free intervals. A rectangular
        // subdivision with horizontal seams would falsely limit growth beside
        // a cover, even though that whole column has unobstructed headroom.
        for pair in edges.windows(2) {
            let mut vertical = vec![Span {
                left: y - depth,
                right: y,
            }];
            for cover in &covers {
                if cover.left < pair[1] && cover.right > pair[0] {
                    subtract(
                        &mut vertical,
                        Span {
                            left: cover.top,
                            right: cover.bottom,
                        },
                    );
                }
            }
            for interval in vertical {
                // Hide pathological masks rather than allocate unbounded
                // geometry or paint accumulation over system UI.
                if clips.len() == 2048 {
                    return Vec::new();
                }
                clips.push(Rect {
                    left: pair[0],
                    right: pair[1],
                    top: interval.left,
                    bottom: interval.right,
                });
            }
        }
    }
    clips
}

/// A bounded, disjoint mask for moving material in desktop coordinates.
/// Exceeding the mask budget hides the material rather than covering a window.
pub fn uncovered_rectangles(rect: Rect, covers: impl Iterator<Item = Rect>) -> Vec<Rect> {
    if rect.width() <= 0.0
        || rect.height() <= 0.0
        || ![rect.left, rect.top, rect.right, rect.bottom]
            .iter()
            .all(|v| v.is_finite())
    {
        return Vec::new();
    }
    accumulation_clips(
        &[Span {
            left: rect.left,
            right: rect.right,
        }],
        rect.bottom,
        rect.height(),
        covers,
    )
}

pub fn subtract(spans: &mut Vec<Span>, cover: Span) {
    let mut next = Vec::with_capacity(spans.len() + 1);
    for span in spans.iter().copied() {
        if cover.right <= span.left || cover.left >= span.right {
            next.push(span);
            continue;
        }
        if cover.left > span.left {
            next.push(Span {
                left: span.left,
                right: cover.left,
            });
        }
        if cover.right < span.right {
            next.push(Span {
                left: cover.right,
                right: span.right,
            });
        }
    }
    *spans = next;
}

/// Follow native support geometry without oscillation or a frame-sized jump.
/// This moves the support as a whole; it does not change the material profile.
pub fn approach_floor(current: f32, target: f32, scale: f32, dt: f32) -> f32 {
    let scale = scale.clamp(0.5, 4.0);
    let delta = target - current;
    if delta.abs() <= 0.05 * scale {
        return target;
    }
    let limit = 360.0 * scale * dt;
    current + (delta * (1.0 - (-dt / 0.09).exp())).clamp(-limit, limit)
}
impl Scene {
    pub fn ground_support(&self, screen: &Screen, y: f32) -> Support {
        let spans = vec![Span {
            left: screen.rect.left,
            right: screen.rect.right,
        }];
        Support {
            key: screen.key | (1 << 63),
            left: screen.rect.left,
            right: screen.rect.right,
            y,
            ground: true,
            // Preserve the floor over ordinary apps, but mask menus, taskbars
            // and other visible non-collecting overlays at their actual position.
            draw_clips: accumulation_clips(
                &spans,
                y,
                490.0,
                self.windows
                    .iter()
                    .filter(|w| w.visible && !w.collect)
                    .map(|w| w.rect),
            ),
            visible: spans,
        }
    }
    /// Off-screen emission/exit margins belong to the nearest display. This
    /// avoids advancing particles just above a display whose physics is paused.
    pub fn nearest_screen(&self, point: Point) -> Option<&Screen> {
        let distance = |s: &Screen| {
            let dx = point.x - point.x.clamp(s.rect.left, s.rect.right);
            let dy = point.y - point.y.clamp(s.rect.top, s.rect.bottom);
            dx * dx + dy * dy
        };
        self.screens
            .iter()
            .find(|s| s.rect.contains(point))
            .or_else(|| {
                self.screens
                    .iter()
                    .min_by(|a, b| distance(a).total_cmp(&distance(b)))
            })
    }
    pub fn paused_at(&self, point: Point, paused: &std::collections::BTreeSet<u64>) -> bool {
        !paused.is_empty()
            && self
                .nearest_screen(point)
                .is_some_and(|s| paused.contains(&s.key))
    }
    pub fn bounds(&self) -> Rect {
        self.screens
            .iter()
            .map(|s| s.rect)
            .reduce(|a, b| Rect {
                left: a.left.min(b.left),
                top: a.top.min(b.top),
                right: a.right.max(b.right),
                bottom: a.bottom.max(b.bottom),
            })
            .unwrap_or_default()
    }
    pub fn scale_at(&self, point: Point) -> f32 {
        // Emission margins, display gaps and ledges at the top edge keep the
        // same display ownership as pause and admission accounting.
        self.nearest_screen(point)
            .map_or(1.0, |s| s.scale.clamp(0.5, 4.0))
    }
    pub fn headroom(&self, x: f32, y: f32) -> f32 {
        self.screens
            .iter()
            .filter(|s| {
                x >= s.rect.left && x < s.rect.right && y >= s.rect.top && y <= s.rect.bottom
            })
            .map(|s| (y - s.rect.top).max(0.0))
            .fold(0.0, f32::max)
    }
    pub fn supports(&self) -> Vec<Support> {
        let mut supports = Vec::new();
        for (index, window) in self.windows.iter().enumerate().take(256) {
            if !window.collect || window.rect.width() < 24.0 {
                continue;
            }
            let inset = 6.0 * self.scale_at(Point::new(window.rect.left + 10.0, window.rect.top));
            let left = window.rect.left + inset;
            let right = window.rect.right - inset;
            let y = window.rect.top;
            let mut spans = Vec::new();
            if window.visible {
                for screen in &self.screens {
                    if y > screen.rect.top + 0.5 && y < screen.rect.bottom {
                        let span = Span {
                            left: left.max(screen.rect.left),
                            right: right.min(screen.rect.right),
                        };
                        if span.right > span.left {
                            spans.push(span);
                        }
                    }
                }
                for front in &self.windows[..index] {
                    if front.visible && front.rect.top <= y && front.rect.bottom > y {
                        subtract(
                            &mut spans,
                            Span {
                                left: front.rect.left,
                                right: front.rect.right,
                            },
                        );
                    }
                }
            }
            supports.push(Support {
                key: window.key,
                left,
                right,
                y,
                ground: false,
                draw_clips: accumulation_clips(
                    &spans,
                    y,
                    140.0,
                    self.windows[..index]
                        .iter()
                        .filter(|w| w.visible)
                        .map(|w| w.rect),
                ),
                visible: spans,
            });
        }
        for screen in &self.screens {
            supports.push(self.ground_support(screen, screen.floor));
        }
        supports
    }
}
