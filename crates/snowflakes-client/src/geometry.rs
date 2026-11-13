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
    pub rect: Rect,
    pub floor: f32,
    pub scale: f32,
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
}
impl Support {
    pub fn visible_at(&self, x: f32) -> bool {
        self.visible.iter().any(|s| s.contains(x))
    }
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
impl Scene {
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
        self.screens
            .iter()
            .find(|s| s.rect.contains(point))
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
                visible: spans,
            });
        }
        for screen in &self.screens {
            supports.push(Support {
                key: screen.key | (1 << 63),
                left: screen.rect.left,
                right: screen.rect.right,
                y: screen.floor,
                ground: true,
                visible: vec![Span {
                    left: screen.rect.left,
                    right: screen.rect.right,
                }],
            });
        }
        supports
    }
}
