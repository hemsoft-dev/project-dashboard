use super::*;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "dashboard-config-atomic-73-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join(FILE_NAME)
    }
    fn assert_clean(&self) {
        let names: Vec<_> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE_NAME]);
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn view_saves_preserve_every_byte_outside_the_value() {
    for value in ["\"project\"", "'project'", "\"\"\"project\"\"\""] {
        let dir = Directory::new();
        let text = format!(
            "# preferences\r\n'view' = {value} # keep this\r\nrepositories = ['a']\r\nunknown = 'keep'\r\n[extra]\r\nview = 'unrelated'\r\n"
        );
        std::fs::write(dir.path(), &text).unwrap();
        for (view, name) in [
            (View::Fleet, "fleet"),
            (View::Backlog, "backlog"),
            (View::Project, "project"),
        ] {
            Config::save_view(&dir.0, view).unwrap();
            assert_eq!(
                std::fs::read_to_string(dir.path()).unwrap(),
                text.replacen(value, &format!("\"{name}\""), 1)
            );
            assert_eq!(Config::load(&dir.0).unwrap().view, view);
            dir.assert_clean();
        }
    }
}

#[test]
fn absent_view_is_added_before_tables_without_changing_existing_text() {
    let dir = Directory::new();
    let text = "# keep\nrepositories = ['a']\n[extra]\nview = 'keep'";
    std::fs::write(dir.path(), text).unwrap();
    Config::save_view(&dir.0, View::Backlog).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path()).unwrap(),
        format!("view = \"backlog\"\n{text}")
    );
    dir.assert_clean();
}

#[test]
fn invalid_view_input_is_not_overwritten() {
    let dir = Directory::new();
    for text in ["view = [", "view = 'unknown'", "repositories = 42"] {
        std::fs::write(dir.path(), text).unwrap();
        assert!(matches!(
            Config::save_view(&dir.0, View::Fleet),
            Err(ConfigError::Parse(_, _))
        ));
        assert_eq!(std::fs::read(dir.path()).unwrap(), text.as_bytes());
        dir.assert_clean();
    }
}

#[test]
fn partial_temporary_write_failure_preserves_original_and_cleans_temporary() {
    let dir = Directory::new();
    let original = b"# original\nrepositories = ['keep']\n";
    std::fs::write(dir.path(), original).unwrap();
    let result = replace_config_with(&dir.path(), b"replacement", |file, _| {
        file.write_all(b"partial")?;
        // Poll while the temporary file contains only a prefix of the new data.
        let mut poll = ConfigFile::new(&dir.0);
        assert!(poll.changed().is_none());
        assert_eq!(poll.changed().unwrap().unwrap().repositories, [PathBuf::from("keep")]);
        Err(std::io::Error::other("forced failure after partial write"))
    });
    assert!(matches!(result, Err(ConfigError::Write(path, _)) if path == dir.path()));
    assert_eq!(std::fs::read(dir.path()).unwrap(), original);
    dir.assert_clean();
}

#[test]
fn temporary_creation_failure_is_a_write_error() {
    let dir = Directory::new();
    let original = b"original bytes";
    std::fs::write(dir.path(), original).unwrap();
    // A file cannot be used as the temporary file's parent directory, even as root.
    let path = dir.path().join(FILE_NAME);
    assert!(matches!(replace_config(&path, b"new"), Err(ConfigError::Write(p, _)) if p == path));
    assert_eq!(std::fs::read(dir.path()).unwrap(), original);
    dir.assert_clean();
}

#[test]
fn replacement_failure_cleans_temporary_and_preserves_destination() {
    let dir = Directory::new();
    std::fs::create_dir(dir.path()).unwrap();
    let sentinel = dir.path().join("original");
    std::fs::write(&sentinel, b"keep").unwrap();
    assert!(matches!(Config::default().save(&dir.0), Err(ConfigError::Write(_, _))));
    assert_eq!(std::fs::read(sentinel).unwrap(), b"keep");
    dir.assert_clean();
}

#[test]
fn all_save_paths_replace_existing_files_and_poll_only_complete_configs() {
    let dir = Directory::new();
    let mut expected = Config {
        repositories: vec![PathBuf::from("original")],
        ..Default::default()
    };
    expected.save(&dir.0).unwrap();
    let mut poll = ConfigFile::new(&dir.0);
    assert!(poll.changed().is_none());
    assert_eq!(poll.changed().unwrap().unwrap(), expected);
    for i in 0..30 {
        expected.view = if i % 2 == 0 { View::Fleet } else { View::Backlog };
        expected.save(&dir.0).unwrap();
        assert!(poll.changed().is_none());
        assert_eq!(poll.changed().unwrap().unwrap(), expected);
        expected.view = View::Project;
        Config::save_view(&dir.0, View::Project).unwrap();
        assert!(poll.changed().is_none());
        assert_eq!(poll.changed().unwrap().unwrap(), expected);
        let checkout = PathBuf::from(format!("checkout-{i}"));
        Config::add_repository(&dir.0, &checkout).unwrap();
        expected.repositories.push(checkout);
        assert!(poll.changed().is_none());
        assert_eq!(poll.changed().unwrap().unwrap(), expected);
        assert!(poll.changed().is_none());
        dir.assert_clean();
    }
}

#[cfg(windows)]
#[test]
fn windows_deny_delete_handle_makes_every_save_fail_without_touching_original() {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
    let dir = Directory::new();
    Config::default().save(&dir.0).unwrap();
    let original = std::fs::read(dir.path()).unwrap();
    let mut poll = ConfigFile::new(&dir.0);
    assert!(poll.changed().is_none());
    assert!(poll.changed().unwrap().is_ok());
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(dir.path())
        .unwrap();
    for result in [
        Config::default().save(&dir.0),
        Config::save_view(&dir.0, View::Fleet),
        Config::add_repository(&dir.0, Path::new("new")).map(|_| ()),
    ] {
        assert!(matches!(result, Err(ConfigError::Write(_, _))));
        assert_eq!(std::fs::read(dir.path()).unwrap(), original);
        assert!(poll.changed().is_none());
        assert!(poll.changed().is_none());
        dir.assert_clean();
    }
    drop(held);
    Config::save_view(&dir.0, View::Fleet).unwrap();
    assert_eq!(Config::load(&dir.0).unwrap().view, View::Fleet);
    assert!(poll.changed().is_none());
    assert_eq!(poll.changed().unwrap().unwrap().view, View::Fleet);
    assert!(poll.changed().is_none());
    dir.assert_clean();
}
