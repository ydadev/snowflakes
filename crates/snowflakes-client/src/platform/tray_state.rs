//! Shell notification delivery, independent of native calls and the weather model.
use crate::{control::Status, session::PauseReason};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Label {
    #[default]
    Starting,
    Active,
    Paused,
    Disabled,
    Waiting,
    SystemUnavailable,
    GraphicsRecovering(Option<i32>),
    GraphicsUnavailable(Option<i32>),
}
impl Label {
    pub fn from_status(status: &Status) -> Self {
        match status.renderer.as_str() {
            "graphics_unavailable" => Self::GraphicsUnavailable(status.graphics_error),
            "graphics_recovering" => Self::GraphicsRecovering(status.graphics_error),
            "system_events_unavailable" => Self::SystemUnavailable,
            _ if status.pauses.contains(&PauseReason::Manual) => Self::Paused,
            _ if status.settings.is_none() => Self::Waiting,
            _ if !status.desired_effects_enabled => Self::Disabled,
            _ if !status.pauses.is_empty() => Self::Paused,
            _ if status.rendering => Self::Active,
            _ => Self::Starting,
        }
    }
    pub fn text(self) -> String {
        let text = match self {
            Self::Starting => "Подготовка эффекта",
            Self::Active => "Эффект работает",
            Self::Paused => "Пауза",
            Self::Disabled => "Эффект выключен",
            Self::Waiting => "Ожидание профиля сервера",
            Self::SystemUnavailable => "Нет доступа к событиям сеанса; эффект приостановлен",
            Self::GraphicsRecovering(_) => "Ошибка графики; выполняется восстановление",
            Self::GraphicsUnavailable(_) => "Графика недоступна; восстановление остановлено",
        };
        let error = match self {
            Self::GraphicsRecovering(error) | Self::GraphicsUnavailable(error) => error,
            _ => None,
        };
        match error {
            Some(code) => format!("Snowflakes — {text} (0x{:08X})", code as u32),
            None => format!("Snowflakes — {text}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Add,
    SetVersion,
    Modify,
    Delete,
}

#[derive(Default)]
enum Phase {
    #[default]
    Missing,
    Version,
    Ready,
}

#[derive(Default)]
pub(super) struct Icon {
    phase: Phase,
    delivered: Option<Label>,
    retry: Option<Instant>,
    failures: u8,
}
impl Icon {
    pub fn recreated(&mut self) {
        *self = Self::default();
    }
    pub fn version4(&self) -> bool {
        matches!(self.phase, Phase::Ready)
    }
    /// At most three calls: Add, SetVersion, and a possibly newer label.
    /// Partial success is retained; version failure must not repeat Add.
    pub fn tick(
        &mut self,
        now: Instant,
        label: Label,
        mut notify: impl FnMut(Action, Label) -> bool,
    ) {
        if self.retry.is_some_and(|deadline| now < deadline) {
            return;
        }
        for _ in 0..3 {
            let action = match self.phase {
                Phase::Missing => Action::Add,
                Phase::Version => Action::SetVersion,
                Phase::Ready if self.delivered != Some(label) => Action::Modify,
                Phase::Ready => return,
            };
            if !notify(action, label) {
                let delay = (1_u64 << self.failures.min(5)).min(30);
                self.failures = self.failures.saturating_add(1);
                self.retry = Some(now + Duration::from_secs(delay));
                return;
            }
            self.failures = 0;
            self.retry = None;
            match action {
                Action::Add => {
                    self.delivered = Some(label);
                    self.phase = Phase::Version;
                }
                Action::SetVersion => self.phase = Phase::Ready,
                Action::Modify => self.delivered = Some(label),
                Action::Delete => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_restore_waits_after_failures_and_delivers_the_latest_reason() {
        let start = Instant::now();
        let mut icon = Icon::default();
        let mut calls = Vec::new();
        icon.tick(start, Label::Paused, |a, l| {
            calls.push((a, l));
            true
        });
        assert!(icon.version4());
        icon.recreated();
        icon.tick(start, Label::Paused, |a, l| {
            calls.push((a, l));
            false
        });
        let error = Label::GraphicsUnavailable(Some(0x887A0005_u32 as i32));
        for ms in 1..1000 {
            icon.tick(start + Duration::from_millis(ms), error, |_, _| {
                panic!("early retry")
            });
        }
        icon.tick(start + Duration::from_secs(1), error, |a, l| {
            calls.push((a, l));
            true
        });
        assert_eq!(
            calls,
            vec![
                (Action::Add, Label::Paused),
                (Action::SetVersion, Label::Paused),
                (Action::Add, Label::Paused),
                (Action::Add, error),
                (Action::SetVersion, error),
            ]
        );
        icon.tick(start + Duration::from_secs(90), error, |_, _| {
            panic!("unchanged label")
        });
    }

    #[test]
    fn version_failure_keeps_the_added_icon_and_retries_only_version_then_dirty_text() {
        let now = Instant::now();
        let mut icon = Icon::default();
        let mut actions = Vec::new();
        icon.tick(now, Label::Starting, |a, _| {
            actions.push(a);
            a == Action::Add
        });
        assert!(!icon.version4());
        icon.tick(now + Duration::from_secs(1), Label::Paused, |a, _| {
            actions.push(a);
            true
        });
        assert_eq!(
            actions,
            [
                Action::Add,
                Action::SetVersion,
                Action::SetVersion,
                Action::Modify
            ]
        );
        assert!(icon.version4());
    }

    #[test]
    fn persistent_shell_failure_is_rate_limited_but_late_recovery_is_possible() {
        let now = Instant::now();
        let mut icon = Icon::default();
        let mut attempts = Vec::new();
        for second in 0..=121 {
            icon.tick(
                now + Duration::from_secs(second),
                Label::Paused,
                |action, _| {
                    assert_eq!(action, Action::Add);
                    attempts.push(second);
                    false
                },
            );
        }
        assert_eq!(attempts, [0, 1, 3, 7, 15, 31, 61, 91, 121]);
        icon.tick(now + Duration::from_secs(150), Label::Paused, |_, _| {
            panic!("early retry")
        });
        let mut actions = Vec::new();
        icon.tick(now + Duration::from_secs(151), Label::Paused, |a, _| {
            actions.push(a);
            true
        });
        assert_eq!(actions, [Action::Add, Action::SetVersion]);
    }

    #[test]
    fn failed_text_update_keeps_version_and_can_be_superseded_or_recreated() {
        let now = Instant::now();
        let mut icon = Icon::default();
        icon.tick(now, Label::Active, |_, _| true);
        icon.tick(now, Label::GraphicsRecovering(None), |a, _| {
            assert_eq!(a, Action::Modify);
            false
        });
        assert!(icon.version4());
        let mut calls = Vec::new();
        icon.tick(now + Duration::from_secs(1), Label::Paused, |a, l| {
            calls.push((a, l));
            true
        });
        assert_eq!(calls, [(Action::Modify, Label::Paused)]);
        icon.tick(now + Duration::from_secs(1), Label::Disabled, |_, _| false);
        icon.recreated();
        calls.clear();
        icon.tick(now + Duration::from_secs(1), Label::Waiting, |a, l| {
            calls.push((a, l));
            true
        });
        assert_eq!(
            calls,
            [
                (Action::Add, Label::Waiting),
                (Action::SetVersion, Label::Waiting)
            ]
        );
    }
}
