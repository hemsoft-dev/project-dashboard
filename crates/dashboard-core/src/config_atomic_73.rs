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

#[test]
fn adding_view_preserves_crlf_style() {
    let dir = Directory::new();
    let text = "# keep\r\nrepositories = ['a']\r\n";
    std::fs::write(dir.path(), text).unwrap();
    Config::save_view(&dir.0, View::Fleet).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path()).unwrap(),
        format!("view = \"fleet\"\r\n{text}")
    );
}

#[cfg(windows)]
#[test]
fn saves_preserve_restricted_windows_dacl_and_new_files_are_owner_only() {
    let dir = Directory::new();
    crate::write::store_token(&dir.path(), "fixture-token").unwrap();
    std::fs::write(dir.path(), "view = 'project'").unwrap();
    let original = crate::write::token_file_sddl(&dir.path()).unwrap();
    let original_security = windows_security(&dir.path());
    for result in [
        Config::save_view(&dir.0, View::Fleet),
        Config::add_repository(&dir.0, Path::new("a")).map(|_| ()),
        Config::default().save(&dir.0),
    ] {
        result.unwrap();
        assert_eq!(crate::write::token_file_sddl(&dir.path()).unwrap(), original);
        assert_eq!(windows_security(&dir.path()), original_security);
        dir.assert_clean();
    }
    let new_dir = Directory::new();
    Config::default().save(&new_dir.0).unwrap();
    assert_eq!(crate::write::token_file_sddl(&new_dir.path()).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn saves_preserve_unix_mode_and_owner_and_new_files_are_private() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let dir = Directory::new();
    Config::default().save(&dir.0).unwrap();
    assert_eq!(std::fs::metadata(dir.path()).unwrap().mode() & 0o777, 0o600);
    for mode in [0o600, 0o640] {
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap();
        let old = std::fs::metadata(dir.path()).unwrap();
        Config::save_view(&dir.0, View::Fleet).unwrap();
        let new = std::fs::metadata(dir.path()).unwrap();
        assert_eq!((new.mode(), new.uid(), new.gid()), (old.mode(), old.uid(), old.gid()));
    }
}

#[cfg(windows)]
fn windows_security(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt as _;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let information = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut descriptor = null_mut();
    // SAFETY: terminated path and valid output; LocalFree releases the descriptor below.
    let status = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            information,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(status, ERROR_SUCCESS);
    let mut text = null_mut();
    let mut len = 0;
    // SAFETY: valid descriptor and output pointers; LocalFree releases the returned text.
    let result = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            information,
            &mut text,
            &mut len,
        )
    };
    let error = std::io::Error::last_os_error();
    let value = if result != 0 {
        // SAFETY: API returned len UTF-16 units, including the terminator.
        Some(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(text, len as usize - 1)
        }))
    } else {
        None
    };
    // SAFETY: both pointers were allocated by the security APIs above; LocalFree accepts null.
    unsafe {
        LocalFree(text as _);
        LocalFree(descriptor as _);
    }
    value.unwrap_or_else(|| panic!("security descriptor conversion failed: {error}"))
}

#[cfg(target_os = "linux")]
fn named_acl(uid: u32, named_permissions: u16) -> Vec<u8> {
    // Linux POSIX ACL xattr format: version, then tag/permissions/id entries.
    let mut acl = 2u32.to_le_bytes().to_vec();
    for (tag, permissions, id) in [
        (1u16, 6u16, u32::MAX),
        (2, named_permissions, uid),
        (4, 4, u32::MAX),
        (16, 6, u32::MAX),
        (32, 0, u32::MAX),
    ] {
        acl.extend(tag.to_le_bytes());
        acl.extend(permissions.to_le_bytes());
        acl.extend(id.to_le_bytes());
    }
    acl
}

#[cfg(target_os = "linux")]
#[test]
fn linux_named_acl_is_preserved_and_parent_acl_does_not_broaden_new_files() {
    use rustix::fs::{XattrFlags, setxattr};
    use std::os::unix::fs::MetadataExt as _;
    let dir = Directory::new();
    let uid = std::fs::metadata(&dir.0).unwrap().uid();
    let named_uid = if uid == 65534 { 65533 } else { 65534 };
    let original_acl = named_acl(named_uid, 0);
    // The parent would grant that same named user read/write access to newly created files.
    setxattr(
        &dir.0,
        "system.posix_acl_default",
        &named_acl(named_uid, 6),
        XattrFlags::empty(),
    )
    .unwrap();
    Config::default().save(&dir.0).unwrap();
    assert_eq!(read_config_acl(&dir.path()).unwrap(), None);
    assert_eq!(std::fs::metadata(dir.path()).unwrap().mode() & 0o777, 0o600);
    setxattr(dir.path(), CONFIG_ACL, &original_acl, XattrFlags::empty()).unwrap();
    let before = std::fs::metadata(dir.path()).unwrap();
    // Negative control: the old default-permission temporary-and-rename drops the named deny entry.
    let legacy = dir.0.join("legacy.tmp");
    std::fs::write(&legacy, Config::default().to_toml()).unwrap();
    std::fs::rename(&legacy, dir.path()).unwrap();
    assert_ne!(read_config_acl(&dir.path()).unwrap().unwrap(), original_acl);
    setxattr(dir.path(), CONFIG_ACL, &original_acl, XattrFlags::empty()).unwrap();
    for result in [
        Config::save_view(&dir.0, View::Fleet),
        Config::add_repository(&dir.0, Path::new("a")).map(|_| ()),
        Config::default().save(&dir.0),
    ] {
        result.unwrap();
        let after = std::fs::metadata(dir.path()).unwrap();
        assert_eq!(
            (after.mode(), after.uid(), after.gid()),
            (before.mode(), before.uid(), before.gid())
        );
        assert_eq!(read_config_acl(&dir.path()).unwrap().unwrap(), original_acl);
        dir.assert_clean();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn invalid_acl_application_fails_without_replacement_and_cleans_temporary() {
    let dir = Directory::new();
    Config::default().save(&dir.0).unwrap();
    let original = std::fs::read(dir.path()).unwrap();
    let original_metadata = std::fs::metadata(dir.path()).unwrap();
    let result = replace_config_with(&dir.path(), b"new", |file, _| {
        ConfigSecurity {
            metadata: Some(original_metadata),
            acl: Some(vec![255]),
        }
        .apply(file)
    });
    assert!(matches!(result, Err(ConfigError::Write(_, _))));
    assert_eq!(std::fs::read(dir.path()).unwrap(), original);
    assert_eq!(read_config_acl(&dir.path()).unwrap(), None);
    dir.assert_clean();
}
