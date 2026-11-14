//! Requested display affinity and bounded per-window application results.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Failure {
    Api(i32),
    DifferentAffinity(u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub requested: bool,
    pub windows: u32,
    pub applied: u32,
    pub failed: u32,
    pub last_failure: Option<Failure>,
}
impl Report {
    pub fn pending(requested: bool) -> Self {
        Self {
            requested,
            ..Self::default()
        }
    }
    pub(crate) fn observe(&mut self, state: &State) {
        self.windows = self.windows.saturating_add(1);
        if state.requested == Some(self.requested) {
            match state.result {
                Some(Ok(())) => self.applied = self.applied.saturating_add(1),
                Some(Err(failure)) => {
                    self.failed = self.failed.saturating_add(1);
                    self.last_failure = Some(failure);
                }
                None => (),
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct State {
    requested: Option<bool>,
    result: Option<Result<(), Failure>>,
}
impl State {
    /// A failure is reported once for this request/window. A changed request
    /// or a newly constructed window permits another attempt.
    pub fn ensure(&mut self, requested: bool, apply: impl FnOnce(bool) -> Result<(), Failure>) {
        if self.requested != Some(requested) {
            self.requested = Some(requested);
            self.result = Some(apply(requested));
        }
    }
}

pub fn description(requested: bool, report: &Report) -> String {
    if report.windows == 0 {
        return if requested {
            "Будет применено при создании слоя. Поддержка зависит от средства захвата.".into()
        } else {
            "Исключение из захвата выключено в настройках.".into()
        };
    }
    if requested != report.requested {
        return "Ожидается применение новой настройки к слоям эффекта.".into();
    }
    if report.failed > 0 {
        return format!(
            "Не подтверждено применение для {} из {} слоёв. Для повтора переключите параметр.",
            report.failed, report.windows
        );
    }
    if report.applied != report.windows {
        return "Ожидается применение к слоям эффекта.".into();
    }
    if requested {
        "Применено к слоям эффекта. Поддержка зависит от средства захвата.".into()
    } else {
        "Исключение из захвата выключено.".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_bounded_per_request_and_new_windows_apply_independently() {
        let mut state = State::default();
        let mut calls = 0;
        for _ in 0..200 {
            state.ensure(true, |_| {
                calls += 1;
                Err(Failure::Api(-5))
            });
        }
        assert_eq!(calls, 1);
        let mut report = Report::pending(true);
        report.observe(&state);
        assert_eq!((report.windows, report.applied, report.failed), (1, 0, 1));
        state.ensure(false, |_| {
            calls += 1;
            Ok(())
        });
        state.ensure(true, |_| {
            calls += 1;
            Ok(())
        });
        let mut recreated = State::default();
        recreated.ensure(true, |_| {
            calls += 1;
            Ok(())
        });
        assert_eq!(calls, 4);
        let mut report = Report::pending(true);
        report.observe(&state);
        report.observe(&recreated);
        assert_eq!((report.windows, report.applied, report.failed), (2, 2, 0));
    }

    #[test]
    fn reports_keep_partial_failure_and_stale_requests_distinct() {
        let mut applied = State::default();
        applied.ensure(true, |_| Ok(()));
        let mut failed = State::default();
        failed.ensure(true, |_| Err(Failure::DifferentAffinity(1)));
        let mut report = Report::pending(true);
        report.observe(&applied);
        report.observe(&failed);
        assert_eq!((report.windows, report.applied, report.failed), (2, 1, 1));
        assert_eq!(report.last_failure, Some(Failure::DifferentAffinity(1)));
        assert!(description(true, &report).contains("Не подтверждено"));
        let mut pending = Report::pending(false);
        pending.observe(&applied);
        assert_eq!(
            (pending.windows, pending.applied, pending.failed),
            (1, 0, 0)
        );
        assert!(description(false, &pending).contains("Ожидается"));
        assert!(description(true, &Report::default()).contains("Будет применено"));
        let bytes = serde_json::to_vec(&report).unwrap();
        assert_eq!(serde_json::from_slice::<Report>(&bytes).unwrap(), report);
    }
}
