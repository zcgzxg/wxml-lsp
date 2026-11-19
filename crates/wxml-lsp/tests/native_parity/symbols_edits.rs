//! `symbols_edits` native parity partition.
//!
//! The three mandatory native edit gates are authored here. They replace the
//! two `semantic_tools` silent returns and the optional `tsc` branch recorded
//! in `docs/plans/tsgo-test-ledger.json`:
//!
//! - [`native_component_property_edit_applies_and_reparses`] applies a real
//!   component-property rename to an isolated overlay, reparses every applied
//!   template and rechecks the applied snapshot through the native engine.
//! - [`assert_native_edit_reparses_and_typechecks`] is a standalone gate: a
//!   verifiable TypeScript edit must reparse/typecheck cleanly, and a positive
//!   control must produce a real type error. No `tsc`, no Node.
//! - [`native_rejects_cross_namespace_and_dynamic_edits`] proves that
//!   cross-namespace, cross-script and dynamic edits are rejected (or answered
//!   `Partial`) with zero edits and unchanged sources.
//!
//! Every test is `#[ignore]`: the default job reports it ignored, never passed.
//! The required native job runs them with `--ignored`; a missing package or
//! engine in required mode is a hard failure, never a skip.

use std::path::Path;

use serde_json::{Value, json};

use crate::harness::{BlockedGate, Partition};
use crate::native_lsp::{self, Session};
use crate::native_package::NativeGate;

/// Ledger target for this partition (21 planned cases).
pub(crate) const PARTITION: Partition = Partition::new("symbols_edits", 21);

/// Cases with a real Rust assertion: the three authored edit gates.
pub(crate) const MIGRATED_CASES: &[&str] = &[
    "native_component_property_edit_applies_and_reparses",
    "assert_native_edit_reparses_and_typechecks",
    "native_rejects_cross_namespace_and_dynamic_edits",
];

/// Future tests that must run before the old silent edit paths can be removed.
///
/// Empty: every registered replacement is now authored above. The ledger
/// status of the old silent surfaces is still `required_not_migrated` until
/// the required native job records green evidence.
pub(crate) const BLOCKED_NATIVE_GATES: &[BlockedGate] = &[];

/// Workspace-relative file that declares the native edit gates.
pub(crate) const NATIVE_EDIT_GATE_FILE: &str =
    "crates/wxml-lsp/tests/native_parity/symbols_edits.rs";

/// The exact ignored native edit gate set.
pub(crate) const IGNORED_GATE_TESTS: &[(&str, &str)] = &[
    (
        NATIVE_EDIT_GATE_FILE,
        "native_component_property_edit_applies_and_reparses",
    ),
    (
        NATIVE_EDIT_GATE_FILE,
        "assert_native_edit_reparses_and_typechecks",
    ),
    (
        NATIVE_EDIT_GATE_FILE,
        "native_rejects_cross_namespace_and_dynamic_edits",
    ),
];

const COMPONENT_PROPERTY_GATE: &str = "G-P5-E2E-22";
const POST_EDIT_TYPECHECK_GATE: &str = "G-P5-E2E-22 (mandatory post-edit typecheck)";
const REJECTION_GATE: &str = "G-P5-E2E-23";

/// G-P5-E2E-22: a component-property edit applies, reparses and rechecks.
///
/// The `component-bindings` fixture is the frozen E2E-20 project. Renaming the
/// `title` property must edit exactly the declaration in `card.ts`, the
/// component template reference in `card.wxml` and both caller attributes in
/// `pages/index/index.wxml`. The applied overlay is then typechecked by the
/// native engine: the intentional `title="{{ total }}"` fixture mismatch must
/// be reported under the new property name, which proves the engine saw the
/// applied snapshot and not the original disk text.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
#[allow(clippy::too_many_lines, reason = "one end-to-end edit gate")]
fn native_component_property_edit_applies_and_reparses() {
    let Some(gate) = NativeGate::require(COMPONENT_PROPERTY_GATE) else {
        return;
    };
    copy_fixture(&gate, "component-bindings");
    let page = native_lsp::read_project_file(&gate, "pages/index/index.wxml");
    let card_wxml = native_lsp::read_project_file(&gate, "components/card/card.wxml");
    let card_ts = native_lsp::read_project_file(&gate, "components/card/card.ts");

    let mut session = Session::start(&gate);
    let page_uri = gate.uri("pages/index/index.wxml");
    let baseline = session.open_wxml(&page_uri, &page, 1);
    let baseline_errors = error_diagnostics(&baseline);
    assert_eq!(
        baseline_errors.len(),
        2,
        "the frozen fixture must report exactly the property mismatch and the \
         missing handler before the edit: {baseline}"
    );

    let position = token_position(&page, "title=\"{{ total }}\"", "title");
    let rename = session.rename(&page_uri, position, "heading");
    assert!(
        rename.get("error").is_none(),
        "the attributed component property must be renameable: {}",
        session.describe(&rename)
    );
    let changes = rename["result"]["documentChanges"]
        .as_array()
        .unwrap_or_else(|| panic!("rename returned no documentChanges: {rename}"));
    assert_eq!(
        changes.len(),
        3,
        "the rename must edit the declaration, the component template and the caller: {rename}"
    );

    let ts_uri = gate.uri("components/card/card.ts");
    let card_uri = gate.uri("components/card/card.wxml");
    let ts_edits = document_change_edits(&rename, &ts_uri, "E2E-22 rename");
    let card_edits = document_change_edits(&rename, &card_uri, "E2E-22 rename");
    let page_edits = document_change_edits(&rename, &page_uri, "E2E-22 rename");
    assert_eq!(ts_edits.len(), 1, "one declaration edit: {rename}");
    assert_eq!(card_edits.len(), 1, "one component template edit: {rename}");
    assert_eq!(page_edits.len(), 2, "two caller attribute edits: {rename}");

    let declaration = card_ts.find("title:").expect("card.ts declaration");
    assert_exact_edit(
        ts_edits,
        &card_ts,
        declaration,
        "title",
        "heading",
        "E2E-22 card.ts",
    );
    let template = card_wxml
        .find("{{ title }}")
        .expect("card.wxml property reference")
        + "{{ ".len();
    assert_exact_edit(
        card_edits,
        &card_wxml,
        template,
        "title",
        "heading",
        "E2E-22 card.wxml",
    );
    let first_caller = page
        .find("title=\"{{ pageTitle }}\"")
        .expect("first caller binding");
    let second_caller = page
        .find("title=\"{{ total }}\"")
        .expect("second caller binding");
    assert_exact_edit(
        page_edits,
        &page,
        first_caller,
        "title",
        "heading",
        "E2E-22 caller pageTitle",
    );
    assert_exact_edit(
        page_edits,
        &page,
        second_caller,
        "title",
        "heading",
        "E2E-22 caller total",
    );

    let applied_ts = apply_edits(&card_ts, ts_edits);
    let applied_card = apply_edits(&card_wxml, card_edits);
    let applied_page = apply_edits(&page, page_edits);
    assert!(
        applied_ts.contains("heading: { type: String, value: '' }"),
        "the declaration must be renamed exactly once: {applied_ts}"
    );
    assert!(
        !applied_ts.contains("title: { type: String"),
        "the original property must be gone: {applied_ts}"
    );
    assert!(
        applied_card.contains("{{ heading }}") && !applied_card.contains("{{ title }}"),
        "the component template must use the new property: {applied_card}"
    );
    assert!(
        applied_page.contains("heading=\"{{ pageTitle }}\"")
            && applied_page.contains("heading=\"{{ total }}\""),
        "both caller attributes must be renamed: {applied_page}"
    );
    assert!(
        !applied_page.contains("title=\"{{"),
        "no caller attribute may keep the old name: {applied_page}"
    );
    assert!(
        wxml_syntax::parse(&applied_page).errors.is_empty(),
        "the applied caller template must reparse cleanly"
    );
    assert!(
        wxml_syntax::parse(&applied_card).errors.is_empty(),
        "the applied component template must reparse cleanly"
    );

    // Recheck the applied snapshot through the native engine without writing
    // any file. The script, the component template and the caller all have to
    // be visible to the check.
    let ts_diagnostics = session.open_script(&ts_uri, &applied_ts, 1);
    assert_no_errors(&ts_diagnostics, "applied card.ts");
    let card_diagnostics = session.open_wxml(&card_uri, &applied_card, 1);
    assert_no_errors(&card_diagnostics, "applied card.wxml");
    let page_diagnostics = session.change_wxml(&page_uri, &applied_page, 2);
    let errors = error_diagnostics(&page_diagnostics);
    assert_eq!(
        errors.len(),
        2,
        "the applied page must keep exactly the frozen fixture diagnostics: {page_diagnostics}"
    );
    let property_error = errors
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("component property \"heading\""))
        })
        .unwrap_or_else(|| {
            panic!("the native check did not see the renamed property: {page_diagnostics}")
        });
    let message = property_error["message"]
        .as_str()
        .expect("property diagnostic message");
    assert!(
        message.contains("expects string") && message.contains("got number"),
        "the applied property type must stay frozen: {property_error}"
    );
    assert!(
        errors.iter().any(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("missingHandler"))
        }),
        "the unrelated missing handler must stay reported: {page_diagnostics}"
    );
    assert!(
        !page_diagnostics
            .to_string()
            .contains("component property \"title\""),
        "the native check must use the applied property name, not the old disk text: \
         {page_diagnostics}"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();

    for (relative, expected) in [
        ("pages/index/index.wxml", &page),
        ("components/card/card.wxml", &card_wxml),
        ("components/card/card.ts", &card_ts),
    ] {
        assert_eq!(
            &native_lsp::read_project_file(&gate, relative),
            expected,
            "the rename must not write {relative}"
        );
    }
}

/// G-P5-E2E-22: every applied edit is reparsed and typechecked natively.
///
/// This is a standalone `#[test]`, not a helper: a helper-only name can never
/// satisfy the partition manifest. The edit renames a page data field and all
/// of its references. A clean applied snapshot must produce zero errors, and a
/// deliberately injected type error must produce a real one. No optional
/// post-edit branch and no Node subprocess are involved.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
#[allow(clippy::too_many_lines, reason = "one end-to-end post-edit gate")]
fn assert_native_edit_reparses_and_typechecks() {
    let Some(gate) = NativeGate::require(POST_EDIT_TYPECHECK_GATE) else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    let wxml = "<view>{{ title }}</view>";
    gate.write("pages/index/index.wxml", wxml);
    let original = concat!(
        "Page({\n",
        "  data: {\n",
        "    count: 1,\n",
        "    title: 'ready',\n",
        "  },\n",
        "  onLoad() {\n",
        "    const label: string = this.data.title;\n",
        "    const total: number = this.data.count;\n",
        "    this.setData({ count: total + 1, title: label });\n",
        "  },\n",
        "});\n",
    );
    gate.write("pages/index/index.ts", original);

    let mut session = Session::start(&gate);
    let page_uri = gate.uri("pages/index/index.wxml");
    session.open_wxml(&page_uri, wxml, 1);
    let uri = gate.uri("pages/index/index.ts");
    let baseline = session.open_script(&uri, original, 1);
    assert_no_errors(&baseline, "the unedited script");

    let offsets = identifier_offsets(original, "count");
    assert_eq!(
        offsets.len(),
        3,
        "the fixture must contain exactly the declaration, read and setData key: {offsets:?}"
    );
    let edits: Vec<(usize, usize, &str)> = offsets
        .iter()
        .map(|start| (*start, *start + "count".len(), "total"))
        .collect();
    for (start, end, replacement) in &edits {
        assert_eq!(
            original.get(*start..*end),
            Some("count"),
            "the edit must replace the exact `count` token"
        );
        assert_eq!(*replacement, "total");
    }
    let applied = apply_byte_edits(original, &edits);
    assert!(
        !applied.contains("count"),
        "the applied snapshot must not keep the old field name: {applied}"
    );
    for (start, _end, replacement) in &edits {
        assert_eq!(
            applied.get(*start..*start + replacement.len()),
            Some(*replacement),
            "the applied snapshot must contain the new name at the exact range"
        );
    }

    let clean = session.change_script(&uri, &applied, 2);
    assert_no_errors(&clean, "the applied clean edit");

    // Positive control: an empty list must never be mistaken for a successful
    // typecheck. Injecting a real type error must be reported.
    let needle = "const total: number = this.data.total;";
    assert!(
        applied.contains(needle),
        "the applied edit must keep the typed local for the control: {applied}"
    );
    let broken = applied.replace(needle, "const total: string = 1;");
    assert_ne!(broken, applied);
    let diagnostics = session.change_script(&uri, &broken, 3);
    let errors = error_diagnostics(&diagnostics);
    assert!(
        !errors.is_empty(),
        "the positive control must produce a real type error: {diagnostics}"
    );
    assert!(
        errors.iter().any(|diagnostic| {
            diagnostic["message"].as_str().is_some_and(|message| {
                message.contains("not assignable")
                    && message.contains("string")
                    && message.contains("number")
            })
        }),
        "the control error must be the injected string/number mismatch: {diagnostics}"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
    assert_eq!(
        native_lsp::read_project_file(&gate, "pages/index/index.ts"),
        original,
        "applying the edit must not write the script"
    );
}

/// G-P5-E2E-23: cross-namespace and dynamic edits stay rejected.
///
/// The project mixes a component property and data namespace, a page data path
/// referenced from another script, a destructuring shorthand overlay, a dynamic
/// handler binding and a non-literal page registration. Every rename must be a
/// `-32803` rejection (or a `Partial` result with zero edits), and no source
/// file may change.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
#[allow(clippy::too_many_lines, reason = "one end-to-end rejection gate")]
fn native_rejects_cross_namespace_and_dynamic_edits() {
    let Some(gate) = NativeGate::require(REJECTION_GATE) else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/nonliteral/nonliteral"],"usingComponents":{"my-card":"/components/card/card"}}"#,
    );
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    gate.write("components/card/card.json", r#"{"component":true}"#);
    gate.write(
        "components/card/card.ts",
        concat!(
            "Component({\n",
            "  properties: { title: { type: String }, heading: { type: String } },\n",
            "  data: { local: 'x', user: { name: 'x' } },\n",
            "  methods: { onTap() {} },\n",
            "});",
        ),
    );
    gate.write(
        "components/card/card.wxml",
        "<view>{{ title }} {{ user.name }}</view>",
    );
    gate.write(
        "pages/index/index.ts",
        concat!(
            "Page({\n",
            "  data: { user: { name: 'x' }, alpha: 1, dynamicHandler: 'onTap' },\n",
            "  onTap() {},\n",
            "});",
        ),
    );
    let page = concat!(
        "<my-card title=\"x\"></my-card>\n",
        "<view>{{ user.name }}</view>\n",
        "<view bindtap=\"{{dynamicHandler}}\"></view>\n",
        "<view>{{ alpha }}</view>",
    );
    gate.write("pages/index/index.wxml", page);
    gate.write(
        "other.ts",
        "export function update(page: any) { page.setData({ user: { name: 'y' } }); }",
    );
    gate.write(
        "pages/nonliteral/nonliteral.ts",
        "const options = { data: { label: 'x' }, onNonLiteralTap() {} };\nPage(options);",
    );
    let nonliteral = "<view><button bindtap=\"onNonLiteralTap\">x</button></view>";
    gate.write("pages/nonliteral/nonliteral.wxml", nonliteral);

    let original = [
        ("app.json", gate.project_file("app.json")),
        ("tsconfig.json", gate.project_file("tsconfig.json")),
        (
            "components/card/card.json",
            gate.project_file("components/card/card.json"),
        ),
        (
            "components/card/card.ts",
            gate.project_file("components/card/card.ts"),
        ),
        (
            "components/card/card.wxml",
            gate.project_file("components/card/card.wxml"),
        ),
        (
            "pages/index/index.ts",
            gate.project_file("pages/index/index.ts"),
        ),
        (
            "pages/index/index.wxml",
            gate.project_file("pages/index/index.wxml"),
        ),
        ("other.ts", gate.project_file("other.ts")),
        (
            "pages/nonliteral/nonliteral.ts",
            gate.project_file("pages/nonliteral/nonliteral.ts"),
        ),
        (
            "pages/nonliteral/nonliteral.wxml",
            gate.project_file("pages/nonliteral/nonliteral.wxml"),
        ),
    ]
    .map(|(relative, path)| {
        (
            relative,
            std::fs::read_to_string(path).expect("gate source file"),
        )
    });

    let mut session = Session::start(&gate);
    let page_uri = gate.uri("pages/index/index.wxml");
    session.open_wxml(&page_uri, page, 1);

    // Positive control: the same project/target path must be able to produce a
    // real edit. Without this, a completely unanalyzable project could reject
    // every case below for the wrong reason.
    let alpha = token_position(page, "{{ alpha }}", "alpha");
    let valid = session.rename(&page_uri, alpha, "beta");
    assert!(
        valid.get("error").is_none(),
        "the page data field must be renameable before the rejection cases: {}",
        session.describe(&valid)
    );
    let valid_edits = document_change_edits(&valid, &page_uri, "E2E-23 positive control");
    assert_eq!(
        valid_edits.len(),
        1,
        "the positive control must edit exactly the data token: {valid}"
    );
    assert_exact_edit(
        valid_edits,
        page,
        page.find("{{ alpha }}").expect("alpha reference") + "{{ ".len(),
        "alpha",
        "beta",
        "E2E-23 positive control",
    );
    assert!(
        apply_edits(page, valid_edits).contains("{{ beta }}"),
        "the positive control edit must apply: {valid}"
    );

    // 1. Component property -> component data namespace.
    let property = token_position(page, "title=\"x\"", "title");
    let response = session.rename(&page_uri, property, "local");
    assert_rejected_or_partial(&session, &response, "property -> component data");

    // 2. Component data -> component property namespace.
    let card = native_lsp::read_project_file(&gate, "components/card/card.wxml");
    let card_uri = gate.uri("components/card/card.wxml");
    session.open_wxml(&card_uri, &card, 1);
    let component_data = token_position(&card, "{{ user.name }}", "user");
    let response = session.rename(&card_uri, component_data, "heading");
    assert_rejected_or_partial(&session, &response, "component data -> property");

    // 3. A page data path referenced from another script's nested setData.
    let page_name = token_position(page, "{{ user.name }}", "name");
    let response = session.rename(&page_uri, page_name, "title");
    assert_rejected_or_partial(&session, &response, "cross-script nested setData");

    // 4. A destructuring shorthand overlay would change a binding shape.
    let shorthand = concat!(
        "Page({\n",
        "  data: { user: { name: 'x' }, alpha: 1, dynamicHandler: 'onTap' },\n",
        "  onLoad() { const { user } = this.data; const label = user.name; this.setData({ 'user.name': label }); },\n",
        "  onTap() {},\n",
        "});",
    );
    let script_uri = gate.uri("pages/index/index.ts");
    session.open_script(&script_uri, shorthand, 1);
    let page_user = token_position(page, "{{ user.name }}", "user");
    let response = session.rename(&page_uri, page_user, "account");
    assert_rejected_or_partial(&session, &response, "destructuring shorthand");

    // 5. A dynamic handler binding has no static rename target.
    let dynamic = token_position(page, "bindtap=\"{{dynamicHandler}}\"", "dynamicHandler");
    let response = session.rename(&page_uri, dynamic, "handleTap");
    assert_rejected_or_partial(&session, &response, "dynamic handler binding");

    // 6. A non-literal page registration has no provable handler declaration.
    let nonliteral_uri = gate.uri("pages/nonliteral/nonliteral.wxml");
    session.open_wxml(&nonliteral_uri, nonliteral, 1);
    let handler = token_position(nonliteral, "bindtap=\"onNonLiteralTap\"", "onNonLiteralTap");
    let response = session.rename(&nonliteral_uri, handler, "handleTap");
    assert_rejected_or_partial(&session, &response, "non-literal registration handler");

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();

    for (relative, text) in original {
        assert_eq!(
            native_lsp::read_project_file(&gate, relative),
            text,
            "a rejected rename must not write {relative}"
        );
    }
}

/// Copies one repository fixture into the disposable gate project.
fn copy_fixture(gate: &NativeGate, fixture: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures")
        .join(fixture);
    copy_fixture_tree(&root, &root, gate);
}

fn copy_fixture_tree(root: &Path, path: &Path, gate: &NativeGate) {
    for entry in std::fs::read_dir(path).expect("fixture directory") {
        let entry = entry.expect("fixture entry");
        let path = entry.path();
        if path.is_dir() {
            copy_fixture_tree(root, &path, gate);
        } else {
            let relative = path.strip_prefix(root).expect("fixture relative");
            let text = std::fs::read_to_string(&path).expect("fixture text");
            gate.write(&relative.to_string_lossy(), &text);
        }
    }
}

/// Returns the UTF-16 LSP position of `target` inside the first `marker`.
fn token_position(text: &str, marker: &str, target: &str) -> (u32, u32) {
    let marker_at = native_lsp::lsp_position(text, marker);
    let target_at = native_lsp::lsp_position(marker, target);
    (marker_at.0 + target_at.0, marker_at.1 + target_at.1)
}

/// Returns the edits for one `WorkspaceEdit.documentChanges` URI.
fn document_change_edits<'a>(response: &'a Value, uri: &str, context: &str) -> &'a [Value] {
    let changes = response["result"]["documentChanges"]
        .as_array()
        .unwrap_or_else(|| panic!("{context}: no documentChanges: {response}"));
    let entry = changes
        .iter()
        .find(|entry| entry["textDocument"]["uri"] == json!(uri))
        .unwrap_or_else(|| panic!("{context}: no documentChanges entry for {uri}: {response}"));
    entry["edits"]
        .as_array()
        .unwrap_or_else(|| panic!("{context}: entry has no edits: {entry}"))
}

/// Asserts one exact edit covers `old` at `start` and replaces it with `new`.
fn assert_exact_edit(
    edits: &[Value],
    text: &str,
    start: usize,
    old: &str,
    new: &str,
    context: &str,
) {
    assert_eq!(
        text.get(start..start + old.len()),
        Some(old),
        "{context}: the expected original token moved"
    );
    let (start_line, start_character) = native_lsp::position_at(text, start);
    let (end_line, end_character) = native_lsp::position_at(text, start + old.len());
    let found = edits.iter().find(|edit| {
        edit["range"]["start"] == json!({"line": start_line, "character": start_character})
            && edit["range"]["end"] == json!({"line": end_line, "character": end_character})
            && edit["newText"] == json!(new)
    });
    assert!(
        found.is_some(),
        "{context}: no exact `{old}` -> `{new}` edit at {start}: {edits:?}"
    );
}

/// Applies LSP edits to an immutable text snapshot using UTF-16 positions.
fn apply_edits(text: &str, edits: &[Value]) -> String {
    let mut ranges: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|edit| {
            let start = usize::try_from(edit["range"]["start"]["line"].as_u64().expect("line"))
                .expect("line fits");
            let start_character =
                usize::try_from(edit["range"]["start"]["character"].as_u64().expect("char"))
                    .expect("char fits");
            let end = usize::try_from(edit["range"]["end"]["line"].as_u64().expect("line"))
                .expect("line fits");
            let end_character =
                usize::try_from(edit["range"]["end"]["character"].as_u64().expect("char"))
                    .expect("char fits");
            (
                byte_from_position(text, start, start_character),
                byte_from_position(text, end, end_character),
                edit["newText"].as_str().expect("newText").to_owned(),
            )
        })
        .collect();
    ranges.sort_by_key(|(start, end, _)| (*start, *end));
    let mut output = text.to_owned();
    for (start, end, replacement) in ranges.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    output
}

fn byte_from_position(text: &str, line: usize, character: usize) -> usize {
    let mut offset = 0_usize;
    for (index, segment) in text.split_inclusive('\n').enumerate() {
        if index == line {
            let mut units = 0_usize;
            for (byte, ch) in segment.char_indices() {
                if units == character {
                    return offset + byte;
                }
                units += ch.len_utf16();
                if units > character {
                    return offset + byte;
                }
            }
            return offset + segment.len();
        }
        offset += segment.len();
    }
    offset
}

/// Returns the byte ranges of the exact identifier occurrences.
fn identifier_offsets(text: &str, name: &str) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut index = 0_usize;
    while let Some(relative) = text.get(index..).and_then(|rest| rest.find(name)) {
        let start = index + relative;
        let end = start + name.len();
        let before_ok = start == 0
            || text
                .as_bytes()
                .get(start - 1)
                .is_none_or(|byte| !is_identifier_byte(*byte));
        let after_ok = text
            .as_bytes()
            .get(end)
            .is_none_or(|byte| !is_identifier_byte(*byte));
        if before_ok && after_ok {
            offsets.push(start);
        }
        index = end;
    }
    offsets
}

const fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// Applies exact byte-range edits to an immutable snapshot.
fn apply_byte_edits(text: &str, edits: &[(usize, usize, &str)]) -> String {
    let mut output = text.to_owned();
    for (start, end, replacement) in edits.iter().rev() {
        assert!(start <= end, "edit range must be ordered");
        output.replace_range(*start..*end, replacement);
    }
    output
}

/// Returns only error-severity diagnostics.
fn error_diagnostics(diagnostics: &Value) -> Vec<&Value> {
    diagnostics
        .as_array()
        .unwrap_or_else(|| panic!("diagnostics are not an array: {diagnostics}"))
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect()
}

fn assert_no_errors(diagnostics: &Value, context: &str) {
    let errors = error_diagnostics(diagnostics);
    assert!(
        errors.is_empty(),
        "{context} reported errors: {diagnostics}"
    );
}

/// Accepts only a `-32803` rejection or a `Partial` result with zero edits.
fn assert_rejected_or_partial(session: &Session, response: &Value, context: &str) {
    if let Some(error) = response.get("error") {
        assert_eq!(
            error["code"],
            json!(-32_803),
            "{context}: expected a checked rejection: {}",
            session.describe(response)
        );
        assert!(
            response.get("result").is_none(),
            "{context}: a rejected rename must not carry a result: {}",
            session.describe(response)
        );
        return;
    }
    let edits = response["result"]["documentChanges"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        edits,
        0,
        "{context}: a Partial rename must return zero edits: {}",
        session.describe(response)
    );
}
