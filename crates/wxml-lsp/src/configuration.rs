//! Explicit workspace-relative Mini Program configuration.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use wxml_project::ProjectPath;

/// Startup options sent by an editor as `initialize.initializationOptions`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializationOptions {
    /// Mini Programs in this workspace; empty means the workspace root.
    #[serde(default)]
    pub apps: Vec<AppConfig>,
    /// Optional workspace snippet configuration path.
    #[serde(default)]
    pub snippets_path: Option<String>,
    /// Optional switch for all snippets.
    #[serde(default)]
    pub snippets_enabled: Option<bool>,
    /// Style/resource settings consumed by the project host object.
    ///
    /// The values are validated by the style layer so one invalid entry does
    /// not reject otherwise valid application roots.
    #[serde(default)]
    pub wxml: Option<Value>,
    /// Alias for [`Self::wxml`] accepted for compatibility with early builds.
    #[serde(default)]
    pub style: Option<Value>,
    /// Optional switches for semantic tools.
    #[serde(default)]
    pub semantic_tools: Option<SemanticToolsOptions>,
}

/// Client-configurable switches for the semantic tool handlers.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SemanticToolsOptions {
    /// `false` disables inlay hints; omitted keeps them enabled.
    #[serde(default)]
    pub inlay_hints: Option<bool>,
    /// `false` disables code lens; omitted keeps them enabled.
    #[serde(default)]
    pub code_lens: Option<bool>,
}

/// One Mini Program, with no filesystem-based root inference.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppConfig {
    /// Workspace-relative directory containing `app.json`; `.` means workspace.
    pub root: String,
    /// Workspace-relative TypeScript project directory; defaults to `root`.
    pub project_root: Option<String>,
}

/// Invalid startup configuration, with no user paths in the message.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ConfigurationError {
    /// The JSON shape, number of apps or relative path is invalid.
    #[error(
        "invalid initializationOptions: expected apps (at most 64) with root and optional projectRoot; paths must be workspace-relative or '.' (at most 4096 bytes)"
    )]
    InvalidOptions,
    /// An explicit application requires a workspace to resolve its paths.
    #[error("configured apps require a workspace root URI")]
    MissingWorkspace,
    /// Configured directories must exist and remain within their boundaries.
    #[error(
        "invalid app configuration: directories must exist inside the workspace and root must be inside projectRoot"
    )]
    InvalidRoot,
    /// Two entries name the same canonical app directory.
    #[error("invalid app configuration: duplicate app roots")]
    DuplicateRoot,
}

/// One explicitly resolved app and its backend project boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppRoot {
    pub(crate) app_dir: PathBuf,
    pub(crate) project_root: PathBuf,
    pub(crate) app_json: ProjectPath,
}

impl InitializationOptions {
    /// Parses and validates JSON shape and relative paths before opening files.
    pub fn parse(value: &Value) -> Result<Self, ConfigurationError> {
        let options: Self = if value.is_null() {
            Self::default()
        } else {
            if !value.is_object()
                || value
                    .get("apps")
                    .and_then(Value::as_array)
                    .is_some_and(|apps| apps.iter().any(|app| !app.is_object()))
            {
                return Err(ConfigurationError::InvalidOptions);
            }
            serde_json::from_value(value.clone()).map_err(|_| ConfigurationError::InvalidOptions)?
        };
        options.validate()?;
        Ok(options)
    }

    fn validate(&self) -> Result<(), ConfigurationError> {
        if self.apps.len() > 64 {
            return Err(ConfigurationError::InvalidOptions);
        }
        for app in &self.apps {
            relative_directory(&app.root)?;
            if let Some(project_root) = &app.project_root {
                relative_directory(project_root)?;
            }
        }
        Ok(())
    }

    pub(crate) fn resolve(&self, workspace: &Path) -> Result<Vec<AppRoot>, ConfigurationError> {
        self.validate()?;
        let workspace =
            std::fs::canonicalize(workspace).map_err(|_| ConfigurationError::InvalidRoot)?;
        let default_app = AppConfig {
            root: ".".to_owned(),
            project_root: None,
        };
        let apps = if self.apps.is_empty() {
            std::slice::from_ref(&default_app)
        } else {
            &self.apps
        };
        let mut roots = Vec::with_capacity(apps.len());
        for app in apps {
            let app_dir = resolve_directory(&workspace, &app.root)?;
            let project_root =
                resolve_directory(&workspace, app.project_root.as_deref().unwrap_or(&app.root))?;
            let prefix = app_dir
                .strip_prefix(&project_root)
                .map_err(|_| ConfigurationError::InvalidRoot)?;
            let app_json = prefix.join("app.json");
            let app_json = ProjectPath::new(app_json.to_string_lossy().into_owned())
                .map_err(|_| ConfigurationError::InvalidRoot)?;
            roots.push(AppRoot {
                app_dir,
                project_root,
                app_json,
            });
        }
        roots.sort_by(|left, right| left.app_dir.cmp(&right.app_dir));
        if roots
            .windows(2)
            .any(|pair| pair[0].app_dir == pair[1].app_dir)
        {
            return Err(ConfigurationError::DuplicateRoot);
        }
        Ok(roots)
    }
}

fn relative_directory(raw: &str) -> Result<PathBuf, ConfigurationError> {
    if raw == "." || raw == "./" {
        return Ok(PathBuf::new());
    }
    if raw.len() > 4096 {
        return Err(ConfigurationError::InvalidOptions);
    }
    ProjectPath::new(raw)
        .map(|path| PathBuf::from(path.as_str()))
        .map_err(|_| ConfigurationError::InvalidOptions)
}

fn resolve_directory(workspace: &Path, raw: &str) -> Result<PathBuf, ConfigurationError> {
    let directory = std::fs::canonicalize(workspace.join(relative_directory(raw)?))
        .map_err(|_| ConfigurationError::InvalidRoot)?;
    if !directory.starts_with(workspace) || !directory.is_dir() {
        return Err(ConfigurationError::InvalidRoot);
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Component;
    use std::sync::atomic::{AtomicU64, Ordering};

    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    #[derive(Debug)]
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            loop {
                let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
                let path = Path::new("/tmp/opencode").join(format!(
                    "wxml-configuration-{}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => {
                        let mut temp = Self(path);
                        temp.0 = fs::canonicalize(&temp.0).expect("canonical temporary directory");
                        return temp;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("create temporary directory: {error}"),
                }
            }
        }

        fn directory(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).expect("create fixture directory");
            path
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            fs::write(path, text).expect("write fixture");
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn resolve(value: &Value, workspace: &Path) -> Result<Vec<AppRoot>, ConfigurationError> {
        InitializationOptions::parse(value)?.resolve(workspace)
    }

    fn assert_invalid_path(raw: &str) {
        for field in ["root", "projectRoot"] {
            let mut app = json!({"root": "."});
            app[field] = json!(raw);
            assert_eq!(
                InitializationOptions::parse(&json!({"apps": [app]})).unwrap_err(),
                ConfigurationError::InvalidOptions,
                "{field}: {raw:?}"
            );
        }
    }

    #[test]
    fn absent_options_use_workspace_root_without_scanning_descendants() {
        let temp = TestDir::new();
        let workspace = temp.directory("workspace");
        temp.write("workspace/apps/first/src/app.json", "{}");
        temp.write("workspace/apps/second/app.json", "{}");
        temp.write("tsconfig.json", "{}");
        assert!(!workspace.join("app.json").exists());

        for value in [json!(null), json!({}), json!({"apps": []})] {
            let roots = resolve(&value, &workspace).expect("default roots");
            assert_eq!(roots.len(), 1);
            assert_eq!(roots[0].app_dir, workspace);
            assert_eq!(roots[0].project_root, workspace);
            assert_eq!(roots[0].app_json.as_str(), "app.json");
        }
    }

    #[test]
    fn project_root_defaults_to_app_root_without_searching_for_tsconfig() {
        let temp = TestDir::new();
        let app = temp.directory("apps/miniprogram/src");
        temp.write("tsconfig.json", "{}");
        temp.write("apps/miniprogram/tsconfig.json", "{}");
        temp.write("apps/miniprogram/src/app.json", "{}");

        let roots = resolve(
            &json!({"apps": [{"root": "apps/miniprogram/src"}]}),
            &temp.0,
        )
        .expect("explicit app");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].app_dir, app);
        assert_eq!(roots[0].project_root, app);
        assert_eq!(roots[0].app_json.as_str(), "app.json");
    }

    #[test]
    fn explicit_parent_project_root_wins_over_a_closer_tsconfig() {
        let temp = TestDir::new();
        let app = temp.directory("apps/miniprogram/src");
        temp.write("apps/miniprogram/tsconfig.json", "{}");
        temp.write("apps/miniprogram/src/tsconfig.json", "{}");
        temp.write("apps/miniprogram/src/app.json", "{}");
        let roots = resolve(
            &json!({"apps": [{"root": "apps/miniprogram/src", "projectRoot": "apps/miniprogram"}]}),
            &temp.0,
        )
        .expect("explicit parent TypeScript directory");
        assert_eq!(roots[0].app_dir, app);
        assert_eq!(roots[0].project_root, temp.0.join("apps/miniprogram"));
        assert_eq!(roots[0].app_json.as_str(), "src/app.json");
    }

    #[test]
    fn project_root_dot_is_relative_to_workspace_not_the_app() {
        let temp = TestDir::new();
        let app = temp.directory("apps/miniprogram/src");
        let roots = resolve(
            &json!({"apps": [{"root": "apps/miniprogram/src", "projectRoot": "."}]}),
            &temp.0,
        )
        .expect("workspace project root");
        assert_eq!(roots[0].app_dir, app);
        assert_eq!(roots[0].project_root, temp.0);
        assert_eq!(roots[0].app_json.as_str(), "apps/miniprogram/src/app.json");
    }

    #[test]
    fn null_project_root_has_the_same_meaning_as_omitting_it() {
        let temp = TestDir::new();
        temp.directory("app");
        let omitted = resolve(&json!({"apps": [{"root": "app"}]}), &temp.0).expect("omitted");
        let null = resolve(
            &json!({"apps": [{"root": "app", "projectRoot": null}]}),
            &temp.0,
        )
        .expect("null projectRoot");
        assert_eq!(null, omitted);
    }

    #[test]
    fn relative_separators_and_leading_dot_are_normalized() {
        let temp = TestDir::new();
        let app = temp.directory("apps/miniprogram/src");
        for root in [
            "apps/miniprogram/src",
            "./apps/miniprogram/src",
            "././apps/miniprogram/src",
            r"apps\miniprogram\src",
            r".\apps\miniprogram\src",
        ] {
            let roots = resolve(
                &json!({"apps": [{"root": root, "projectRoot": r"apps\miniprogram"}]}),
                &temp.0,
            )
            .expect("normalized relative directory");
            assert_eq!(roots[0].app_dir, app, "{root:?}");
            assert_eq!(roots[0].project_root, temp.0.join("apps/miniprogram"));
            assert_eq!(roots[0].app_json.as_str(), "src/app.json");
        }
        for root in [".", "./"] {
            let roots = resolve(&json!({"apps": [{"root": root}]}), &temp.0)
                .expect("explicit workspace root");
            assert_eq!(roots[0].app_dir, temp.0);
            assert_eq!(roots[0].project_root, temp.0);
        }
    }

    #[test]
    fn multiple_apps_may_share_a_project_root() {
        let temp = TestDir::new();
        temp.directory("apps/first/src");
        temp.directory("apps/second/src");
        let roots = resolve(
            &json!({"apps": [
                {"root": "apps/second/src", "projectRoot": "apps"},
                {"root": "apps/first/src", "projectRoot": "apps"}
            ]}),
            &temp.0,
        )
        .expect("distinct apps sharing one project directory");
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].project_root, temp.0.join("apps"));
        assert_eq!(roots[1].project_root, temp.0.join("apps"));
        let mut app_json: Vec<_> = roots.iter().map(|root| root.app_json.as_str()).collect();
        app_json.sort_unstable();
        assert_eq!(app_json, ["first/src/app.json", "second/src/app.json"]);
    }

    #[test]
    fn duplicate_canonical_roots_are_rejected() {
        let temp = TestDir::new();
        temp.directory("apps/mini");
        for (left, right) in [
            (".", "./"),
            ("apps/mini", "./apps/mini"),
            ("apps/mini", r"apps\mini"),
        ] {
            assert_eq!(
                resolve(&json!({"apps": [{"root": left}, {"root": right}]}), &temp.0,),
                Err(ConfigurationError::DuplicateRoot),
                "{left:?}, {right:?}"
            );
        }
    }

    #[test]
    fn nonexistent_directories_and_files_are_rejected_for_both_roots() {
        let temp = TestDir::new();
        temp.write("file", "not a directory");
        for raw in ["missing", "file"] {
            for field in ["root", "projectRoot"] {
                let mut app = json!({"root": "."});
                app[field] = json!(raw);
                assert_eq!(
                    resolve(&json!({"apps": [app]}), &temp.0),
                    Err(ConfigurationError::InvalidRoot),
                    "{field}: {raw}"
                );
            }
        }
    }

    #[test]
    fn nonexistent_workspace_and_workspace_file_are_rejected() {
        let temp = TestDir::new();
        temp.write("file", "not a directory");
        for workspace in [temp.0.join("missing"), temp.0.join("file")] {
            assert_eq!(
                resolve(&json!({}), &workspace),
                Err(ConfigurationError::InvalidRoot)
            );
        }
    }

    #[test]
    fn app_must_be_inside_project_root_on_a_component_boundary() {
        let temp = TestDir::new();
        temp.directory("apps");
        temp.directory("apps-other/src");
        temp.directory("separate/src");
        for root in ["apps-other/src", "separate/src", "."] {
            assert_eq!(
                resolve(
                    &json!({"apps": [{"root": root, "projectRoot": "apps"}]}),
                    &temp.0,
                ),
                Err(ConfigurationError::InvalidRoot),
                "{root}"
            );
        }
    }

    #[test]
    fn invalid_json_shapes_are_rejected_without_fallback() {
        for value in [
            json!(true),
            json!(42),
            json!("app"),
            json!({"appRoots": ["src"]}),
            json!({"apps": null}),
            json!({"apps": {"root": "."}}),
            json!({"apps": ["src"]}),
            json!({"apps": [null]}),
            json!({"apps": [{}]}),
            json!({"apps": [{"projectRoot": "."}]}),
            json!({"apps": [{"root": null}]}),
            json!({"apps": [{"root": 1}]}),
            json!({"apps": [{"root": ".", "project_root": "."}]}),
            json!({"apps": [{"root": ".", "projectRoot": false}]}),
            json!({"apps": [{"root": ".", "unknown": true}]}),
        ] {
            assert_eq!(
                InitializationOptions::parse(&value).unwrap_err(),
                ConfigurationError::InvalidOptions,
                "{value}"
            );
        }
    }

    #[test]
    fn top_level_arrays_are_rejected_instead_of_becoming_default_options() {
        let accepted: Vec<_> = [json!([]), json!([[]])]
            .into_iter()
            .filter(|value| InitializationOptions::parse(value).is_ok())
            .collect();
        assert!(
            accepted.is_empty(),
            "non-object options accepted: {accepted:?}"
        );
    }

    #[test]
    fn app_entries_must_be_objects_instead_of_positional_arrays() {
        let accepted: Vec<_> = [json!(["."]), json!([".", null]), json!([".", "."])]
            .into_iter()
            .filter(|app| InitializationOptions::parse(&json!({"apps": [app]})).is_ok())
            .collect();
        assert!(
            accepted.is_empty(),
            "non-object app entries accepted: {accepted:?}"
        );
    }

    #[test]
    fn app_count_is_limited_to_sixty_four() {
        for count in [0, 1, 64] {
            let options =
                InitializationOptions::parse(&json!({"apps": vec![json!({"root": "."}); count]}))
                    .expect("valid app count before filesystem resolution");
            assert_eq!(options.apps.len(), count);
        }
        assert_eq!(
            InitializationOptions::parse(&json!({"apps": vec![json!({"root": "."}); 65]}))
                .unwrap_err(),
            ConfigurationError::InvalidOptions
        );
    }

    #[test]
    fn parent_absolute_and_malformed_paths_are_rejected() {
        for raw in [
            "",
            "/",
            "/app",
            "//server/share",
            r"\\server\share",
            r"\app",
            "C:/app",
            r"C:\app",
            "c:app",
            "./C:/app",
            r".\C:\app",
            "..",
            "../app",
            "apps/../app",
            r"..\app",
            r"apps\..\app",
            "apps//app",
            r"apps\\app",
            "app/",
            "apps/./app",
            r"apps\.\app",
            r".\",
        ] {
            assert_invalid_path(raw);
        }
    }

    #[test]
    fn control_and_disguising_format_characters_are_rejected() {
        for character in ('\u{0}'..='\u{1f}')
            .chain('\u{7f}'..='\u{9f}')
            .chain('\u{200b}'..='\u{200f}')
            .chain('\u{202a}'..='\u{202e}')
            .chain('\u{2066}'..='\u{2069}')
        {
            assert_invalid_path(&format!("apps/{character}mini"));
        }
    }

    #[test]
    fn path_limit_counts_utf8_bytes_instead_of_characters() {
        for field in ["root", "projectRoot"] {
            for path in ["a".repeat(4096), "é".repeat(2048)] {
                let mut app = json!({"root": "."});
                app[field] = json!(&path);
                InitializationOptions::parse(&json!({"apps": [app]}))
                    .expect("exactly 4096 bytes is valid before filesystem resolution");
                assert_invalid_path(&format!("{path}a"));
            }
        }
    }

    #[test]
    fn resolve_validates_manually_constructed_options() {
        let temp = TestDir::new();
        let options = InitializationOptions {
            apps: vec![AppConfig {
                root: "../outside".to_owned(),
                project_root: None,
            }],
            ..InitializationOptions::default()
        };
        assert_eq!(
            options.resolve(&temp.0),
            Err(ConfigurationError::InvalidOptions)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_outside_workspace_are_rejected_for_both_roots() {
        let temp = TestDir::new();
        let workspace = temp.directory("workspace");
        temp.directory("workspace-other");
        std::os::unix::fs::symlink("../workspace-other", workspace.join("escape"))
            .expect("outside symlink");
        for field in ["root", "projectRoot"] {
            let mut app = json!({"root": "."});
            app[field] = json!("escape");
            assert_eq!(
                resolve(&json!({"apps": [app]}), &workspace),
                Err(ConfigurationError::InvalidRoot),
                "{field}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_within_workspace_resolve_to_canonical_directories() {
        let temp = TestDir::new();
        let app = temp.directory("project/src");
        std::os::unix::fs::symlink("project/src", temp.0.join("app-alias"))
            .expect("internal app symlink");
        std::os::unix::fs::symlink("project", temp.0.join("project-alias"))
            .expect("internal project symlink");
        let roots = resolve(
            &json!({"apps": [{"root": "app-alias", "projectRoot": "project-alias"}]}),
            &temp.0,
        )
        .expect("internal symlinks");
        assert_eq!(roots[0].app_dir, app);
        assert_eq!(roots[0].project_root, temp.0.join("project"));
        assert_eq!(roots[0].app_json.as_str(), "src/app.json");

        let roots = resolve(&json!({"apps": [{"root": "app-alias"}]}), &temp.0)
            .expect("default projectRoot through internal symlink");
        assert_eq!(roots[0].app_dir, app);
        assert_eq!(roots[0].project_root, app);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_cannot_create_duplicate_app_roots() {
        let temp = TestDir::new();
        temp.directory("real");
        std::os::unix::fs::symlink("real", temp.0.join("alias")).expect("internal symlink");
        assert_eq!(
            resolve(
                &json!({"apps": [{"root": "real"}, {"root": "alias"}]}),
                &temp.0,
            ),
            Err(ConfigurationError::DuplicateRoot)
        );
    }

    #[test]
    fn published_schema_has_the_expected_shape_and_limits() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../editors/zed/initialization-options.schema.json"
        ))
        .expect("initialization options JSON Schema");
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["apps"]["default"], json!([]));
        assert_eq!(schema["properties"]["apps"]["maxItems"], 64);
        assert_eq!(schema["$defs"]["app"]["required"], json!(["root"]));
        assert_eq!(schema["$defs"]["app"]["additionalProperties"], false);
        assert_eq!(schema["$defs"]["relativeDirectory"]["maxLength"], 4096);
        assert_eq!(
            schema["$defs"]["app"]["properties"]["projectRoot"]["anyOf"][1]["type"],
            "null"
        );
    }

    fn relative_inputs() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(".".to_owned()),
            Just("./".to_owned()),
            prop::collection::vec("[a-zA-Z0-9_-]{1,12}", 1..6).prop_map(|parts| parts.join("/")),
            prop::collection::vec(any::<char>(), 0..96)
                .prop_map(|characters| characters.into_iter().collect()),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 128,
            // A failing property must never create files beside the source tree.
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        #[test]
        fn accepted_relative_input_has_no_lexical_escape(raw in relative_inputs()) {
            if let Ok(relative) = relative_directory(&raw) {
                prop_assert!(!relative.is_absolute());
                prop_assert!(relative.components().all(|part| matches!(part, Component::Normal(_))));
                prop_assert!(!relative.to_string_lossy().contains('\\'));
                prop_assert!(raw.len() <= 4096);
                prop_assert!(Path::new("/tmp/opencode/workspace").join(relative)
                    .starts_with("/tmp/opencode/workspace"));
            }
        }

        #[test]
        fn generated_directories_resolve_within_workspace(
            components in prop::collection::vec("[a-z][a-z0-9_-]{0,7}", 1..5),
            windows_separators in any::<bool>(),
            leading_dot in any::<bool>(),
            explicit_parent in any::<bool>(),
        ) {
            let temp = TestDir::new();
            let normalized = components.join("/");
            let app = temp.directory(&normalized);
            let separator = if windows_separators { "\\" } else { "/" };
            let mut raw = components.join(separator);
            if leading_dot {
                raw = format!(".{separator}{raw}");
            }
            let mut config = json!({"root": raw});
            if explicit_parent {
                config["projectRoot"] = json!(&components[0]);
            }
            let roots = resolve(&json!({"apps": [config]}), &temp.0).expect("generated app");
            prop_assert_eq!(roots.len(), 1);
            prop_assert_eq!(&roots[0].app_dir, &app);
            prop_assert!(roots[0].app_dir.starts_with(&temp.0));
            prop_assert!(roots[0].project_root.starts_with(&temp.0));
            prop_assert!(roots[0].app_dir.starts_with(&roots[0].project_root));
            let expected_project = if explicit_parent { temp.0.join(&components[0]) } else { app };
            prop_assert_eq!(&roots[0].project_root, &expected_project);
        }

        #[test]
        fn parent_components_are_rejected_even_between_valid_segments(
            before in "[a-z]{1,12}",
            after in "[a-z]{1,12}",
            windows_separators in any::<bool>(),
        ) {
            let separator = if windows_separators { "\\" } else { "/" };
            let raw = format!("{before}{separator}..{separator}{after}");
            let result = InitializationOptions::parse(&json!({"apps": [{"root": raw}]}));
            prop_assert_eq!(result.unwrap_err(), ConfigurationError::InvalidOptions);
        }
    }
}
