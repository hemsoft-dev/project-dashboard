use dashboard_core::{placement::NativePlacement, ui_state::UiState};

#[test]
fn physical_workspace_bounds_survive_mixed_dpi_and_zoom_state_updates() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("fixtures/mixed-dpi-placement.json")).unwrap();
    let saved: NativePlacement = serde_json::from_value(fixture["nativePlacement"].clone()).unwrap();
    for system_dpi in fixture["systemDpis"].as_array().unwrap() {
        let dir = std::env::temp_dir().join(format!("dashboard-native-{}-{}", std::process::id(), system_dpi));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ui-state.json"), fixture.to_string()).unwrap();
        // Updating unrelated rem zoom must never convert the native rectangle.
        UiState::update(&dir, |state| state.zoom = 1.3).unwrap();
        let actual = UiState::load(&dir).native_placement.unwrap();
        assert_eq!(actual, saved);
        assert_eq!(actual.rect, [4738, 468, 6691, 1812]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
