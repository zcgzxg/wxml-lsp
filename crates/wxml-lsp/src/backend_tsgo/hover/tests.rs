//! Hover fidelity for declaration-derived `type` + `optionalTypes` unions.

#![allow(clippy::too_many_lines, clippy::unimplemented)]

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::backend_tsgo::query::hover::{HoverEvidence, HoverGap, HoverSymbolFacts};
use crate::backend_tsgo::query::type_info::{TypeInfo, TypeQuality};
use wxml_backend_tsgo::native::handles::BoundNodeHandle;
use wxml_backend_tsgo::shadow::mapping::ShadowMap;
use wxml_backend_tsgo::shadow::mapping::SourceRange as ShadowSourceRange;
use wxml_backend_tsgo::shadow::rewrite::{
    Assembled, EditKind, MountedSegment, ShadowEdit, apply_edits,
};
use wxml_types::{
    BackendEpoch, CapabilityStatus, FileId, HoverRequest, ProjectId, ProjectRevision,
    RequestContext, SnapshotId, SourceRevision, SourceSnapshot, ViewRevision,
};

use super::{DeclaredPropertyType, apply_declared_property_type, hovered_root};
use crate::backend_tsgo::mapping::{FragmentMap, ShadowLayout, TemplateRange};

fn project() -> ProjectId {
    ProjectId::new("optional-types")
}

fn wxml_file() -> FileId {
    FileId::new(project(), "components/card/card.wxml").expect("wxml file")
}

fn request(wxml: &str, offset: u32) -> HoverRequest {
    HoverRequest {
        context: RequestContext::new(
            ProjectRevision::new(project(), 1),
            ViewRevision::new(1),
            BackendEpoch::new(wxml_types::BackendId::new("test"), 1),
            Arc::new(SourceSnapshot::new(
                wxml_file(),
                SourceRevision::new(1),
                SnapshotId::new(1),
                wxml,
            )),
        )
        .expect("context"),
        offset,
    }
}

fn declared(text: &str, complete: bool) -> BTreeMap<String, DeclaredPropertyType> {
    BTreeMap::from([(
        "cardTitle".to_owned(),
        DeclaredPropertyType {
            type_text: text.to_owned(),
            complete,
        },
    )])
}

fn evidence(text: Option<&str>, quality: TypeQuality) -> HoverEvidence {
    HoverEvidence {
        type_info: text.map(|text| TypeInfo {
            text: text.to_owned(),
            handle: 7,
            flags: 1,
        }),
        type_quality: quality,
        symbol: None,
        documentation: None,
        declarations: Vec::new(),
        status: CapabilityStatus::Supported,
        gaps: Vec::new(),
    }
}

/// One mounted expression at the start of the derived text.
fn layout(wxml: &str, mount_text: &str, segments: Vec<MountedSegment>) -> ShadowLayout {
    let assembled = Assembled {
        text: mount_text.to_owned(),
        edits: Vec::new(),
    };
    let map = ShadowMap::new(mount_text, &assembled);
    let range = TemplateRange::from_bounds(3, 12).expect("wxml range");
    ShadowLayout::new(
        wxml,
        mount_text.to_owned(),
        map,
        vec![FragmentMap {
            wxml_range: range,
            mount: wxml_backend_tsgo::shadow::mapping::MountRecord {
                mount_id: "fragment#0".to_owned(),
                fragment_id: "f0".to_owned(),
                insertion_id: "probe".to_owned(),
                text: mount_text.to_owned(),
                start_utf16: 0,
                target_start_utf16: 16,
                target_length_utf16: 9,
                segments,
            },
        }],
    )
}

fn name_segment() -> MountedSegment {
    MountedSegment {
        start_utf16: 16,
        length_utf16: 9,
        source: Some(ShadowSourceRange::new(3, 12)),
    }
}

#[test]
fn a_declared_union_replaces_an_under_reported_native_type() {
    let mut evidence = evidence(Some("string"), TypeQuality::Known);
    evidence.finalize_status();
    apply_declared_property_type(
        &mut evidence,
        &declared("string | number", true),
        Some("cardTitle"),
    );
    assert_eq!(
        evidence.type_info.as_ref().expect("type").text,
        "string | number"
    );
    assert_eq!(evidence.type_quality, TypeQuality::Known);
    assert_eq!(evidence.status, CapabilityStatus::Supported);
}

#[test]
fn incomplete_member_and_undeclared_names_are_never_overridden() {
    let mut incomplete = evidence(Some("string"), TypeQuality::Known);
    apply_declared_property_type(
        &mut incomplete,
        &declared("string | number", false),
        Some("cardTitle"),
    );
    assert_eq!(incomplete.type_info.as_ref().expect("type").text, "string");

    let mut member = evidence(Some("string"), TypeQuality::Known);
    apply_declared_property_type(
        &mut member,
        &declared("string | number", true),
        Some("name"),
    );
    assert_eq!(member.type_info.as_ref().expect("type").text, "string");

    let mut undeclared = evidence(Some("string"), TypeQuality::Known);
    apply_declared_property_type(&mut undeclared, &BTreeMap::new(), Some("cardTitle"));
    assert_eq!(undeclared.type_info.as_ref().expect("type").text, "string");
}

#[test]
fn a_missing_type_with_symbol_evidence_becomes_supported_after_the_override() {
    let mut evidence = evidence(None, TypeQuality::Missing);
    evidence.symbol = Some(HoverSymbolFacts {
        name: "cardTitle".to_owned(),
        flags: 1,
        declarations: Vec::<BoundNodeHandle>::new(),
    });
    evidence.finalize_status();
    assert!(
        evidence
            .gaps
            .iter()
            .any(|gap| matches!(gap, HoverGap::Blocked { .. }))
    );

    apply_declared_property_type(
        &mut evidence,
        &declared("string | number", true),
        Some("cardTitle"),
    );
    assert_eq!(evidence.type_quality, TypeQuality::Known);
    assert!(
        !evidence
            .gaps
            .iter()
            .any(|gap| matches!(gap, HoverGap::Blocked { .. }))
    );
    assert_eq!(evidence.status, CapabilityStatus::Supported);
}

#[test]
fn a_matching_union_is_left_untouched() {
    let mut evidence = evidence(Some("string | number"), TypeQuality::Known);
    evidence.finalize_status();
    apply_declared_property_type(
        &mut evidence,
        &declared("string | number", true),
        Some("cardTitle"),
    );
    assert_eq!(evidence.type_info.as_ref().expect("type").handle, 7);
    assert_eq!(evidence.status, CapabilityStatus::Supported);
}

#[test]
fn only_a_free_root_is_eligible_for_a_property_override() {
    let wxml = "{{ cardTitle }}";
    let root = layout(
        wxml,
        "__wxml_props_0.cardTitle",
        vec![
            MountedSegment {
                start_utf16: 0,
                length_utf16: 16,
                source: None,
            },
            name_segment(),
        ],
    );
    let root_request = request(wxml, 8);
    assert_eq!(
        hovered_root(&root, &root_request, 24).as_deref(),
        Some("cardTitle")
    );

    let member_wxml = "{{ user.name }}";
    let member = layout(
        member_wxml,
        "__wxml_root.user.name",
        vec![MountedSegment {
            start_utf16: 16,
            length_utf16: 4,
            source: Some(ShadowSourceRange::new(8, 12)),
        }],
    );
    let member_request = request(member_wxml, 11);
    assert_eq!(hovered_root(&member, &member_request, 20), None);
}

#[test]
fn a_synthetic_mount_keeps_the_original_script_map_valid() {
    // The layout helper above uses a real `ShadowMap`; a mount insertion must
    // not make the derived text inconsistent for unrelated range mapping.
    let original = "abcdefgh";
    let edit = ShadowEdit::insert("mount", EditKind::Probe, 4, "x");
    let assembled = apply_edits(original, &[edit]).expect("assembled shadow");
    assert!(assembled.text.contains('x'));
}

mod adapter {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use crate::backend_tsgo::query::hover::HoverEvidence;
    use crate::backend_tsgo::query::type_info::{TypeInfo, TypeQuality};
    use wxml_backend_tsgo::input::{
        AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
    };
    use wxml_backend_tsgo::native::NativeError;
    use wxml_backend_tsgo::native::document_symbol::DocumentSymbolsEvidence;
    use wxml_backend_tsgo::native::session::TypeQuery;
    use wxml_backend_tsgo::native::strings::Utf16Position;
    use wxml_backend_tsgo::native::wire::RawCompletionList;
    use wxml_backend_tsgo::native::wire::RawDiagnostic;
    use wxml_backend_tsgo::script::registration::RegistrationKind;
    use wxml_backend_tsgo::script::{ScriptRequest, analyze_script};
    use wxml_backend_tsgo::shadow::layout::{
        BehaviorPropertyOverlay, FragmentSpec, JsCheckPolicy, RootKind, ShadowRequest,
        build_in_place_shadow_with_behavior,
    };
    use wxml_types::{
        BackendEpoch, BackendId, CapabilityStatus, FileId, HoverRequest, MarkupKind, ProjectId,
        ProjectRevision, RequestContext, SnapshotId, SourceRevision, SourceSnapshot, ViewRevision,
    };

    use super::super::hover;
    use crate::backend_tsgo::mapping::{FragmentMap, ShadowLayout, TemplateRange};
    use crate::backend_tsgo::session::QuerySession;

    struct FakeSession {
        evidence: HoverEvidence,
    }

    impl QuerySession for FakeSession {
        fn open_project(&mut self, _input: &AnalysisInput) -> Result<(), NativeError> {
            unimplemented!("not used")
        }

        fn update_project(&mut self, _input: &AnalysisInput) -> Result<(), NativeError> {
            unimplemented!("not used")
        }

        fn semantic_diagnostics(&mut self, _path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
            unimplemented!("not used")
        }

        fn syntactic_diagnostics(
            &mut self,
            _path: &str,
        ) -> Result<Vec<RawDiagnostic>, NativeError> {
            unimplemented!("not used")
        }

        fn type_query(
            &mut self,
            _path: &str,
            _position: Utf16Position,
        ) -> Result<TypeQuery, NativeError> {
            unimplemented!("not used")
        }

        fn hover_evidence(
            &mut self,
            _path: &str,
            _position: Utf16Position,
        ) -> Result<HoverEvidence, NativeError> {
            Ok(self.evidence.clone())
        }

        fn completions(
            &mut self,
            _path: &str,
            _position: Utf16Position,
        ) -> Result<Option<RawCompletionList>, NativeError> {
            unimplemented!("not used")
        }

        fn definition(
            &mut self,
            _path: &str,
            _position: Utf16Position,
        ) -> Result<crate::backend_tsgo::query::definition::DefinitionResult, NativeError> {
            unimplemented!("not used")
        }

        fn document_symbols(
            &mut self,
            _path: &str,
        ) -> Result<DocumentSymbolsEvidence, NativeError> {
            unimplemented!("not used")
        }

        fn shutdown(self: Box<Self>) {}
    }

    fn project() -> ProjectId {
        ProjectId::new("optional-types-adapter")
    }

    fn wxml_file() -> FileId {
        FileId::new(project(), "components/card/card.wxml").expect("wxml file")
    }

    #[test]
    fn a_behavior_property_hovers_as_the_declared_union_through_the_adapter() {
        let wxml = "{{ cardTitle }}";
        let source = r"Component({
  behaviors: [Behavior({
    properties: {
      /** 共享标题 */
      cardTitle: { type: String, optionalTypes: [Number] },
    },
  })],
  data: {},
});";
        let input = AnalysisInput::new(
            ProjectKey::new("optional-types-adapter"),
            ProjectRoot::new("/work/optional-types-adapter").expect("root"),
            SnapshotRevision::new(1),
            vec![
                SourceFile::new("components/card/card.ts", source, DocumentRevision::new(1))
                    .expect("script"),
            ],
        )
        .expect("input");
        let analysis = analyze_script(&ScriptRequest::new(
            &input,
            "components/card/card.ts",
            RegistrationKind::Component,
        ))
        .expect("analysis");
        let fragment = FragmentSpec::new(
            "f0",
            "cardTitle",
            wxml_backend_tsgo::shadow::mapping::SourceRange::new(3, 12),
        );
        let script = SourceFile::new("components/card/card.ts", source, DocumentRevision::new(1))
            .expect("script");
        let overlay = BehaviorPropertyOverlay {
            properties: &analysis.behavior.properties,
            complete: true,
            reason: None,
        };
        let built = build_in_place_shadow_with_behavior(
            &ShadowRequest {
                script: &script,
                root: RootKind::Component,
                fragments: std::slice::from_ref(&fragment),
                js_check: JsCheckPolicy::Inherit,
                check_js: false,
            },
            Some(&overlay),
        )
        .expect("shadow");
        let mount = built
            .view
            .mounts
            .iter()
            .find(|mount| mount.fragment_id == "f0")
            .expect("mount")
            .clone();
        let layout = ShadowLayout::new(
            wxml,
            built.view.text,
            built.view.map,
            vec![FragmentMap {
                wxml_range: TemplateRange::from_bounds(3, 12).expect("range"),
                mount,
            }],
        );
        let declared = BTreeMap::from([(
            "cardTitle".to_owned(),
            super::super::DeclaredPropertyType {
                type_text: "string | number".to_owned(),
                complete: true,
            },
        )]);
        let request = HoverRequest {
            context: RequestContext::new(
                ProjectRevision::new(project(), 1),
                ViewRevision::new(1),
                BackendEpoch::new(BackendId::new("test"), 1),
                Arc::new(SourceSnapshot::new(
                    wxml_file(),
                    SourceRevision::new(1),
                    SnapshotId::new(1),
                    wxml,
                )),
            )
            .expect("context"),
            offset: 8,
        };
        let mut session = FakeSession {
            evidence: HoverEvidence {
                type_info: Some(TypeInfo {
                    text: "string".to_owned(),
                    handle: 1,
                    flags: 1,
                }),
                type_quality: TypeQuality::Known,
                symbol: None,
                documentation: None,
                declarations: Vec::new(),
                status: CapabilityStatus::Supported,
                gaps: Vec::new(),
            },
        };
        let result = hover(
            &mut session,
            &layout,
            "components/card/card.ts",
            &declared,
            None,
            &request,
        )
        .expect("hover");
        assert_eq!(result.markup, MarkupKind::Markdown);
        assert_eq!(result.contents, "```ts\nstring | number\n```");
        assert_eq!(result.status, CapabilityStatus::Supported);
    }

    #[test]
    fn a_member_hover_keeps_the_native_member_type() {
        let wxml = "{{ user.name }}";
        let source = "Component({ properties: { name: { type: String } }, data: {} });";
        let script = SourceFile::new("components/card/card.ts", source, DocumentRevision::new(1))
            .expect("script");
        let fragment = FragmentSpec::new(
            "f0",
            "user.name",
            wxml_backend_tsgo::shadow::mapping::SourceRange::new(3, 12),
        );
        let built = build_in_place_shadow_with_behavior(
            &ShadowRequest {
                script: &script,
                root: RootKind::Component,
                fragments: std::slice::from_ref(&fragment),
                js_check: JsCheckPolicy::Inherit,
                check_js: false,
            },
            None,
        )
        .expect("shadow");
        let mount = built
            .view
            .mounts
            .iter()
            .find(|mount| mount.fragment_id == "f0")
            .expect("mount")
            .clone();
        let layout = ShadowLayout::new(
            wxml,
            built.view.text,
            built.view.map,
            vec![FragmentMap {
                wxml_range: TemplateRange::from_bounds(3, 12).expect("range"),
                mount,
            }],
        );
        let declared = BTreeMap::from([(
            "name".to_owned(),
            super::super::DeclaredPropertyType {
                type_text: "string | number".to_owned(),
                complete: true,
            },
        )]);
        let request = HoverRequest {
            context: RequestContext::new(
                ProjectRevision::new(project(), 1),
                ViewRevision::new(1),
                BackendEpoch::new(BackendId::new("test"), 1),
                Arc::new(SourceSnapshot::new(
                    wxml_file(),
                    SourceRevision::new(1),
                    SnapshotId::new(1),
                    wxml,
                )),
            )
            .expect("context"),
            offset: 11,
        };
        let mut session = FakeSession {
            evidence: HoverEvidence {
                type_info: Some(TypeInfo {
                    text: "string".to_owned(),
                    handle: 1,
                    flags: 1,
                }),
                type_quality: TypeQuality::Known,
                symbol: None,
                documentation: None,
                declarations: Vec::new(),
                status: CapabilityStatus::Supported,
                gaps: Vec::new(),
            },
        };
        let result = hover(
            &mut session,
            &layout,
            "components/card/card.ts",
            &declared,
            None,
            &request,
        )
        .expect("hover");
        assert_eq!(result.contents, "```ts\nstring\n```");
    }
}

mod plan {
    use std::sync::Arc;

    use wxml_text::SourceRange;
    use wxml_types::{
        BackendEpoch, BackendId, CheckFragment, FileId, ProjectId, ProjectRevision, RequestContext,
        RootKind, SnapshotId, SourceRevision, SourceSnapshot, ViewRevision,
    };

    use crate::backend_tsgo::environment::Environment;
    use crate::backend_tsgo::input::{ViewProvider, build_request_plan};
    use wxml_project::ProjectPath;

    use crate::project_host::{HostError, PreparedFile, PreparedView, ViewTarget};

    fn project() -> ProjectId {
        ProjectId::new("optional-types-plan")
    }

    struct Views(PreparedView);

    impl ViewProvider for Views {
        fn prepare(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
            Ok(self.0.clone())
        }
    }

    fn prepared_file(project: &ProjectId, path: &str, text: &str) -> PreparedFile {
        PreparedFile {
            file: FileId::new(project.clone(), path).expect("file id"),
            path: ProjectPath::new(path).expect("project path"),
            kind: "ts",
            text: Arc::from(text),
            source_revision: 1,
        }
    }

    #[test]
    fn the_template_plan_merges_behavior_optional_types_into_hover_facts() {
        let wxml = "{{ cardTitle }}";
        let card = r#"import shared from "./shared";
Component({ behaviors: [shared], data: {} });"#;
        let shared = r"export default Behavior({
  properties: {
    /** 共享标题 */
    cardTitle: { type: String, optionalTypes: [Number] },
  },
});";
        let project = project();
        let files: Arc<[PreparedFile]> = Arc::from(vec![
            prepared_file(&project, "components/card/card.ts", card),
            prepared_file(&project, "components/card/shared.ts", shared),
        ]);
        let target = ViewTarget {
            wxml: ProjectPath::new("components/card/card.wxml").expect("wxml path"),
            owner: RootKind::Component,
            script: ProjectPath::new("components/card/card.ts").expect("script path"),
            script_file: FileId::new(project.clone(), "components/card/card.ts").expect("script"),
            view_revision: ViewRevision::new(1),
        };
        let fragments: Arc<[CheckFragment]> = Arc::from(vec![CheckFragment {
            fragment_id: "f0".to_owned(),
            expr_text: "cardTitle".to_owned(),
            target: None,
            bindings: Vec::new(),
            guards: Vec::new(),
            wxml_range: SourceRange::from_bounds(3, 12).expect("range"),
            fragment_revision: 1,
            root: RootKind::Component,
        }]);
        let view = PreparedView {
            view_id: "card:1".to_owned(),
            target: Some(target),
            files,
            compiler_options: serde_json::Map::new(),
            fragments,
            components: Arc::from(Vec::new()),
            typing: None,
            events: Arc::from(Vec::new()),
            project_root: std::path::PathBuf::from("/work/optional-types-plan"),
            syntax_diagnostics: Vec::new(),
            status_hint: None,
            source_complete: true,
        };
        let context = RequestContext::new(
            ProjectRevision::new(project.clone(), 1),
            ViewRevision::new(1),
            BackendEpoch::new(BackendId::new("test"), 1),
            Arc::new(SourceSnapshot::new(
                FileId::new(project, "components/card/card.wxml").expect("wxml"),
                SourceRevision::new(1),
                SnapshotId::new(1),
                wxml,
            )),
        )
        .expect("context");
        let provider = Views(view);
        let plan = build_request_plan(&provider, &context, &Environment::Legacy).expect("plan");
        let declared = plan
            .declared_properties
            .get("cardTitle")
            .expect("declared behavior property");
        assert_eq!(declared.type_text, "string | number");
        assert!(declared.complete);
        assert!(
            plan.layout
                .derived()
                .contains("cardTitle: string | number;"),
            "{}",
            plan.layout.derived()
        );
        assert!(plan.layout.derived().contains("__wxml_props_0.cardTitle"));
        assert!(
            plan.degradation
                .as_deref()
                .is_none_or(|reason| !reason.contains("behaviors are partially resolved")),
            "{:?}",
            plan.degradation
        );
    }
}

#[test]
fn a_local_with_a_property_name_is_not_a_declaration_root() {
    let wxml = "{{ cardTitle }}";
    // A `wx:for` local is emitted without the free-root prefix; the property
    // declaration must not relabel it.
    let local = layout(
        wxml,
        "cardTitle",
        vec![MountedSegment {
            start_utf16: 0,
            length_utf16: 9,
            source: Some(ShadowSourceRange::new(3, 12)),
        }],
    );
    let request = request(wxml, 8);
    assert_eq!(hovered_root(&local, &request, 8), None);
}
