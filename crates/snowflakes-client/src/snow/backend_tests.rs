use super::*;
use crate::geometry::Window;

#[test]
fn world_reports_the_evaluator_passed_to_rotating_contacts() {
    for evaluator in [Evaluator::portable(), Evaluator::new()] {
        let mut scene = Scene::default();
        scene.screens.push(Screen {
            key: 1,
            persistent_id: true,
            rect: Rect {
                left: 0.0,
                right: 800.0,
                top: 0.0,
                bottom: 900.0,
            },
            floor: 900.0,
            scale: 1.0,
        });
        scene.windows.push(Window {
            key: 7,
            rect: Rect {
                left: 300.0,
                right: 500.0,
                top: 300.0,
                bottom: 400.0,
            },
            visible: true,
            collect: true,
        });
        let mut world = World::new(scene, 59);
        world.contact_evaluator = evaluator;
        let backend = world.contact_backend();
        assert_eq!(backend, evaluator.backend());
        let mut settings = Settings::default();
        settings.snow.intensity_percent = 0;
        settings.snow.melting = false;
        settings.snow.drift_size_percent = 100;
        let mut body = Flake::new(Point::new(400.0, 295.0), (160.0_f32 / 1.2).sqrt());
        body.mass = 160.0;
        body.spin = 0.55;
        body.angle = 0.08;
        body.velocity.y = 240.0;
        body.slab = Some(SnowSlab {
            heights: vec![4.0; 4],
            cell_width: 10.0,
            center: Point::new(20.0, -2.0),
        });
        assert!(world.release(body));
        Evaluator::reset_test_usage();
        world.step(&settings, None);
        let calls = Evaluator::test_usage();
        let (values, refinements, other) = match backend {
            ContactBackend::Portable => (calls[0], calls[2], calls[1] + calls[3]),
            ContactBackend::Fma => (calls[1], calls[3], calls[0] + calls[2]),
        };
        assert!(values > 0 && refinements > 0);
        assert_eq!(other, 0);
        assert!(world.flakes.is_empty() && world.drifts[&7].total_mass() > 0.0);
        world.clear();
        assert_eq!(world.contact_backend(), backend);
    }
}
