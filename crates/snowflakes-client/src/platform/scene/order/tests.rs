use super::*;
use crate::scene_registry::Source;
use windows::{
    Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId},
    core::w,
};

struct Fixtures(Vec<HWND>);
impl Fixtures {
    fn new() -> Self {
        let mut fixture = Self(Vec::new());
        for _ in 0..3 {
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    w!("STATIC"),
                    w!(""),
                    WS_POPUP,
                    0,
                    0,
                    2,
                    2,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap()
            };
            fixture.0.push(hwnd);
        }
        fixture
    }
    fn check(&self, hwnd: HWND) {
        assert!(self.0.contains(&hwnd));
        assert!(
            owner(hwnd.0 as usize)
                == Some(Owner {
                    process: unsafe { GetCurrentProcessId() },
                    thread: unsafe { GetCurrentThreadId() }
                })
        );
        assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
    }
    fn place(&self, hwnd: HWND, after: HWND) {
        self.check(hwnd);
        unsafe {
            SetWindowPos(
                hwnd,
                Some(after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
            .unwrap();
        }
    }
}
impl Drop for Fixtures {
    fn drop(&mut self) {
        for &hwnd in &self.0 {
            self.check(hwnd);
            unsafe {
                DestroyWindow(hwnd).unwrap();
            }
        }
    }
}

#[test]
fn native_order_revalidates_own_hidden_windows_in_normal_and_topmost_groups() {
    let fixture = Fixtures::new();
    for topmost in [false, true] {
        for permutation in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            for &hwnd in &fixture.0 {
                fixture.place(
                    hwnd,
                    if topmost {
                        HWND_TOPMOST
                    } else {
                        HWND_NOTOPMOST
                    },
                );
            }
            for index in permutation {
                fixture.place(
                    fixture.0[index],
                    if topmost { HWND_TOPMOST } else { HWND_TOP },
                );
            }
            let mut native = super::super::Native::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let order = loop {
                match native.order() {
                    Ok(order) => break order,
                    Err(_) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(5))
                    }
                    Err(error) => panic!(
                        "bounded native ordering did not stabilize: {:?}",
                        error.code()
                    ),
                }
            };
            let actual: Vec<_> = order
                .into_iter()
                .filter(|h| fixture.0.contains(&HWND(*h as *mut _)))
                .collect();
            let expected: Vec<_> = permutation
                .into_iter()
                .rev()
                .map(|i| fixture.0[i].0 as usize)
                .collect();
            assert!(actual == expected, "relative hidden fixture order changed");
            for &hwnd in &fixture.0 {
                fixture.check(hwnd);
                assert!(native.owner(hwnd.0 as usize).is_some());
            }
        }
    }
    for (top, expected) in [(0, [0, 2, 1]), (1, [1, 2, 0])] {
        for &hwnd in &fixture.0 {
            fixture.place(hwnd, HWND_NOTOPMOST);
        }
        for &hwnd in &fixture.0 {
            fixture.place(hwnd, HWND_TOP);
        }
        fixture.place(fixture.0[top], HWND_TOPMOST);
        let mut native = super::super::Native::default();
        let order = native.order().unwrap();
        let actual: Vec<_> = order
            .into_iter()
            .filter(|h| fixture.0.contains(&HWND(*h as *mut _)))
            .collect();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|i| fixture.0[i].0 as usize)
            .collect();
        assert!(
            actual == expected,
            "mixed topmost/normal fixture order changed"
        );
    }
}
