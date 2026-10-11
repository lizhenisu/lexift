//! Cross-viewport geometry preparation, independent pixels and bounded snapshot ownership.
use super::tests::object;
use super::*;

fn views() -> [(Rect, f32); 2] {
    [
        (
            Rect {
                left: -96,
                top: -64,
                right: 0,
                bottom: 64,
            },
            1.,
        ),
        (
            Rect {
                left: 0,
                top: -64,
                right: 96,
                bottom: 64,
            },
            1.25,
        ),
    ]
}

#[test]
fn changing_trajectories_share_one_snapshot_and_match_independent_rendering() {
    for kind in [Kind::Pencil, Kind::Polyline, Kind::Arrow] {
        let mut session = Session::new();
        let mut shape = object(kind);
        shape.bounds = Bounds::from_corners((-30., -20.), (30., 20.));
        shape.points = vec![(-30., -20.), (30., 20.), (0., -35.)];
        session.objects.push(shape);
        let mut compiler = SceneCompiler::default();
        for change in 0..4 {
            session.objects[0].style.width = 2. + change as f32;
            session.objects[0].points[0].0 -= 2.;
            let views = if change == 3 {
                [views()[1], views()[0]]
            } else {
                views()
            };
            let layers = compiler.layers(&session, &views, None, None).unwrap();
            let snapshots: Vec<_> = compiler
                .objects
                .values()
                .map(|entry| &entry.object)
                .collect();
            assert_eq!(snapshots.len(), 2);
            assert!(Arc::ptr_eq(snapshots[0], snapshots[1]));
            assert_eq!(compiler.retained_trajectory_payload().0, 1);
            for (view, scale) in views {
                let actual = compose(
                    view,
                    layers.iter().filter_map(|layer| {
                        if let RenderLayer::Raster(raster) = layer {
                            Some(raster)
                        } else {
                            None
                        }
                    }),
                )
                .unwrap();
                let expected =
                    annotation_render::render_editing(&session, view, scale, None, None, None)
                        .unwrap();
                assert!(
                    actual
                        .data()
                        .iter()
                        .zip(expected.data())
                        .all(|(a, b)| a.abs_diff(*b) <= 1),
                    "kind={kind:?} change={change} view={view:?}"
                );
            }
        }
        let weak = Arc::downgrade(&compiler.objects.values().next().unwrap().object);
        compiler.clear();
        assert!(weak.upgrade().is_none());
    }
}

#[test]
fn invisible_geometry_preparation_does_not_copy_the_object_snapshot() {
    let mut shape = object(Kind::Pencil);
    shape.bounds = Bounds::from_corners((1000., 1000.), (1020., 1020.));
    shape.points = vec![(1000., 1000.), (1020., 1020.)];
    let prepared = PreparedObject::new(&shape);
    for (view, scale) in views() {
        assert!(clipped_bounds(prepared.bounds, shape.style.width, view, scale).is_none());
    }
    assert!(prepared.object.is_none());
    assert!(prepared.line_points.is_none());
}

#[test]
fn shared_curved_centerlines_preserve_dash_fill_endpoints_and_changed_points() {
    use lexift_core::domain::annotation::Endpoint;
    for kind in [Kind::Arrow, Kind::Polyline] {
        for dash in 0..5 {
            let mut shape = object(kind);
            shape.bounds = Bounds::from_corners((-45., -30.), (45., 30.));
            shape.points = vec![(-45., -30.), (45., 30.), (0., -38.), (35., -15.)];
            shape.style.dash = dash;
            shape.style.fill = kind == Kind::Polyline && dash == 4;
            shape.extra = if kind == Kind::Arrow {
                Extra::Arrow {
                    curved: true,
                    head: true,
                    start: Endpoint::OpenCircle,
                    end: Endpoint::Diamond,
                }
            } else {
                Extra::Polyline {
                    curved: true,
                    head: true,
                    start: Endpoint::Bar,
                    end: Endpoint::FilledArrow,
                }
            };
            let mut session = Session::new();
            session.objects.push(shape);
            let mut compiler = SceneCompiler::default();
            for change in 0..2 {
                session.objects[0].points[1].0 -= 4.;
                let layers = compiler.layers(&session, &views(), None, None).unwrap();
                for (view, scale) in views() {
                    let actual = compose(
                        view,
                        layers.iter().filter_map(|layer| {
                            if let RenderLayer::Raster(raster) = layer {
                                Some(raster)
                            } else {
                                None
                            }
                        }),
                    )
                    .unwrap();
                    let expected =
                        annotation_render::render_editing(&session, view, scale, None, None, None)
                            .unwrap();
                    assert!(
                        actual
                            .data()
                            .iter()
                            .zip(expected.data())
                            .all(|(a, b)| a.abs_diff(*b) <= 1),
                        "kind={kind:?} dash={dash} change={change}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "Release paired CPU centerline interpolation; excludes path construction/raster/GPU/App latency"]
fn curved_line_centerline_preparation_cpu_costs() {
    use lexift_core::domain::annotation::Endpoint;
    use std::{hint::black_box, time::Instant};
    if cfg!(debug_assertions) {
        panic!("run CPU measurement with --release");
    }
    let mut shape = object(Kind::Polyline);
    shape.extra = Extra::Polyline {
        curved: true,
        head: false,
        start: Endpoint::None,
        end: Endpoint::None,
    };
    shape.points = (0..10_000)
        .map(|i| (-30. + (i % 60) as f32, -20. + (i % 40) as f32))
        .collect();
    let reference = || {
        let mut prepared = PreparedObject::new(black_box(&shape));
        let _snapshot = prepared.snapshot(&shape);
        for (view, scale) in views() {
            black_box(clipped_bounds(
                prepared.bounds,
                shape.style.width,
                view,
                scale,
            ));
            black_box(shape.stroke_points());
        }
    };
    let shared = || {
        let mut prepared = PreparedObject::new(black_box(&shape));
        let _snapshot = prepared.snapshot(&shape);
        for (view, scale) in views() {
            black_box(clipped_bounds(
                prepared.bounds,
                shape.style.width,
                view,
                scale,
            ));
            black_box(prepared.line_points(&shape).unwrap());
        }
    };
    let mut check = PreparedObject::new(&shape);
    assert!(check.line_points.is_none());
    assert_eq!(check.line_points(&shape).unwrap(), shape.stroke_points());
    let address = check.line_points(&shape).unwrap().as_ptr();
    assert_eq!(address, check.line_points(&shape).unwrap().as_ptr());
    let mut old = Vec::new();
    let mut new = Vec::new();
    for round in 0..80 {
        for use_shared in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            if use_shared {
                shared();
            } else {
                reference();
            }
            if round >= 16 {
                let elapsed = start.elapsed().as_secs_f64() * 1000.;
                if use_shared {
                    new.push(elapsed);
                } else {
                    old.push(elapsed);
                }
            }
        }
    }
    let p95 = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[(values.len() as f64 * 0.95).ceil() as usize - 1]
    };
    eprintln!(
        "scope=cpu_centerline_interpolation_and_existing_bounds_snapshot_not_path_raster_gpu_app control_points=10000 views=2 reference_interpolations=3 shared_interpolations=2 reference_p95_ms={:.6} shared_p95_ms={:.6}",
        p95(old),
        p95(new)
    );
}

#[test]
#[ignore = "Release paired CPU measurement of cross-viewport bounds and snapshot preparation, not raster/GPU/App latency"]
fn cross_viewport_preparation_cpu_costs() {
    use std::{hint::black_box, time::Instant};
    if cfg!(debug_assertions) {
        panic!("run CPU measurement with --release");
    }
    let mut shape = object(Kind::Pencil);
    shape.points = (0..100_000)
        .map(|i| (-30. + (i % 60) as f32, -20. + (i % 40) as f32))
        .collect();
    let reference = || {
        views().map(|(view, scale)| {
            let bounds = content_bounds(black_box(&shape), view, scale);
            let snapshot = Arc::new(black_box(&shape).clone());
            black_box((bounds, snapshot))
        })
    };
    let shared = || {
        let mut prepared = PreparedObject::new(black_box(&shape));
        views().map(|(view, scale)| {
            let bounds = clipped_bounds(prepared.bounds, shape.style.width, view, scale);
            let snapshot = prepared.snapshot(&shape);
            black_box((bounds, snapshot))
        })
    };
    let expected = reference();
    let actual = shared();
    assert_eq!(expected[0].0, actual[0].0);
    assert_eq!(expected[1].0, actual[1].0);
    assert!(Arc::ptr_eq(&actual[0].1, &actual[1].1));
    assert!(!Arc::ptr_eq(&expected[0].1, &expected[1].1));
    let mut old = Vec::new();
    let mut new = Vec::new();
    for round in 0..80 {
        for use_shared in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            if use_shared {
                black_box(shared());
            } else {
                black_box(reference());
            }
            if round >= 16 {
                let elapsed = start.elapsed().as_secs_f64() * 1000.;
                if use_shared {
                    new.push(elapsed);
                } else {
                    old.push(elapsed);
                }
            }
        }
    }
    let p95 = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[(values.len() as f64 * 0.95).ceil() as usize - 1]
    };
    eprintln!(
        "scope=cpu_geometry_and_snapshot_preparation_not_raster_gpu_or_app points=100000 views=2 retained_reference_buffers=2 retained_shared_buffers=1 reference_p95_ms={:.6} shared_p95_ms={:.6}",
        p95(old),
        p95(new)
    );
}
