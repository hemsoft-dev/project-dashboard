//! The config file and the status cache on disk.

use std::path::{Path, PathBuf};

use dashboard_core::cache::{self, StatusCache};
use dashboard_core::config::{self, Config, DEFAULT_PARALLELISM, DEFAULT_REFRESH_MINUTES};
use dashboard_core::fixtures::{self, ATTENTION, CLEAN};

/// A scratch folder for one test, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("project-dashboard-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn config_reads_repositories_and_defaults_the_rest() {
    let config = Config::parse(r#"repositories = ["/src/a", "/src/b"]"#, Path::new("config.toml")).unwrap();
    assert_eq!(config.repositories, [PathBuf::from("/src/a"), PathBuf::from("/src/b")]);
    assert_eq!(config.refresh_minutes, DEFAULT_REFRESH_MINUTES);
    assert_eq!(config.parallelism, DEFAULT_PARALLELISM);
    assert_eq!(config.refresh_interval(), Some(std::time::Duration::from_secs(300)));
}

#[test]
fn config_zero_refresh_turns_auto_refresh_off_and_parallelism_is_at_least_one() {
    let config = Config::parse("refresh_minutes = 0\nparallelism = 0", Path::new("config.toml")).unwrap();
    assert_eq!(config.refresh_interval(), None);
    assert_eq!(config.parallelism, 1);
}

#[test]
fn config_parallelism_is_capped() {
    let config = Config::parse("parallelism = 100000", Path::new("config.toml")).unwrap();
    assert_eq!(config.parallelism, dashboard_core::scheduler::MAX_PARALLELISM);
}

#[test]
fn a_huge_refresh_interval_is_capped_at_a_day_instead_of_overflowing() {
    let config = Config::parse("refresh_minutes = 4611686018427387904", Path::new("config.toml")).unwrap();
    assert_eq!(config.refresh_minutes, config::MAX_REFRESH_MINUTES);
    assert_eq!(
        config.refresh_interval(),
        Some(std::time::Duration::from_secs(24 * 60 * 60))
    );
    // Built directly (not parsed), the interval still can't overflow.
    let direct = Config {
        refresh_minutes: u64::MAX,
        ..Config::default()
    };
    assert_eq!(
        direct.refresh_interval(),
        Some(std::time::Duration::from_secs(24 * 60 * 60))
    );
}

#[test]
fn invalid_config_names_the_file() {
    let err = Config::parse("repositories = 3", Path::new("/cfg/config.toml")).unwrap_err();
    assert!(err.to_string().starts_with("/cfg/config.toml is not valid"), "{err}");
}

#[test]
fn first_run_discovers_checkouts_one_level_under_the_root_and_writes_them() {
    let scratch = Scratch::new("first-run");
    let root = scratch.path().join("github");
    for name in ["beta", "alpha"] {
        std::fs::create_dir_all(root.join(name).join(".git")).unwrap();
    }
    // Not checkouts: a plain folder, a linked worktree (a .git file) and a checkout two levels down.
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::create_dir_all(root.join("alpha.worktrees/pr-1")).unwrap();
    std::fs::write(
        root.join("alpha.worktrees/pr-1/.git"),
        "gitdir: ../../alpha/.git/worktrees/pr-1",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("group/deep/.git")).unwrap();
    let dir = scratch.path().join("config");

    let config = Config::load_or_create_in(&dir, root.clone()).unwrap();
    assert_eq!(config.repositories, [root.join("alpha"), root.join("beta")]);
    assert_eq!(config.root.as_deref(), Some(root.as_path()));

    // The second start reads the written file rather than scanning again.
    std::fs::create_dir_all(root.join("gamma/.git")).unwrap();
    let again = Config::load_or_create_in(&dir, root.clone()).unwrap();
    assert_eq!(again, config);
    assert!(dir.join(config::FILE_NAME).is_file());
}

#[test]
fn cache_round_trips_and_drops_removed_repositories() {
    let scratch = Scratch::new("cache");
    let mut cache = StatusCache::default();
    cache.insert("/src/gh-x".into(), fixtures::document(CLEAN));
    cache.insert("/src/codexbar".into(), fixtures::document(ATTENTION));
    cache.retain(&["/src/gh-x".into()]);
    cache.save(scratch.path()).unwrap();

    let loaded = StatusCache::load(scratch.path());
    assert_eq!(loaded, cache);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded.get("/src/gh-x".as_ref()), Some(&fixtures::document(CLEAN)));
    assert!(!scratch.path().join("status-cache.json.tmp").exists());
}

#[test]
fn a_missing_corrupt_or_other_version_cache_is_empty() {
    let scratch = Scratch::new("cache-bad");
    assert!(StatusCache::load(scratch.path()).is_empty());
    std::fs::write(scratch.path().join(cache::FILE_NAME), "{ not json").unwrap();
    assert!(StatusCache::load(scratch.path()).is_empty());
    std::fs::write(
        scratch.path().join(cache::FILE_NAME),
        r#"{"version": 99, "entries": {}}"#,
    )
    .unwrap();
    assert!(StatusCache::load(scratch.path()).is_empty());
}

#[test]
fn the_config_file_reports_each_edit_once() {
    use dashboard_core::config::ConfigFile;
    let scratch = Scratch::new("config-watch");
    let file = scratch.path().join(config::FILE_NAME);
    std::fs::write(&file, r#"repositories = ["/src/a"]"#).unwrap();
    let mut watch = ConfigFile::new(scratch.path());
    // Already read at startup: nothing to report until it changes.
    assert!(watch.changed().is_none());
    std::fs::write(&file, r#"repositories = ["/src/a", "/src/bb"]"#).unwrap();
    let config = watch.changed().expect("an edit").unwrap();
    assert_eq!(config.repositories, [PathBuf::from("/src/a"), PathBuf::from("/src/bb")]);
    assert!(watch.changed().is_none(), "reported once");
    // A same-length edit right after, which a timestamp-and-size check could miss.
    std::fs::write(&file, r#"repositories = ["/src/a", "/src/cc"]"#).unwrap();
    let config = watch.changed().expect("a same-length edit").unwrap();
    assert_eq!(config.repositories[1], PathBuf::from("/src/cc"));
    // An invalid edit is an error, not a panic, and is reported once too.
    std::fs::write(&file, "repositories = 3").unwrap();
    assert!(matches!(watch.changed(), Some(Err(config::ConfigError::Parse(..)))));
    assert!(watch.changed().is_none());
    // A deleted file is an error rather than a fresh, rediscovered config.
    std::fs::remove_file(&file).unwrap();
    assert!(matches!(watch.changed(), Some(Err(config::ConfigError::Read(..)))));
    assert!(!file.exists(), "nothing was written");
    // reload() reads the file whether or not it changed.
    std::fs::write(&file, r#"repositories = ["/src/c"]"#).unwrap();
    assert_eq!(watch.reload().unwrap().repositories, [PathBuf::from("/src/c")]);
    assert!(watch.changed().is_none());
}
