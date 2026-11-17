//! Cross-feature JSON-RPC integration matrix (IT-09, IT-10, IT-11, IT-12).
//!
//! The session is the real server event loop over a socket pair with the
//! in-process mock type backend. Styles, resources, formatting and structure
//! are local features, so every assertion is derived from actual WXML/WXSS
//! text and files written under `/tmp/opencode`.

#![cfg(unix)]
#![allow(
    clippy::assert_is_empty,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

#[path = "full_feature_matrix/support.rs"]
mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use support::{
    Session, TempProject, apply_completion_item, apply_edits, assert_parses_cleanly,
    definition_uris, diagnostic_codes, item, labels, link_targets, offset_at, position_at,
};
use wxml_types::{Backend, MockBackend};

fn hover_text(hover: &Value) -> String {
    hover["contents"]["value"]
        .as_str()
        .expect("hover markdown")
        .to_owned()
}

fn byte_range(text: &str, range: &Value) -> (usize, usize) {
    (
        offset_at(text, &range["start"]),
        offset_at(text, &range["end"]),
    )
}

fn style_fixture() -> TempProject {
    let project = TempProject::new("wxml-it09");
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.ts", "Page({ data: {} });");
    project.write("pages/index/index.wxml", "<view />");
    project.write(
        "app.wxss",
        "/* app global doc */\n.app-global { color: black; }\n/* .comment-global { color: red; } */\n",
    );
    project.write(
        "shared/shared.scss",
        "/* shared docs */\n.shared { color: red; }\n.shared-child { color: blue; }\n/* .comment-shared { color: green; } */\n",
    );
    project.write(
        "styles/global.wxss",
        "/* configured doc */\n.configured { color: gray; }\n",
    );
    project.write(
        "pages/index/index.wxss",
        "@import \"../../shared/shared\";\n.page-only { color: red; }\n/* .comment-page { color: blue; } */\n",
    );
    project
}

/// IT-09: WXSS imports, app/global styles and project comments drive class
/// completion, hover and definition; commented selectors are not definitions.
#[test]
fn it_09_style_classes_follow_imports_globals_and_ignore_comments() {
    let project = style_fixture();
    let mut session = Session::start(
        &project,
        json!({"wxml": {"globalStyles": ["styles/global.wxss"]}}),
    );
    let uri = project.uri("pages/index/index.wxml");
    let completion_text = "<view class=\"page-only \"></view>";
    session.open_wxml(&uri, completion_text, 1);
    let cursor = completion_text.find("page-only ").expect("page-only") + "page-only ".len();

    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(completion_text, cursor),
        }),
    );
    let completion = session.result(id);
    let visible = labels(&completion);
    for expected in ["shared", "app-global", "configured", "shared-child"] {
        assert!(
            visible.contains(&expected.to_owned()),
            "{expected} missing: {visible:?}"
        );
    }
    assert!(
        !visible.contains(&"page-only".to_owned()),
        "already present class must be filtered: {visible:?}"
    );
    for forbidden in [
        "comment-global",
        "comment-shared",
        "comment-page",
        "unknown",
    ] {
        assert!(
            !visible.contains(&forbidden.to_owned()),
            "{forbidden} must not be a class: {visible:?}"
        );
    }
    let shared_item = item(&completion, "shared");
    assert!(
        shared_item["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("shared/shared.scss")),
        "{shared_item}"
    );
    assert_eq!(
        shared_item["textEdit"]["range"]["start"],
        position_at(completion_text, cursor),
        "{shared_item}"
    );
    let configured_item = item(&completion, "configured");
    assert!(
        configured_item["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("styles/global.wxss")),
        "{configured_item}"
    );

    let text = "<view class=\"page-only shared app-global configured\"></view>";
    session.change(&uri, text, 2);
    let _ = session.diagnostics_version(&uri, 2);

    let queries = [
        ("shared", "shared docs", Some("shared/shared.scss")),
        ("app-global", "app global doc", Some("app.wxss")),
        ("configured", "configured doc", Some("styles/global.wxss")),
    ];
    for (class, hover_comment, definition_path) in queries {
        let offset = text.find(class).expect("class token") + 1;
        let id = session.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": position_at(text, offset),
            }),
        );
        let hover = session.result(id);
        let hover = hover_text(&hover);
        assert!(hover.contains("WXSS class"), "{class}: {hover}");
        assert!(hover.contains(hover_comment), "{class}: {hover}");
        assert!(
            !hover.contains("style index is partial"),
            "{class}: {hover}"
        );
        let id = session.request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": uri},
                "position": position_at(text, offset),
            }),
        );
        let definition = session.result(id);
        let uris = definition_uris(&definition);
        assert!(
            uris.iter()
                .any(|uri| uri.ends_with(definition_path.expect("path"))),
            "{class}: {uris:?}"
        );
    }

    let comment_text = "<view class=\"comment-page\" />";
    session.change(&uri, comment_text, 3);
    let _ = session.diagnostics_version(&uri, 3);
    let offset = comment_text.find("comment-page").expect("comment class") + 2;
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(comment_text, offset),
        }),
    );
    let hover = session.result(id);
    assert!(
        hover["result"].is_null(),
        "comment text is not a class definition: {hover}"
    );
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(comment_text, offset),
        }),
    );
    let definition = session.result(id);
    assert!(
        definition_uris(&definition).is_empty(),
        "comment text is not a definition: {definition}"
    );

    session.finish();
}

fn resource_fixture() -> TempProject {
    let project = TempProject::new("wxml-it10");
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.ts", "Page({ data: {} });");
    project.write("pages/index/index.wxml", "<view />");
    project.write("images/logo.png", "png");
    project
}

/// IT-10: local resource document links and definitions follow file
/// creation/deletion through watched files; missing and remote values are
/// never turned into local targets.
#[test]
fn it_10_document_links_and_resource_diagnostics_track_disk_changes() {
    let project = resource_fixture();
    let mut session = Session::start(
        &project,
        json!({"wxml": {"styleDiagnostics": {"resources": true, "classes": false}}}),
    );
    let uri = project.uri("pages/index/index.wxml");
    let text = "<view>\
<image src=\"../../images/logo.png\" />\
<cover-image src=\"../../images/logo.png\" />\
<image src=\"../../images/new.png\" />\
<image src=\"https://example.com/a.png\" />\
</view>";
    session.open_wxml(&uri, text, 1);
    let published = session.diagnostics_version(&uri, 1);
    let codes = diagnostic_codes(&published);
    assert_eq!(
        codes
            .iter()
            .filter(|code| *code == "style.missing-resource")
            .count(),
        1,
        "{published}"
    );
    assert!(
        published["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("new.png"))),
        "{published}"
    );

    let id = session.request(
        "textDocument/documentLink",
        json!({"textDocument": {"uri": uri}}),
    );
    let links = session.result(id);
    let targets = link_targets(&links);
    assert_eq!(
        targets
            .iter()
            .filter(|target| target.ends_with("images/logo.png"))
            .count(),
        2,
        "{targets:?}"
    );
    assert!(
        targets.iter().all(|target| !target.contains("http")),
        "remote URLs must never become links: {targets:?}"
    );
    assert!(
        targets.iter().all(|target| !target.contains("new.png")),
        "missing resources must not become links: {targets:?}"
    );

    let missing_offset = text.find("new.png").expect("missing") + 2;
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(text, missing_offset),
        }),
    );
    let missing_definition = session.result(id);
    assert!(
        definition_uris(&missing_definition).is_empty(),
        "missing resource must not resolve: {missing_definition}"
    );

    let typed = "<image src=\"../../images/\" />";
    session.change(&uri, typed, 2);
    let _ = session.diagnostics_version(&uri, 2);
    let cursor = typed.find("images/").expect("images") + "images/".len();
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(typed, cursor),
        }),
    );
    let completion = session.result(id);
    let visible = labels(&completion);
    assert!(visible.contains(&"logo.png".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"new.png".to_owned()), "{visible:?}");

    // Restore full resource paths before exercising the watched-file updates.
    session.change(&uri, text, 3);
    let _ = session.diagnostics_version(&uri, 3);

    // Creating the missing file and deleting the existing one updates both
    // links and diagnostics through the watched-file path.
    project.write("images/new.png", "png");
    session.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": project.uri("images/new.png"), "type": 1}]}),
    );
    let created = session.diagnostics_version(&uri, 3);
    assert!(
        !diagnostic_codes(&created).contains(&"style.missing-resource".to_owned()),
        "{created}"
    );
    let id = session.request(
        "textDocument/documentLink",
        json!({"textDocument": {"uri": uri}}),
    );
    let links = session.result(id);
    let targets = link_targets(&links);
    assert!(
        targets.iter().any(|target| target.ends_with("new.png")),
        "{targets:?}"
    );
    assert!(
        targets.iter().all(|target| !target.contains("http")),
        "{targets:?}"
    );

    project.remove("images/logo.png");
    session.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": project.uri("images/logo.png"), "type": 3}]}),
    );
    let deleted = session.diagnostics(&uri);
    assert!(
        diagnostic_codes(&deleted).contains(&"style.missing-resource".to_owned()),
        "{deleted}"
    );
    assert!(
        deleted["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("logo.png"))),
        "{deleted}"
    );
    let id = session.request(
        "textDocument/documentLink",
        json!({"textDocument": {"uri": uri}}),
    );
    let links = session.result(id);
    let targets = link_targets(&links);
    assert!(
        targets.iter().all(|target| !target.ends_with("logo.png")),
        "deleted resource must not link: {targets:?}"
    );
    assert!(
        targets.iter().any(|target| target.ends_with("new.png")),
        "created resource must stay linked: {targets:?}"
    );

    session.change(&uri, typed, 4);
    let _ = session.diagnostics_version(&uri, 4);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(typed, cursor),
        }),
    );
    let completion = session.result(id);
    let visible = labels(&completion);
    assert!(!visible.contains(&"logo.png".to_owned()), "{visible:?}");
    assert!(visible.contains(&"new.png".to_owned()), "{visible:?}");

    // Remote values stay remote: no link, no local definition.
    let remote = "<image src=\"https://example.com/a.png\" />";
    session.change(&uri, remote, 5);
    let _ = session.diagnostics_version(&uri, 5);
    let remote_offset = remote.find("https://").expect("remote") + 2;
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(remote, remote_offset),
        }),
    );
    let remote_definition = session.result(id);
    assert!(
        definition_uris(&remote_definition).is_empty(),
        "remote resource must not resolve locally: {remote_definition}"
    );
    let id = session.request(
        "textDocument/documentLink",
        json!({"textDocument": {"uri": uri}}),
    );
    let links = session.result(id);
    assert!(
        link_targets(&links)
            .iter()
            .all(|target| !target.contains("http")),
        "{links}"
    );

    session.finish();
}

const FORMAT_SOURCE: &str = "<view   class=\"中文😀\"   data-label=\"{{ user.name }}\">\r\n  <text   >hi &amp; {{ name }}</text>\r\n  <!-- 注释 😀 -->\r\n  <wxs module=\"m\">var a = 1 < 2;\r\nvar b = \"中文😀\";</wxs>\r\n  <image src=\"a b.png\"   />\r\n</view>\r\n";

fn formatting_fixture() -> TempProject {
    let project = TempProject::new("wxml-it11");
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.ts", "Page({ data: {} });");
    project.write("pages/index/index.wxml", "<view />");
    project.write("pages/index/a b.png", "png");
    project
}

/// IT-11: document/range formatting preserves CRLF, Chinese, emoji, Mustache
/// and WXS raw text, is idempotent, and the applied result re-parses cleanly.
#[test]
fn it_11_formatting_is_lossless_idempotent_and_reparses() {
    let project = formatting_fixture();
    let mut session = Session::start(&project, json!({}));
    let uri = project.uri("pages/index/index.wxml");
    session.open_wxml(&uri, FORMAT_SOURCE, 1);

    let id = session.request(
        "textDocument/formatting",
        json!({
            "textDocument": {"uri": uri},
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let edits = session.result(id);
    let formatted = apply_edits(FORMAT_SOURCE, &edits);
    assert_ne!(formatted, FORMAT_SOURCE, "formatting must make progress");
    assert!(
        formatted.contains("class=\"中文😀\""),
        "attribute value changed: {formatted:?}"
    );
    assert!(
        formatted.contains("data-label=\"{{ user.name }}\""),
        "mustache changed: {formatted:?}"
    );
    assert!(formatted.contains("&amp;"), "{formatted:?}");
    assert!(formatted.contains("{{ name }}"), "{formatted:?}");
    assert!(formatted.contains("注释 😀"), "{formatted:?}");
    let wxs_start = FORMAT_SOURCE.find("var a").expect("wxs start");
    let wxs_end = FORMAT_SOURCE.find("</wxs>").expect("wxs end");
    let raw_wxs = &FORMAT_SOURCE[wxs_start..wxs_end];
    assert!(
        formatted.contains(raw_wxs),
        "WXS raw text changed: {formatted:?}"
    );
    assert!(
        formatted.contains("src=\"a b.png\""),
        "resource value changed: {formatted:?}"
    );
    assert!(
        formatted
            .match_indices('\n')
            .all(|(index, _)| index > 0 && formatted.as_bytes()[index - 1] == b'\r'),
        "CRLF was not preserved: {formatted:?}"
    );
    assert_parses_cleanly(&formatted);

    session.change(&uri, &formatted, 2);
    let diagnostics = session.diagnostics_version(&uri, 2);
    assert!(
        diagnostics["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .is_empty(),
        "formatted text must still analyze cleanly: {diagnostics}"
    );
    let id = session.request(
        "textDocument/formatting",
        json!({
            "textDocument": {"uri": uri},
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let second_edits = session.result(id);
    assert_eq!(
        second_edits,
        json!([]),
        "formatting must be idempotent: {second_edits}"
    );

    // Resource links and linked editing still see the same semantic text.
    let id = session.request(
        "textDocument/documentLink",
        json!({"textDocument": {"uri": uri}}),
    );
    let links = session.result(id);
    assert!(
        link_targets(&links)
            .iter()
            .any(|target| target.contains("a%20b.png")),
        "formatted resource string changed: {links}"
    );
    let view_offset = formatted.find("view").expect("view tag") + 1;
    let id = session.request(
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(&formatted, view_offset),
        }),
    );
    let linked = session.result(id);
    assert_eq!(
        linked["ranges"].as_array().expect("linked ranges").len(),
        2,
        "{linked}"
    );

    // Range formatting touches only the selected line.
    session.change(&uri, FORMAT_SOURCE, 3);
    let _ = session.diagnostics_version(&uri, 3);
    let line_start = FORMAT_SOURCE.find("  <text").expect("text line");
    let line_end = FORMAT_SOURCE[line_start..]
        .find('\r')
        .map_or(FORMAT_SOURCE.len(), |offset| line_start + offset);
    let id = session.request(
        "textDocument/rangeFormatting",
        json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": position_at(FORMAT_SOURCE, line_start),
                "end": position_at(FORMAT_SOURCE, line_end),
            },
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let ranged_edits = session.result(id);
    let ranged = apply_edits(FORMAT_SOURCE, &ranged_edits);
    assert!(
        ranged.starts_with("<view   class=\"中文😀\"   data-label=\"{{ user.name }}\">"),
        "range formatting changed text outside the range: {ranged:?}"
    );
    assert!(
        ranged.contains("  <text>hi &amp; {{ name }}</text>"),
        "{ranged:?}"
    );
    assert!(
        ranged.contains(raw_wxs),
        "range formatting changed WXS raw text: {ranged:?}"
    );
    assert_parses_cleanly(&ranged);

    session.finish();
}

fn structure_fixture() -> TempProject {
    let project = TempProject::new("wxml-it12");
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.ts", "Page({ data: {} });");
    project.write("pages/index/index.wxml", "<view />");
    project
}

/// IT-12: `</` stack completion, tag snippets, linked editing and the current
/// rename-preparation edit set stay exact for nested same-name siblings and
/// self-closing elements.
#[test]
fn it_12_closing_tag_completion_linked_editing_and_rename_prep() {
    let project = structure_fixture();
    let mut session = Session::start_with(
        &project,
        json!({}),
        json!({
            "textDocument": {
                "completion": {"completionItem": {"snippetSupport": true}},
            },
        }),
        |_| Arc::new(MockBackend::new()) as Arc<dyn Backend>,
    );
    let uri = project.uri("pages/index/index.wxml");

    let closing = "<view><text></";
    session.open_wxml(&uri, closing, 1);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(closing, closing.len()),
        }),
    );
    let completion = session.result(id);
    let visible = labels(&completion);
    assert!(visible.contains(&"view".to_owned()), "{visible:?}");
    assert!(visible.contains(&"text".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"button".to_owned()), "{visible:?}");
    let applied = apply_completion_item(closing, item(&completion, "text"));
    assert_eq!(applied, "<view><text></text", "{applied}");
    let closed = format!("{applied}></view>");
    assert_eq!(closed, "<view><text></text></view>", "{closed}");
    assert_parses_cleanly(&closed);

    // Nested same-name siblings suggest one closing name, not one per depth.
    let nested = "<view><view></";
    session.change(&uri, nested, 2);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(nested, nested.len()),
        }),
    );
    let completion = session.result(id);
    let views = labels(&completion)
        .iter()
        .filter(|label| label.as_str() == "view")
        .count();
    assert_eq!(views, 1, "duplicate closing candidate: {completion}");
    let applied = apply_completion_item(nested, item(&completion, "view"));
    assert_eq!(applied, "<view><view></view", "{applied}");
    assert_parses_cleanly(&format!("{applied}></view>"));

    // A tag snippet closes itself once and keeps the CRLF/indent style.
    let snippet_source = "<!-- 注释 -->\r\n  <vi";
    session.change(&uri, snippet_source, 3);
    let snippet_offset = snippet_source.find("<vi").expect("<vi") + 3;
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(snippet_source, snippet_offset),
        }),
    );
    let completion = session.result(id);
    let snippet = item(&completion, "view (snippet)");
    let close_up = apply_completion_item(snippet_source, snippet);
    assert_eq!(
        close_up, "<!-- 注释 -->\r\n  <view>\r\n    \r\n  </view>",
        "{close_up}"
    );
    assert_eq!(close_up.matches("</view>").count(), 1, "{close_up}");
    assert_parses_cleanly(&close_up);

    let text = "<view><view>x</view></view>";
    session.change(&uri, text, 4);
    let id = session.request(
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(text, 2),
        }),
    );
    let outer = session.result(id);
    let ranges = outer["ranges"].as_array().expect("outer ranges");
    assert_eq!(ranges.len(), 2, "{outer}");
    assert_eq!(byte_range(text, &ranges[0]), (1, 5));
    assert_eq!(byte_range(text, &ranges[1]), (22, 26));
    let id = session.request(
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(text, 16),
        }),
    );
    let inner = session.result(id);
    let inner_ranges = inner["ranges"].as_array().expect("inner ranges");
    assert_eq!(byte_range(text, &inner_ranges[0]), (7, 11));
    assert_eq!(byte_range(text, &inner_ranges[1]), (15, 19));

    // Rename preparation: the linked pair is the edit set. Applying it to the
    // outer pair must leave the nested sibling untouched.
    let mut renamed = text.to_owned();
    let mut edits: Vec<(usize, usize)> =
        ranges.iter().map(|range| byte_range(text, range)).collect();
    edits.sort_by_key(|(start, _)| *start);
    for (start, end) in edits.iter().rev() {
        renamed.replace_range(*start..*end, "section");
    }
    assert_eq!(renamed, "<section><view>x</view></section>");
    assert_parses_cleanly(&renamed);
    assert!(!renamed.contains("section>x</section>"), "{renamed}");

    // Linked editing must not pair self-closing elements.
    let self_closing = "<view><image src=\"x\"/></view>";
    session.change(&uri, self_closing, 5);
    let offset = self_closing.find("image").expect("image") + 2;
    let id = session.request(
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(self_closing, offset),
        }),
    );
    let linked = session.result(id);
    assert!(linked["result"].is_null(), "{linked}");

    session.finish();
}
