use dashboard_core::{
    columns::{Column, ColumnWidths},
    placement::Placement,
    ui_state::UiState,
};
fn saved() -> Placement {
    Placement {
        monitor: "second".into(),
        x: 80.,
        y: 60.,
        width: 800.,
        height: 600.,
        maximized: true,
    }
}
fn monitors() -> Vec<(String, f32, f32)> {
    vec![("primary".into(), 1920., 1080.), ("second".into(), 1280., 960.)]
}
#[test]
fn restores_monitor_normal_bounds_and_maximized_state() {
    assert_eq!(saved().restore(&monitors(), 0), Some((1, saved())));
}
#[test]
fn missing_monitor_uses_primary_and_clamps() {
    let mut p = saved();
    p.width = 3000.;
    p.x = 4000.;
    let (i, p) = p.restore(&monitors()[..1], 0).unwrap();
    assert_eq!(i, 0);
    assert_eq!((p.x, p.width), (0., 1920.));
}
#[test]
fn offscreen_monitor_uses_primary() {
    let mut p = saved();
    p.x = 9000.;
    let (i, p) = p.restore(&monitors(), 0).unwrap();
    assert_eq!(i, 0);
    assert_eq!(p.x, 1120.);
}
#[test]
fn changed_dpi_clamps_to_current_logical_work_area() {
    let mut m = monitors();
    m[1].1 = 640.;
    m[1].2 = 480.;
    let (_, p) = saved().restore(&m, 1).unwrap();
    assert_eq!((p.x, p.y, p.width, p.height), (0., 0., 640., 480.));
}
#[test]
fn malformed_geometry_is_rejected() {
    let mut p = saved();
    p.x = f32::NAN;
    assert!(p.restore(&monitors(), 0).is_none());
    assert!(saved().restore(&[], 0).is_none());
}
#[test]
fn columns_shrink_below_old_limits() {
    let mut widths = ColumnWidths::default();
    for (c, w) in [
        (Column::Priority, 3.),
        (Column::PullRequests, 1.75),
        (Column::Issues, 2.25),
        (Column::Merged24h, 4.5),
        (Column::Merged7d, 4.5),
        (Column::Updated, 3.5),
    ] {
        widths.set(c, w);
        assert_eq!(widths.get(c), w);
        widths.set(c, 0.);
        assert_eq!(widths.get(c), w);
    }
}
#[test]
fn placement_roundtrip_preserves_zoom_and_columns() {
    let dir = std::env::temp_dir().join(format!("dashboard-placement-{}", std::process::id()));
    UiState::update(&dir, |s| {
        s.zoom = 1.3;
        s.columns.set(Column::Priority, 3.);
    })
    .unwrap();
    UiState::update(&dir, |s| s.placement = Some(saved())).unwrap();
    let state = UiState::load(&dir);
    assert_eq!(state.zoom, 1.3);
    assert_eq!(state.columns.get(Column::Priority), 3.);
    assert_eq!(state.placement, Some(saved()));
    std::fs::remove_dir_all(dir).unwrap();
}
