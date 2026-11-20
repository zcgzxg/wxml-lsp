//! Fixture anchors and the per-path query cases.
//!
//! Offsets are resolved from unique text anchors and then converted to LSP
//! positions by [`position`]. Changing the WXML text therefore only requires
//! updating the anchors here, never a magic line/column pair.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::session::{position_params, uri_for};

/// The `bench-queries` mini-program used by every case.
pub(crate) struct Fixture {
    /// Project root (`tests/fixtures/bench-queries`).
    pub root: PathBuf,
    /// Absolute path of `pages/index/index.wxml`.
    pub page: PathBuf,
    /// Text of the page, opened as an overlay in every session.
    pub text: String,
}

impl Fixture {
    /// Loads the fixture from the workspace.
    pub(crate) fn load() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bench-queries");
        let root = root.canonicalize().expect("bench fixture root");
        let page = root.join("pages/index/index.wxml");
        let text = std::fs::read_to_string(&page).expect("bench fixture page");
        Self { root, page, text }
    }

    /// Returns the `file://` URI of the page.
    pub(crate) fn page_uri(&self) -> String {
        uri_for(&self.page)
    }

    /// Offset of the `card` token inside the mixed `class="card {{extra}}"`.
    pub(crate) fn class_mixed_offset(&self) -> u32 {
        byte_offset(self.anchor("class=\"card {{extra}}\"") + "class=\"".len())
    }

    /// Prints the fixture scale required by ADR-0004.
    pub(crate) fn print_scale(&self) {
        let mut wxss = Vec::new();
        collect_wxss(&self.root, &mut wxss);
        wxss.sort();
        let wxss_lines = wxss.iter().fold(0_usize, |sum, path| {
            sum + std::fs::read_to_string(path).map_or(0, |text| text.lines().count())
        });
        let mut classes = std::collections::BTreeSet::<String>::new();
        for (_, _, name) in wxml_lsp::bench_support::class_tokens(&self.text) {
            classes.insert(name);
        }
        println!(
            "bench fixture: {} lines WXML, {} distinct class tokens, {} WXSS files / {} lines",
            self.text.lines().count(),
            classes.len(),
            wxss.len(),
            wxss_lines,
        );
    }

    fn anchor(&self, needle: &str) -> usize {
        anchor(&self.text, needle)
    }

    /// Case for a root data variable hover.
    pub(crate) fn type_cases(&self) -> Vec<Case> {
        vec![
            self.hover_case("root", "{{ title }}", 3, &["string", "页面标题"]),
            self.hover_case(
                "member",
                "{{ user.name }}",
                3 + "user.".len(),
                &["string", "用户显示名称"],
            ),
            self.hover_case(
                "local",
                "{{ item.label }}",
                3 + "item.".len(),
                &["string", "行标题"],
            ),
        ]
    }

    /// Cases for builtin/component tag, attribute and value hover.
    pub(crate) fn tag_cases(&self) -> Vec<Case> {
        vec![
            self.hover_case("builtin", "<view class=\"card\">", 1, &["view", "视图容器"]),
            self.hover_case(
                "component",
                "<outer-card ",
                1,
                &["outer-card", "自定义组件"],
            ),
            self.hover_case(
                "attribute",
                "class=\"page-shell\"",
                1,
                &["class", "样式类列表"],
            ),
            self.hover_case("value", "type=\"text\"", "type=\"".len(), &["text", "取值"]),
        ]
    }

    /// LSP cases for class hover, completion and definition.
    ///
    /// The returned order is part of the cold-case selection: `hover` must
    /// stay first.
    pub(crate) fn class_cases(&self) -> Vec<Case> {
        let anchor = self.anchor("class=\"card\"");
        let token = anchor + "class=\"".len();
        let hover = self.case_at(
            "hover",
            "textDocument/hover",
            token,
            Expect::Hover(&["card", "WXSS class"]),
        );
        let completion = self.case_at(
            "completion",
            "textDocument/completion",
            token + "car".len(),
            Expect::Completion("card-title"),
        );
        let definition = self.case_at(
            "definition",
            "textDocument/definition",
            token,
            Expect::Definition("index.wxss"),
        );
        vec![hover, completion, definition]
    }

    fn hover_case(
        &self,
        id: &'static str,
        needle: &str,
        extra: usize,
        expect: &'static [&'static str],
    ) -> Case {
        self.case_at(
            id,
            "textDocument/hover",
            self.anchor(needle) + extra,
            Expect::Hover(expect),
        )
    }

    fn case_at(&self, id: &'static str, method: &'static str, byte: usize, expect: Expect) -> Case {
        let (line, character) = position(&self.text, byte);
        Case {
            id,
            method,
            params: position_params(&self.page_uri(), line, character),
            expect,
        }
    }
}

/// One query plus the correctness gate that must pass before timing.
pub(crate) struct Case {
    /// Group suffix (`root`, `builtin`, `hover`, ...).
    pub id: &'static str,
    /// JSON-RPC method.
    method: &'static str,
    /// Pre-built request parameters.
    params: Value,
    /// Acceptance rule for the result.
    expect: Expect,
}

impl Case {
    /// Sends the query once and returns its result.
    pub(crate) fn run(&self, session: &mut crate::session::Session) -> Value {
        session.call(self.method, self.params.clone())
    }

    /// Runs the case once and asserts its result.
    pub(crate) fn verify(&self, session: &mut crate::session::Session) {
        let result = self.run(session);
        self.check(&result);
    }

    /// Asserts that `result` is the expected non-degraded answer.
    ///
    /// # Panics
    ///
    /// Panics with the case id when the server answered `null`, an error
    /// shape, or a value missing an expected fragment.
    pub(crate) fn check(&self, result: &Value) {
        assert_ne!(result, &Value::Null, "{}: empty result", self.id);
        let text = serde_json::to_string(result).expect("serialize result");
        match self.expect {
            Expect::Hover(fragments) => {
                let markdown = result
                    .pointer("/contents/value")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("{}: not a hover: {text}", self.id));
                for fragment in fragments {
                    assert!(
                        markdown.contains(fragment),
                        "{}: hover missing {fragment:?}: {markdown}",
                        self.id
                    );
                }
            }
            Expect::Completion(fragment) => {
                assert!(
                    text.contains(fragment),
                    "{}: completion missing {fragment:?}: {text}",
                    self.id
                );
            }
            Expect::Definition(fragment) => {
                assert!(
                    text.contains(fragment),
                    "{}: definition missing {fragment:?}: {text}",
                    self.id
                );
            }
        }
    }
}

/// What a successful result must contain.
enum Expect {
    /// Markdown hover containing every fragment.
    Hover(&'static [&'static str]),
    /// Completion list containing `label`.
    Completion(&'static str),
    /// Definition locations containing `path` suffix.
    Definition(&'static str),
}

/// Returns the unique byte offset of `needle` in `text`.
///
/// # Panics
///
/// Panics when the anchor is missing or appears more than once, so a fixture
/// edit cannot silently benchmark the wrong token.
fn anchor(text: &str, needle: &str) -> usize {
    let mut matches = text.match_indices(needle);
    let (offset, _) = matches
        .next()
        .unwrap_or_else(|| panic!("anchor {needle:?} not found"));
    assert!(
        matches.next().is_none(),
        "anchor {needle:?} is not unique in the fixture"
    );
    offset
}

/// Converts an in-file byte offset into `u32` for `class_site_at`.
fn byte_offset(offset: usize) -> u32 {
    u32::try_from(offset).expect("fixture offset fits u32")
}

/// Converts a byte offset into an LSP `(line, character)` position.
///
/// `character` counts UTF-16 code units, like every LSP client.
fn position(text: &str, byte: usize) -> (u32, u32) {
    assert!(
        text.is_char_boundary(byte),
        "anchor splits a UTF-8 character"
    );
    let before = &text[..byte];
    let line = u32::try_from(before.matches('\n').count()).expect("line fits u32");
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let character =
        u32::try_from(before[line_start..].encode_utf16().count()).expect("character fits u32");
    (line, character)
}

fn collect_wxss(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("fixture directory") {
        let entry = entry.expect("fixture entry");
        let path = entry.path();
        if path.is_dir() {
            collect_wxss(&path, out);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "wxss")
        {
            out.push(path);
        }
    }
}
