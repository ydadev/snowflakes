//! GUI-independent coalescing and view data; never serialized by status/IPC.
use crate::{
    control::{Command, Setting, Status},
    session::Limits,
};
use snowflakes_protocol::{Display, MonitorSelection};
use std::collections::VecDeque;

/// Public geometry only; no raw monitor names or device interface paths.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub id: String,
    pub rect: crate::geometry::Rect,
    pub primary: bool,
    pub persistent: bool,
}
impl From<&crate::geometry::Screen> for Monitor {
    fn from(screen: &crate::geometry::Screen) -> Self {
        Self {
            id: screen.id(),
            rect: screen.rect,
            primary: screen.rect.contains(crate::geometry::Point::new(0.0, 0.0)),
            persistent: screen.persistent_id,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorRow {
    pub id: String,
    pub label: String,
    pub connected: bool,
    pub primary: bool,
}
impl MonitorRow {
    pub fn selected(&self, display: &Display) -> bool {
        match display.selection {
            MonitorSelection::All => self.connected,
            MonitorSelection::Primary => self.connected && self.primary,
            MonitorSelection::Selected => display.monitor_ids.contains(&self.id),
        }
    }
}

/// Keep absent saved and draft choices addressable. Discovery must never edit
/// preferences or reinterpret a disconnected selection as "all displays".
pub fn monitor_rows(
    monitors: &[Monitor],
    remembered: impl Iterator<Item = String>,
) -> Vec<MonitorRow> {
    let mut sorted: Vec<_> = monitors.iter().collect();
    sorted.sort_by(|a, b| {
        a.rect
            .left
            .total_cmp(&b.rect.left)
            .then(a.rect.top.total_cmp(&b.rect.top))
            .then(a.id.cmp(&b.id))
    });
    let mut rows = Vec::new();
    for monitor in sorted.into_iter().take(32) {
        if rows.iter().any(|r: &MonitorRow| r.id == monitor.id) {
            continue;
        }
        rows.push(MonitorRow {
            id: monitor.id.clone(),
            label: format!(
                "Экран {}{} — {:.0} × {:.0}{}",
                rows.len() + 1,
                if monitor.primary {
                    " (основной)"
                } else {
                    ""
                },
                monitor.rect.width(),
                monitor.rect.height(),
                if monitor.persistent {
                    ""
                } else {
                    " — без постоянного ID"
                }
            ),
            connected: true,
            primary: monitor.primary,
        });
    }
    for id in remembered.take(32) {
        if !rows.iter().any(|r| r.id == id) {
            rows.push(MonitorRow {
                id,
                label: format!("Сохранённый экран {} (недоступен)", rows.len() + 1),
                connected: false,
                primary: false,
            });
        }
    }
    rows
}

#[derive(Clone)]
pub struct View {
    pub status: Status,
    pub server_url: String,
    pub limits: Limits,
    /// None means discovery is unavailable, not that saved monitors disappeared.
    pub monitors: Option<Vec<Monitor>>,
}

pub const CONTENT_HEIGHT: i32 = 1586;
pub const DISPLAY_COMMAND: u8 = 20;
pub fn scale(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}
pub fn scroll_limit(client_height: i32, dpi: u32) -> i32 {
    (scale(CONTENT_HEIGHT, dpi) - client_height).max(0)
}
pub fn command_key(command: &Command) -> u8 {
    match command {
        Command::Set { setting } => match setting {
            Setting::SnowIntensity(_) => 1,
            Setting::FlakeSize(_) => 18,
            Setting::FlakeVisibility(_) => 19,
            Setting::RainIntensity(_) => 2,
            Setting::DriftSize(_) => 3,
            Setting::PuddleSize(_) => 4,
            Setting::Mode(_) => 5,
            Setting::EffectsEnabled(_) => 6,
            Setting::Opacity(_) => 7,
            Setting::SnowWind(_) => 8,
            Setting::RainWind(_) => 9,
            Setting::Display(_) => DISPLAY_COMMAND,
            Setting::TargetFps(_) => 21,
            Setting::BatterySaver(_) => 22,
            Setting::SnowOnWindows(_) => 23,
            Setting::SnowOnGround(_) => 24,
            Setting::SnowMelting(_) => 25,
            Setting::CursorEnabled(_) => 26,
            Setting::SnowPreset(_) => 27,
        },
        Command::FollowServer { .. } => 10,
        Command::ServerUrl { .. } => 11,
        Command::Pause {} | Command::Resume {} => 12,
        Command::Clear {} => 13,
        Command::SyncNow {} => 14,
        Command::Settings {} => 15,
        Command::Status {} => 16,
        Command::Quit {} => 17,
    }
}

/// Coalesce within a source interval. Follow/endpoint changes are barriers:
/// an edit between two source switches must keep its original source.
#[derive(Default)]
pub struct Pending {
    commands: VecDeque<Command>,
}
impl Pending {
    pub fn push(&mut self, command: Command) -> bool {
        let key = command_key(&command);
        let barrier = |command: &Command| {
            matches!(
                command,
                Command::FollowServer { .. }
                    | Command::ServerUrl { .. }
                    | Command::Set {
                        setting: Setting::SnowPreset(_)
                    }
            )
        };
        if !barrier(&command) {
            let start = self
                .commands
                .iter()
                .rposition(barrier)
                .map_or(0, |index| index + 1);
            if let Some(index) = self
                .commands
                .iter()
                .enumerate()
                .skip(start)
                .find_map(|(index, old)| (command_key(old) == key).then_some(index))
            {
                self.commands.remove(index);
            }
        }
        if self.commands.len() >= 32 {
            return false;
        }
        self.commands.push_back(command);
        true
    }
    pub fn pop(&mut self) -> Option<Command> {
        self.commands.pop_front()
    }
    pub fn contains(&self, key: u8) -> bool {
        self.commands.iter().any(|c| command_key(c) == key)
    }
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}
