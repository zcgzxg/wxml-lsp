//! Discovery of the component type declaration file.
//!
//! A project may place one declaration file next to its `tsconfig.json`:
//! `components.d.ts`. The file is read through the project overlay, so unsaved
//! editor text wins over disk, and it is bounded by the crate-wide file read
//! limit. There is no directory scan and no wildcard matching: a missing file
//! is a normal `None`, and only a path that exists but cannot be used produces
//! an issue.

use thiserror::Error;

use crate::Project;
use crate::config::TsConfig;
use crate::files::{FileError, MAX_FILE_BYTES, ProjectPath};

/// Name of the declaration file, resolved next to `tsconfig.json`.
pub const DECLARATION_FILE_NAME: &str = "components.d.ts";

/// Effective text of one component declaration file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclarationSource {
    /// Project-relative path of the declaration file.
    pub path: ProjectPath,
    /// Effective text: overlay content when present, otherwise disk content.
    pub text: String,
}

/// Why a declaration file exists but cannot be used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DeclarationIssueKind {
    /// The path exists but is not a regular file, for example a directory.
    NotAFile,
    /// The file, or a directory leading to it, escapes the project root.
    EscapesRoot,
    /// The file is larger than [`MAX_FILE_BYTES`].
    TooLarge,
    /// The file could not be read as UTF-8 text.
    Unreadable,
}

/// One readable explanation for an unusable declaration file.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("组件类型声明 {path}：{message}")]
pub struct DeclarationIssue {
    /// Project-relative path that produced the issue.
    pub path: ProjectPath,
    /// Issue category.
    pub kind: DeclarationIssueKind,
    /// Stable explanation that never contains an absolute host path.
    pub message: String,
}

/// Returns the fixed declaration path next to `tsconfig`.
///
/// The file is always `components.d.ts` in the directory that holds the loaded
/// configuration, so an `extends` parent does not move the lookup. `None`
/// means no configuration was loaded, and therefore no declaration file is
/// considered.
#[must_use]
pub fn declaration_path(tsconfig: Option<&TsConfig>) -> Option<ProjectPath> {
    let tsconfig = tsconfig?;
    let directory = tsconfig.path.parent();
    ProjectPath::resolve(directory.as_ref(), DECLARATION_FILE_NAME).ok()
}

/// Reads the component declaration file for `project`.
///
/// Overlay text wins over disk, mirroring every other project read. A missing
/// file is `Ok(None)` and is not an error: projects without declarations keep
/// their previous behaviour and produce no issue or degradation. A path that
/// exists but is not a usable regular file inside the project root returns
/// [`DeclarationIssue`].
///
/// # Errors
///
/// Returns [`DeclarationIssue`] when the path is a directory or another special
/// file, escapes the project root, exceeds [`MAX_FILE_BYTES`], or cannot be
/// read as UTF-8.
pub fn resolve_declaration(
    project: &Project,
) -> Result<Option<DeclarationSource>, DeclarationIssue> {
    let Some(path) = declaration_path(project.tsconfig()) else {
        return Ok(None);
    };
    if let Some(text) = project.overlay().text(&path) {
        return Ok(Some(DeclarationSource {
            path,
            text: text.to_owned(),
        }));
    }
    let absolute = project.root().join(path.as_str());
    if let Ok(metadata) = std::fs::metadata(&absolute)
        && !metadata.is_file()
    {
        return Err(DeclarationIssue {
            path,
            kind: DeclarationIssueKind::NotAFile,
            message: "路径存在但不是普通文件（例如目录），已忽略".to_owned(),
        });
    }
    match project.read_optional(&path) {
        Ok(Some(text)) => Ok(Some(DeclarationSource { path, text })),
        Ok(None) => Ok(None),
        Err(error) => Err(from_file_error(&path, &error)),
    }
}

/// Maps a project read error onto a declaration issue.
fn from_file_error(path: &ProjectPath, error: &FileError) -> DeclarationIssue {
    let (kind, message) = match error {
        FileError::TooLarge { size, .. } => (
            DeclarationIssueKind::TooLarge,
            format!("文件为 {size} 字节，超过 {MAX_FILE_BYTES} 字节上限，已忽略"),
        ),
        FileError::EscapesRoot { .. } => (
            DeclarationIssueKind::EscapesRoot,
            "文件或所在目录逃出项目根，已忽略".to_owned(),
        ),
        FileError::InvalidUtf8 { .. } => (
            DeclarationIssueKind::Unreadable,
            "文件不是有效的 UTF-8 文本，已忽略".to_owned(),
        ),
        FileError::Io { kind, .. } => (
            DeclarationIssueKind::Unreadable,
            format!("读取失败（{kind:?}），已忽略"),
        ),
        FileError::NotFound { .. } => (
            DeclarationIssueKind::Unreadable,
            "文件在读取过程中消失，已忽略".to_owned(),
        ),
    };
    DeclarationIssue {
        path: path.clone(),
        kind,
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    /// Base for temporary directories: `TMPDIR` when set, otherwise `/tmp/opencode`.
    fn temp_base() -> PathBuf {
        std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from)
    }

    struct TempDir {
        /// Owns creation and recursive cleanup.
        _dir: tempfile::TempDir,
        /// Directory handle used by the helpers.
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = tempfile::Builder::new()
                .prefix(name)
                .tempdir_in(temp_base())
                .expect("create temp dir");
            let path = dir.path().to_path_buf();
            Self { _dir: dir, path }
        }

        fn write(&self, relative: &str, text: &str) {
            let target = self.path.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(target, text).expect("write file");
        }

        /// Returns the absolute root text, used to prove messages stay relative.
        fn root_text(&self) -> String {
            self.path.display().to_string()
        }
    }

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::new(raw).expect("valid path")
    }

    /// Welcomes discovery with a minimal `app.json`.
    fn open_project(temp: &TempDir) -> Project {
        temp.write("app.json", r#"{"pages": []}"#);
        Project::open(temp.path.clone()).expect("open project")
    }

    #[test]
    fn finds_components_dts_next_to_tsconfig() {
        let temp = TempDir::new("declaration-found");
        temp.write("tsconfig.json", "{}");
        temp.write("components.d.ts", "interface WxmlComponentMap {}");
        let project = open_project(&temp);

        let source = resolve_declaration(&project)
            .expect("read")
            .expect("declaration file");
        assert_eq!(source.path, path("components.d.ts"));
        assert_eq!(source.text, "interface WxmlComponentMap {}");
    }

    #[test]
    fn returns_none_when_the_declaration_file_is_missing() {
        let temp = TempDir::new("declaration-missing");
        temp.write("tsconfig.json", "{}");
        let project = open_project(&temp);

        assert_eq!(resolve_declaration(&project), Ok(None));
    }

    #[test]
    fn returns_none_without_a_tsconfig() {
        let temp = TempDir::new("declaration-no-config");
        temp.write("components.d.ts", "interface WxmlComponentMap {}");
        let project = open_project(&temp);

        assert!(project.tsconfig().is_none());
        assert_eq!(resolve_declaration(&project), Ok(None));
    }

    #[test]
    fn overlay_text_wins_over_disk() {
        let temp = TempDir::new("declaration-overlay");
        temp.write("tsconfig.json", "{}");
        temp.write(
            "components.d.ts",
            "interface WxmlComponentMap { disk: Disk }",
        );
        let mut project = open_project(&temp);
        project
            .set_overlay(
                &path("components.d.ts"),
                "interface WxmlComponentMap { overlay: Overlay }",
                1,
            )
            .expect("overlay");

        let source = resolve_declaration(&project)
            .expect("read")
            .expect("declaration file");
        assert_eq!(
            source.text,
            "interface WxmlComponentMap { overlay: Overlay }"
        );
    }

    #[test]
    fn does_not_search_deeper_directories() {
        let temp = TempDir::new("declaration-deeper");
        temp.write("tsconfig.json", "{}");
        temp.write("src/components.d.ts", "interface WxmlComponentMap {}");
        let project = open_project(&temp);

        assert_eq!(resolve_declaration(&project), Ok(None));
    }

    #[test]
    fn reads_the_declaration_next_to_a_nested_tsconfig() {
        let temp = TempDir::new("declaration-nested");
        temp.write("app.json", r#"{"pages": []}"#);
        temp.write(
            "components.d.ts",
            "interface WxmlComponentMap { root: Root }",
        );
        temp.write("apps/app/tsconfig.json", "{}");
        temp.write(
            "apps/app/components.d.ts",
            "interface WxmlComponentMap { nested: Nested }",
        );
        let project = Project::open_with_config(
            temp.path,
            path("app.json"),
            Vec::new(),
            path("apps/app/tsconfig.json"),
            BTreeMap::new(),
        )
        .expect("open project");

        let source = resolve_declaration(&project)
            .expect("read")
            .expect("declaration file");
        assert_eq!(source.path, path("apps/app/components.d.ts"));
        assert_eq!(source.text, "interface WxmlComponentMap { nested: Nested }");
    }

    #[test]
    fn reports_a_directory_instead_of_a_file() {
        let temp = TempDir::new("declaration-directory");
        temp.write("tsconfig.json", "{}");
        fs::create_dir_all(temp.path.join("components.d.ts")).expect("create directory");
        let project = open_project(&temp);

        let error = resolve_declaration(&project).expect_err("directory must be rejected");
        assert_eq!(error.kind, DeclarationIssueKind::NotAFile);
        assert_eq!(error.path, path("components.d.ts"));
        assert!(error.message.contains("目录"), "message={}", error.message);
        assert!(!error.message.contains(&temp.root_text()));
    }

    #[test]
    fn reports_an_oversized_declaration() {
        let temp = TempDir::new("declaration-too-large");
        temp.write("tsconfig.json", "{}");
        let target = temp.path.join("components.d.ts");
        let file = fs::File::create(&target).expect("create declaration");
        file.set_len(MAX_FILE_BYTES + 1).expect("set length");
        let project = open_project(&temp);

        let error = resolve_declaration(&project).expect_err("oversized file must be rejected");
        assert_eq!(error.kind, DeclarationIssueKind::TooLarge);
        assert_eq!(error.path, path("components.d.ts"));
        assert!(
            error.message.contains(&MAX_FILE_BYTES.to_string()),
            "message={}",
            error.message
        );
        assert!(!error.message.contains(&temp.root_text()));
    }

    #[test]
    fn reports_a_non_utf8_declaration() {
        let temp = TempDir::new("declaration-invalid-utf8");
        temp.write("tsconfig.json", "{}");
        fs::write(temp.path.join("components.d.ts"), [0xff, 0xfe]).expect("write bytes");
        let project = open_project(&temp);

        let error = resolve_declaration(&project).expect_err("invalid UTF-8 must be rejected");
        assert_eq!(error.kind, DeclarationIssueKind::Unreadable);
        assert!(error.message.contains("UTF-8"), "message={}", error.message);
    }

    #[cfg(unix)]
    #[test]
    fn reports_a_symlink_that_escapes_the_root() {
        let outside = TempDir::new("declaration-outside");
        outside.write("components.d.ts", "interface WxmlComponentMap {}");

        let temp = TempDir::new("declaration-escape");
        temp.write("tsconfig.json", "{}");
        std::os::unix::fs::symlink(
            outside.path.join("components.d.ts"),
            temp.path.join("components.d.ts"),
        )
        .expect("create symlink");
        let project = open_project(&temp);

        let error = resolve_declaration(&project).expect_err("escape must be rejected");
        assert_eq!(error.kind, DeclarationIssueKind::EscapesRoot);
        assert_eq!(error.path, path("components.d.ts"));
        assert!(
            error.message.contains("项目根"),
            "message={}",
            error.message
        );
        assert!(!error.message.contains(&temp.root_text()));
    }
}
