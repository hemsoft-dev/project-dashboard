//! The Projects dialog's data (#34): pmd's projects, the client-side checks, the full-row `PUT`, linking a checkout to
//! a project without losing config.toml's comments, and pmd's answers against a loopback stub.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use dashboard_core::config::Config;
use dashboard_core::edit::{Form, Next};
use dashboard_core::projects::{self, FIELDS, LinkError, Project};
use dashboard_core::write::{Method, Precondition, WriteBackend, WriteError, WriteRequest};
use serde_json::{Value, json};

#[path = "support/stub.rs"]
mod stub;
use stub::{Dir, Reply, Stub};

fn project(overrides: Value) -> Value {
    let mut project = json!({"slug": "gh-x", "repo": "hemsoft-dev/gh-x", "priority": 2,
        "goal": "Maintenance: keep healthy, no new initiatives.", "notes": "", "requires": [], "tracked": true,
        "source": "portfolio", "version": 3});
    for (key, value) in overrides.as_object().unwrap() {
        project[key] = value.clone();
    }
    project
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

fn new_project() -> BTreeMap<String, String> {
    values(&[
        ("slug", "tickdown"),
        ("repo", "hemsoft-dev/tickdown"),
        ("priority", "3"),
        ("goal", " Ship the iOS widget "),
        ("notes", ""),
        ("requires", "apple"),
        ("tracked", "true"),
    ])
}

#[test]
fn projects_read_their_priority_tier_and_source() {
    let gh_x = Project::from_json(&project(json!({}))).unwrap();
    assert_eq!((gh_x.priority, gh_x.priority_label()), (Some(2), "P2".to_owned()));
    assert_eq!(gh_x.path(), "v1/projects/gh-x");
    let chief = Project::from_json(&project(
        json!({"slug": "chief", "repo": "hemsoft-dev/chief", "priority": null}),
    ))
    .unwrap();
    assert_eq!((chief.priority, chief.priority_label()), (None, "—".to_owned()));
    let row = chief.row();
    assert_eq!(row.values["priority"], "", "no tier edits as empty");
    assert_eq!(row.values["tracked"], "true");
    let unknown = Project::from_json(&project(json!({"priority": 9}))).unwrap();
    assert_eq!(unknown.priority_label(), "—", "an unknown tier shows as a dash");
    assert_eq!(Project::from_json(&json!({"slug": "x"})), Err(WriteError::Malformed));
    let list = Project::list_from_json(&json!({"projects": [project(json!({}))]})).unwrap();
    assert_eq!(list.len(), 1);
}

#[test]
fn the_list_sorts_by_priority_then_slug_with_no_tier_last_and_filters_by_text() {
    let mut list: Vec<Project> = [
        ("zeta", Some(1)),
        ("chief", None),
        ("alpha", Some(1)),
        ("gh-x", Some(0)),
    ]
    .into_iter()
    .map(|(slug, priority)| {
        Project::from_json(&project(
            json!({"slug": slug, "repo": format!("hemsoft-dev/{slug}"), "priority": priority}),
        ))
        .unwrap()
    })
    .collect();
    projects::sort(&mut list);
    let slugs: Vec<&str> = list.iter().map(|p| p.slug.as_str()).collect();
    assert_eq!(slugs, ["gh-x", "alpha", "zeta", "chief"]);
    assert!(list[0].matches("GH-X"));
    assert!(list[0].matches("maintenance"));
    assert!(!list[0].matches("tray"));
    assert!(list[0].matches("  "));
}

#[test]
fn a_body_carries_every_field_and_no_tier_is_null() {
    let (slug, body) = projects::body(&new_project()).unwrap();
    assert_eq!(slug, "tickdown");
    assert_eq!(
        body,
        json!({"repo": "hemsoft-dev/tickdown", "priority": 3, "goal": "Ship the iOS widget", "notes": "",
               "requires": ["apple"], "tracked": true})
    );
    let mut none = new_project();
    none.insert("priority".into(), String::new());
    assert_eq!(projects::body(&none).unwrap().1["priority"], Value::Null);
    assert_eq!(projects::slug_for(" hemsoft-dev/TickDown "), "tickdown");
}

#[test]
fn p5_must_be_untracked() {
    let mut dormant = new_project();
    dormant.insert("priority".into(), "5".into());
    let problems = projects::body(&dormant).unwrap_err();
    assert_eq!(problems[0].0, "priority");
    assert!(problems[0].1.starts_with("P5 is dormant"));
    dormant.insert("tracked".into(), "false".into());
    let (_, body) = projects::body(&dormant).unwrap();
    assert_eq!(
        (body["priority"].clone(), body["tracked"].clone()),
        (json!(5), json!(false))
    );
}

#[test]
fn checks_match_pmds() {
    let bad = values(&[
        ("slug", "other"),
        ("repo", "hemsoft-dev/tickdown"),
        ("priority", "7"),
        ("goal", &"g".repeat(1001)),
        ("notes", "two\nlines"),
        ("requires", "Apple"),
        ("tracked", "true"),
    ]);
    let fields: Vec<&str> = projects::body(&bad).unwrap_err().iter().map(|(f, _)| *f).collect();
    assert_eq!(fields, ["slug", "priority", "goal", "notes", "requires"]);
    let mut no_owner = new_project();
    no_owner.insert("repo".into(), "tickdown".into());
    assert_eq!(projects::body(&no_owner).unwrap_err()[0].0, "repo");
}

#[test]
fn pmds_repo_and_slug_errors_land_on_their_fields() {
    let unknown = WriteError::BadRequest {
        code: "unknown_repo".into(),
        message: "no repo".into(),
    };
    assert_eq!(projects::error_field(&unknown), Some("repo"));
    let hidden = WriteError::BadRequest {
        code: "repo_not_installed".into(),
        message: "can't see it".into(),
    };
    assert_eq!(projects::error_field(&hidden), Some("repo"));
    assert_eq!(projects::error_field(&WriteError::AlreadyExists), Some("slug"));
    assert_eq!(projects::error_field(&WriteError::StaleVersion), None);
}

#[test]
fn create_and_edit_send_the_whole_project_with_their_precondition() {
    let dir = Dir::new();
    let stub = Stub::new(|request| {
        if request.header("if-none-match") == Some("*") {
            Reply::json(
                201,
                project(json!({"slug": "tickdown", "repo": "hemsoft-dev/tickdown", "source": "api", "version": 1})),
            )
        } else {
            Reply::json(200, project(json!({"priority": 0, "source": "api", "version": 4})))
        }
    });
    let writer = dir.writer(&stub);
    let (slug, body) = projects::body(&new_project()).unwrap();
    writer
        .write(&WriteRequest {
            method: Method::Put,
            path: projects::path(&slug),
            precondition: Precondition::IfNoneMatch,
            body,
        })
        .unwrap();
    let gh_x = Project::from_json(&project(json!({}))).unwrap();
    let mut edit = Form::edit(gh_x.row(), &FIELDS);
    edit.set("priority", "0");
    let (slug, body) = projects::body(edit.values()).unwrap();
    let precondition = edit.begin_save().unwrap();
    let answer = writer
        .write(&WriteRequest {
            method: Method::Put,
            path: projects::path(&slug),
            precondition,
            body,
        })
        .unwrap();
    assert_eq!(Project::from_json(&answer).unwrap().priority, Some(0));
    let requests = stub.requests();
    assert_eq!(
        (requests[0].method.as_str(), requests[0].path.as_str()),
        ("PUT", "/v1/projects/tickdown")
    );
    assert_eq!(requests[1].header("if-match"), Some("3"));
    let sent: Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(sent["priority"], 0);
    assert_eq!(sent["repo"], "hemsoft-dev/gh-x");
}

#[test]
fn already_exists_unknown_repo_and_stale_versions_reach_the_form() {
    for (reply, field, expected) in [
        (
            Reply::error(409, "already_exists", "project exists (version 3)"),
            Some("slug"),
            Next::Show,
        ),
        (
            Reply::error(400, "unknown_repo", "hemsoft-dev/nope doesn't exist"),
            Some("repo"),
            Next::Show,
        ),
        (
            Reply::error(409, "stale_version", "If-Match is 3 but it is 4"),
            None,
            Next::Reread,
        ),
    ] {
        let dir = Dir::new();
        let reply = std::sync::Mutex::new(Some(reply));
        let stub = Stub::new(move |_| reply.lock().unwrap().take().unwrap());
        let mut edit = Form::create(&FIELDS, &projects::defaults());
        for (k, v) in new_project() {
            edit.set(&k, v);
        }
        let (slug, body) = projects::body(edit.values()).unwrap();
        let precondition = edit.begin_save().unwrap();
        let error = dir
            .writer(&stub)
            .write(&WriteRequest {
                method: Method::Put,
                path: projects::path(&slug),
                precondition,
                body,
            })
            .unwrap_err();
        let next = match projects::error_field(&error) {
            Some(field) => {
                edit.failed_on(field, &error);
                Next::Show
            }
            None => edit.failed(&error),
        };
        assert_eq!(next, expected);
        if let Some(field) = field {
            assert!(edit.field_error(field).is_some(), "{error:?}");
        }
    }
}

/// A temporary folder: a config dir and checkouts with a given origin.
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dashboard-projects-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn checkout(&self, name: &str, origin: Option<&str>) -> PathBuf {
        let checkout = self.0.join(name);
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        let config = origin.map_or(String::new(), |url| format!("[remote \"origin\"]\n\turl = {url}\n"));
        std::fs::write(checkout.join(".git/config"), config).unwrap();
        checkout
    }
    fn config(&self, text: &str) -> PathBuf {
        let dir = self.0.join("config");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), text).unwrap();
        dir
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("config.toml")).unwrap()
}

#[test]
fn linking_a_checkout_appends_it_and_keeps_every_comment() {
    let folder = Folder::new();
    let existing = folder.checkout("gh-x", Some("https://github.com/hemsoft-dev/gh-x.git"));
    let tickdown = folder.checkout("tickdown", Some("git@github.com:hemsoft-dev/tickdown.git"));
    let text = format!(
        "# My dashboard\nrepositories = [\n    # the CLI\n    {:?}, # trailing ]\n]\nrefresh_minutes = 5 # minutes\n",
        existing.to_string_lossy()
    );
    let dir = folder.config(&text);
    let written = projects::link_checkout(&dir, &tickdown, "hemsoft-dev/tickdown").unwrap();
    assert_eq!(written, tickdown);
    let after = read(&dir);
    assert!(
        after.starts_with("# My dashboard\nrepositories = [\n    # the CLI\n"),
        "{after}"
    );
    assert!(after.contains("# trailing ]"), "{after}");
    assert!(after.contains("refresh_minutes = 5 # minutes"), "{after}");
    assert!(
        after.contains(&format!("\n    {:?},\n]", tickdown.to_string_lossy())),
        "the new line takes the list's indent: {after}"
    );
    let config = Config::parse(&after, &dir.join("config.toml")).unwrap();
    assert_eq!(config.repositories, [existing, tickdown.clone()]);
    // Linking it again changes nothing.
    projects::link_checkout(&dir, &tickdown, "hemsoft-dev/tickdown").unwrap();
    assert_eq!(read(&dir), after);
}

#[test]
fn single_line_empty_and_missing_lists_are_edited_too() {
    let folder = Folder::new();
    let first = folder.checkout("a", Some("https://github.com/hemsoft-dev/a"));
    let second = folder.checkout("b", Some("https://github.com/hemsoft-dev/b"));
    for (text, already) in [
        (format!("repositories = [{:?}]\n", first.to_string_lossy()), 1),
        ("repositories = []\n".to_owned(), 0),
        ("parallelism = 2\n".to_owned(), 0),
        (format!("repositories = [ {:?}, ]\n", first.to_string_lossy()), 1),
    ] {
        let dir = folder.config(&text);
        assert!(Config::add_repository(&dir, &second).unwrap(), "{text}");
        let config = Config::parse(&read(&dir), &dir.join("config.toml")).unwrap();
        assert_eq!(config.repositories.len(), already + 1, "{text}");
        assert_eq!(config.repositories.last(), Some(&second), "{text}");
    }
}

#[test]
fn a_checkout_of_another_repo_or_none_is_not_linked_and_nothing_is_written() {
    let folder = Folder::new();
    let wrong = folder.checkout("fork", Some("https://github.com/someone/tickdown.git"));
    let bare = folder.checkout("bare", None);
    let dir = folder.config("repositories = []\n");
    match projects::link_checkout(&dir, &wrong, "hemsoft-dev/tickdown") {
        Err(error @ LinkError::Mismatch { .. }) => {
            assert_eq!(
                error.to_string(),
                "That folder's origin is someone/tickdown; nothing was written."
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        projects::link_checkout(&dir, &bare, "hemsoft-dev/tickdown"),
        Err(LinkError::Mismatch { found: None })
    ));
    assert_eq!(read(&dir), "repositories = []\n");
}
