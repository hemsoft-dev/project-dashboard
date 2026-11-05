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

#[test]
fn compatibility_in_place_helper_preserves_existing_file_and_write_errors() {
    let dir = Directory::new();
    Config::default().save(&dir.0).unwrap();
    #[cfg(windows)]
    let security = windows_security(&dir.path());
    #[cfg(target_os = "linux")]
    let metadata = std::fs::metadata(dir.path()).unwrap();
    write_config_in_place(&dir.path(), b"view = 'fleet'\n").unwrap();
    assert_eq!(Config::load(&dir.0).unwrap().view, View::Fleet);
    #[cfg(windows)]
    assert_eq!(windows_security(&dir.path()), security);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt as _;
        let after = std::fs::metadata(dir.path()).unwrap();
        assert_eq!(
            (after.ino(), after.mode(), after.uid(), after.gid()),
            (metadata.ino(), metadata.mode(), metadata.uid(), metadata.gid())
        );
    }
    let invalid = dir.path().join("missing");
    assert!(matches!(write_config_in_place(&invalid, b"new"), Err(ConfigError::Write(path, _)) if path == invalid));
    dir.assert_clean();
}

// A process can name itself or an owner-capable group as a file's owner, and nothing else. A UAC-filtered token holds
// Administrators as deny-only, so it can read an Administrators-owned file but can't create one.
#[cfg(windows)]
mod owner_seam {
    use super::*;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::FromRawHandle as _;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_SUCCESS, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
        SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACL, CreateRestrictedToken, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
        ImpersonateLoggedOnUser, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, RevertToSelf,
        SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_INFORMATION_CLASS,
        TOKEN_QUERY, TokenPrimaryGroup, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL};

    // Win32_System_Threading is not a dashboard-core feature, and tests alone need this one call.
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: HANDLE, access: u32, token: *mut HANDLE) -> i32;
    }

    fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        text.as_ref().encode_wide().chain(Some(0)).collect()
    }

    fn process_token() -> HANDLE {
        let mut token = null_mut();
        // SAFETY: -1 is the documented current-process pseudo handle, and token is a valid output.
        let opened = unsafe {
            OpenProcessToken(
                -1isize as HANDLE,
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY,
                &mut token,
            )
        };
        assert_ne!(opened, 0, "{}", std::io::Error::last_os_error());
        token
    }

    /// The string form of a SID the process token holds; each information class used here leads with a SID pointer.
    fn token_sid(class: TOKEN_INFORMATION_CLASS) -> String {
        let token = process_token();
        let mut buffer = vec![0u64; 128];
        let mut length = 0;
        // SAFETY: the buffer is aligned, its byte size is passed, and length is a valid output.
        let read = unsafe {
            GetTokenInformation(
                token,
                class,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                &mut length,
            )
        };
        assert_ne!(read, 0, "{}", std::io::Error::last_os_error());
        let sid = buffer[0] as usize as PSID;
        let mut text = null_mut();
        // SAFETY: sid points into buffer; LocalFree releases the returned text.
        assert_ne!(unsafe { ConvertSidToStringSidW(sid, &mut text) }, 0);
        // SAFETY: the API returned a terminated string; the text and the token handle are owned here.
        unsafe {
            let length = (0..).take_while(|&i| *text.add(i) != 0).count();
            let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text as _);
            CloseHandle(token);
            value
        }
    }

    pub(super) fn user() -> String {
        token_sid(TokenUser)
    }

    pub(super) fn primary_group() -> String {
        token_sid(TokenPrimaryGroup)
    }

    pub(super) fn descriptor(sddl: &str) -> PSECURITY_DESCRIPTOR {
        let text = wide(sddl);
        let mut descriptor = null_mut();
        // SAFETY: terminated SDDL and valid output; the caller releases the descriptor with LocalFree.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        assert_ne!(converted, 0, "{sddl}: {}", std::io::Error::last_os_error());
        descriptor
    }

    /// Replaces a fixture's DACL with a protected one, leaving its owner alone.
    pub(super) fn set_dacl(path: &Path, sddl: &str) {
        let descriptor = descriptor(sddl);
        let (mut present, mut defaulted, mut dacl): (i32, i32, *mut ACL) = (0, 0, null_mut());
        // SAFETY: valid descriptor and outputs; dacl points into descriptor, which outlives its use.
        let status = unsafe {
            assert_ne!(
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted),
                0
            );
            let status = SetNamedSecurityInfoW(
                wide(path).as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null(),
            );
            LocalFree(descriptor as _);
            status
        };
        assert_eq!(status, ERROR_SUCCESS, "{}", path.display());
    }

    /// CREATE_NEW with exactly this descriptor and the thread's effective token.
    pub(super) fn create_with(path: &Path, sddl: &str) -> std::io::Result<std::fs::File> {
        let descriptor = descriptor(sddl);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let path = wide(path);
        // SAFETY: path and descriptor stay valid for the call; a valid returned handle has no other owner.
        unsafe {
            let handle = CreateFileW(
                path.as_ptr(),
                GENERIC_WRITE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            );
            let result = if handle == INVALID_HANDLE_VALUE {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(std::fs::File::from_raw_handle(handle as _))
            };
            LocalFree(descriptor as _);
            result
        }
    }

    /// While alive, the current thread uses a copy of the process token with `deny_only` SIDs disabled and access
    /// limited by `restrict_to`. Reverts on drop, so a failing assert can't leak it into the next test on this thread.
    pub(super) struct Impersonation(HANDLE);

    impl Impersonation {
        pub(super) fn begin(deny_only: &[&str], restrict_to: &[&str]) -> Self {
            let sids = |list: &[&str]| -> Vec<SID_AND_ATTRIBUTES> {
                list.iter()
                    .map(|sid| {
                        let mut value = null_mut();
                        // SAFETY: terminated text and valid output; released below, after the token exists.
                        let converted = unsafe { ConvertStringSidToSidW(wide(sid).as_ptr(), &mut value) };
                        assert_ne!(converted, 0, "{sid}");
                        SID_AND_ATTRIBUTES {
                            Sid: value,
                            Attributes: 0,
                        }
                    })
                    .collect()
            };
            let (disable, restrict) = (sids(deny_only), sids(restrict_to));
            let source = process_token();
            let mut token = null_mut();
            // SAFETY: both arrays and their counts are valid for the call, and token is a valid output.
            let created = unsafe {
                CreateRestrictedToken(
                    source,
                    0,
                    disable.len() as u32,
                    disable.as_ptr(),
                    0,
                    null(),
                    restrict.len() as u32,
                    restrict.as_ptr(),
                    &mut token,
                )
            };
            let error = std::io::Error::last_os_error();
            // SAFETY: the SIDs came from ConvertStringSidToSidW and the handle from OpenProcessToken.
            unsafe {
                for sid in disable.iter().chain(&restrict) {
                    LocalFree(sid.Sid as _);
                }
                CloseHandle(source);
            }
            assert_ne!(created, 0, "{error}");
            // SAFETY: token is a valid primary token for this user; RevertToSelf ends it in Drop.
            let impersonating = unsafe { ImpersonateLoggedOnUser(token) };
            assert_ne!(impersonating, 0, "{}", std::io::Error::last_os_error());
            Self(token)
        }
    }

    impl Drop for Impersonation {
        fn drop(&mut self) {
            // SAFETY: reverts this thread's own impersonation and closes the handle this guard owns.
            unsafe {
                RevertToSelf();
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
mod unassignable_owner {
    use super::owner_seam::{Impersonation, create_with, descriptor, primary_group, set_dacl, user};
    use super::*;

    const ADMINISTRATORS: &str = "S-1-5-32-544";
    /// What Windows reports for an owner it won't assign: "This security ID may not be assigned as the owner".
    const INVALID_OWNER: i32 = 1307;

    /// The descriptor shape read from the reported config: an owner the logged-in user may not hold, inherited ACEs,
    /// and full control for that user.
    fn snapshot(owner: &str, destination: &Path) -> ConfigSecurity {
        snapshot_with_group(owner, &primary_group(), destination)
    }

    fn snapshot_with_group(owner: &str, group: &str, destination: &Path) -> ConfigSecurity {
        let user = user();
        ConfigSecurity::from_descriptor(
            descriptor(&format!(
                "O:{owner}G:{group}D:AI(A;ID;FA;;;SY)(A;ID;FA;;;BA)(A;ID;FA;;;{user})"
            )),
            destination,
        )
    }

    /// The group and DACL, without the owner.
    fn group_and_dacl(path: &Path) -> String {
        let security = windows_security(path);
        security.split_once("G:").unwrap().1.to_owned()
    }

    fn owner(path: &Path) -> String {
        let security = windows_security(path);
        security.split_once("G:").unwrap().0.to_owned()
    }

    fn save(security: &ConfigSecurity, path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
        replace_config_using(security, path, bytes, |file, bytes| file.write_all(bytes))
    }

    fn os_error(result: Result<(), ConfigError>) -> Option<i32> {
        match result {
            Err(ConfigError::Write(_, err)) => err.raw_os_error(),
            other => panic!("expected a write error, got {other:?}"),
        }
    }

    #[test]
    fn a_filtered_token_replaces_a_config_snapshotted_with_an_administrators_owner() {
        let (dir, control) = (Directory::new(), Directory::new());
        std::fs::write(dir.path(), "view = 'project'\n").unwrap();
        std::fs::write(control.path(), "view = 'project'\n").unwrap();
        let mut poll = ConfigFile::new(&dir.0);
        assert!(poll.changed().is_none());
        assert_eq!(poll.changed().unwrap().unwrap().view, View::Project);

        let _filtered = Impersonation::begin(&[ADMINISTRATORS], &[]);
        let replacement = b"# new\nview = 'fleet'\n";
        save(&snapshot("BA", &dir.path()), &dir.path(), replacement).unwrap();
        // The same snapshot with an owner Windows accepts: only the owner may differ.
        save(&snapshot(&user(), &control.path()), &control.path(), replacement).unwrap();

        assert_eq!(std::fs::read(dir.path()).unwrap(), replacement);
        assert_eq!(group_and_dacl(&dir.path()), group_and_dacl(&control.path()));
        // SDDL prints an alias for some accounts, so compare what Windows reports: the owner ends up as the user.
        assert_eq!(owner(&dir.path()), owner(&control.path()));
        assert_ne!(owner(&dir.path()), "O:BA");
        assert!(poll.changed().is_none());
        assert_eq!(poll.changed().unwrap().unwrap().view, View::Fleet);
        dir.assert_clean();
    }

    #[test]
    fn the_snapshot_group_survives_when_the_owner_cannot() {
        let dir = Directory::new();
        std::fs::write(
            dir.path(),
            "view = 'project'
",
        )
        .unwrap();
        let _filtered = Impersonation::begin(&[ADMINISTRATORS], &[]);
        // SYSTEM is not the token's own primary group, so a dropped group would show.
        let security = snapshot_with_group("BA", "SY", &dir.path());
        save(
            &security,
            &dir.path(),
            b"view = 'fleet'
",
        )
        .unwrap();
        assert!(group_and_dacl(&dir.path()).starts_with("SYD:"));
    }

    #[test]
    fn an_owner_it_cannot_assign_is_still_refused_without_the_right_to_change_the_dacl() {
        let dir = Directory::new();
        let original = b"view = 'project'\n";
        std::fs::write(dir.path(), original).unwrap();
        // Read, write and delete, but no WRITE_DAC: OWNER RIGHTS removes the owner's implicit one. An elevated host owns
        // this file as Administrators, which the filtered token below can't use, so the user needs its own entry.
        set_dacl(&dir.path(), &format!("D:P(A;;0x1301bf;;;OW)(A;;0x1301bf;;;{})", user()));
        let security_before = windows_security(&dir.path());

        let _filtered = Impersonation::begin(&[ADMINISTRATORS], &[]);
        let result = save(&snapshot("BA", &dir.path()), &dir.path(), b"view = 'fleet'\n");

        assert_eq!(os_error(result), Some(INVALID_OWNER));
        assert_eq!(std::fs::read(dir.path()).unwrap(), original);
        assert_eq!(windows_security(&dir.path()), security_before);
        dir.assert_clean();
    }

    #[test]
    fn a_failed_replacement_past_the_owner_keeps_the_original_bytes_and_security() {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let dir = Directory::new();
        let original = b"view = 'project'\n";
        std::fs::write(dir.path(), original).unwrap();
        let security_before = windows_security(&dir.path());
        let _filtered = Impersonation::begin(&[ADMINISTRATORS], &[]);
        let security = snapshot("BA", &dir.path());

        // The write fails after a partial prefix, with polling running against the file meanwhile.
        let mut poll = ConfigFile::new(&dir.0);
        assert!(poll.changed().is_none());
        assert!(poll.changed().unwrap().is_ok());
        let result = replace_config_using(&security, &dir.path(), b"view = 'fleet'\n", |file, _| {
            file.write_all(b"view = ")?;
            assert!(poll.changed().is_none());
            Err(std::io::Error::other("forced failure after partial write"))
        });
        assert!(matches!(result, Err(ConfigError::Write(_, err)) if err.to_string().contains("forced failure")));
        assert_eq!(std::fs::read(dir.path()).unwrap(), original);
        assert_eq!(windows_security(&dir.path()), security_before);
        dir.assert_clean();

        // The rename fails: this handle doesn't share delete access.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(dir.path())
            .unwrap();
        let result = save(&security, &dir.path(), b"view = 'fleet'\n");
        assert_ne!(os_error(result), Some(INVALID_OWNER));
        drop(held);
        assert_eq!(std::fs::read(dir.path()).unwrap(), original);
        assert_eq!(windows_security(&dir.path()), security_before);
        assert!(poll.changed().is_none());
        dir.assert_clean();
    }

    /// The reported state in full, which only an elevated process can set up: a real file owned by Administrators,
    /// saved with Administrators deny-only, as under UAC. GitHub's Windows runners are elevated. A non-elevated host
    /// gets no fixture and skips; the snapshot tests above still run there, on its real filtered token.
    #[test]
    fn every_save_replaces_a_real_administrators_owned_config_under_a_filtered_token() {
        type Save = Box<dyn Fn(&Path) -> Result<(), ConfigError>>;
        let user = user();
        let text = "# keep\nview = 'project'\nrepositories = []\n";
        let sddl = format!("O:BAD:P(A;;FA;;;{user})(A;;FA;;;SY)");
        let saves: [(&str, Save); 3] = [
            ("save_view", Box::new(|dir| Config::save_view(dir, View::Fleet))),
            (
                "add_repository",
                Box::new(|dir| Config::add_repository(dir, Path::new("a")).map(|_| ())),
            ),
            ("save", Box::new(|dir| Config::default().save(dir))),
        ];
        for (name, entry_point) in saves {
            let dir = Directory::new();
            match create_with(&dir.path(), &sddl) {
                Ok(mut file) => file.write_all(text.as_bytes()).unwrap(),
                Err(err) if err.raw_os_error() == Some(INVALID_OWNER) => {
                    assert!(
                        std::env::var_os("GITHUB_ACTIONS").is_none(),
                        "CI must run this test elevated: {err}"
                    );
                    eprintln!("skipped: creating an Administrators-owned fixture needs an elevated process");
                    return;
                }
                Err(err) => panic!("{err}"),
            }
            let before = windows_security(&dir.path());
            assert!(before.starts_with("O:BA"), "{before}");
            let dacl = before.split_once("D:").unwrap().1.to_owned();
            let mut poll = ConfigFile::new(&dir.0);
            assert!(poll.changed().is_none());
            assert!(poll.changed().unwrap().is_ok());

            let _filtered = Impersonation::begin(&[ADMINISTRATORS], &[]);
            // The token now behaves as the reported one: it can't name Administrators as an owner.
            let probe = dir.0.join("probe");
            assert_eq!(
                create_with(&probe, &sddl).unwrap_err().raw_os_error(),
                Some(INVALID_OWNER)
            );

            entry_point(&dir.0).unwrap_or_else(|err| panic!("{name}: {err}"));

            let after = windows_security(&dir.path());
            assert_ne!(std::fs::read_to_string(dir.path()).unwrap(), text, "{name}");
            assert_eq!(after.split_once("D:").unwrap().1, dacl, "{name}");
            assert!(!after.starts_with("O:BA"), "{name}: {after}");
            assert!(poll.changed().is_none());
            assert!(poll.changed().unwrap().is_ok());
            dir.assert_clean();
        }
    }
}
