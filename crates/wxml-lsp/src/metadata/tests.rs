use super::*;

fn target(text: &str, offset: usize) -> Option<Target> {
    let offset = u32::try_from(offset).expect("offset");
    target_at(text, offset)
}

#[test]
fn finds_tag_names_attributes_and_values() {
    let text = "<my-card title=\"hello\"></my-card>";
    match target(text, 4).expect("tag") {
        Target::Tag {
            name,
            closing: false,
            ..
        } => assert_eq!(name, "my-card"),
        other => panic!("unexpected target: {other:?}"),
    }
    match target(text, 14).expect("attribute") {
        Target::Attribute { tag, name, .. } => {
            assert_eq!(tag, "my-card");
            assert_eq!(name, "title");
        }
        other => panic!("unexpected target: {other:?}"),
    }
    match target(text, 18).expect("value") {
        Target::Value {
            attribute, value, ..
        } => {
            assert_eq!(attribute, "title");
            assert_eq!(value, "hello");
        }
        other => panic!("unexpected target: {other:?}"),
    }
}

#[test]
fn mustaches_comments_and_wxs_bodies_have_no_static_target() {
    for (text, offset) in [
        ("<view>{{ user.name }}</view>", 12),
        ("<view><!-- count --></view>", 12),
        ("<wxs module=\"m\">var a = 1;</wxs>", 24),
        ("<view>plain text</view>", 8),
        ("<view title=\"{{ name }}\"></view>", 18),
        ("<view bindtap=\"onTap\"></view>", 18),
    ] {
        assert!(
            target(text, offset).is_none(),
            "{text:?} at {offset} produced a target"
        );
    }
}

#[test]
fn unclosed_mustache_stops_later_static_targets() {
    // Documented limitation: an unterminated mustache consumes the rest of
    // the template, matching the raw-text interpretation. The backend may
    // still answer expression queries.
    let text = "<view>{{ x </view><my-card count=\"1\" />";
    let offset = text.find("count").expect("attribute") + 2;
    assert!(target(text, offset).is_none());
}

#[test]
fn prose_less_than_is_not_a_tag() {
    assert!(target("<view>a < b</view>", 10).is_none());
}

#[test]
fn unicode_offsets_stay_on_character_boundaries() {
    let text = "<view>😀</view><my-card title=\"中\" />";
    let offset = text.find("my-card").expect("tag") + 3;
    assert!(matches!(target(text, offset), Some(Target::Tag { .. })));
    let value = text.find("中").expect("value");
    assert!(matches!(target(text, value), Some(Target::Value { .. })));
}

#[test]
fn closing_tags_have_tag_targets() {
    let text = "<my-card></my-card>";
    match target(text, 13).expect("closing tag") {
        Target::Tag {
            name,
            closing: true,
            ..
        } => assert_eq!(name, "my-card"),
        other => panic!("unexpected target: {other:?}"),
    }
}

fn component_fixture() -> ViewComponent {
    ViewComponent {
        name: "my-card".to_owned(),
        specifier: "../../components/my-card/index".to_owned(),
        kind: ComponentTargetKind::Local,
        component: Some(ProjectPath::new("components/my-card/index").expect("path")),
        wxml: Some(ProjectPath::new("components/my-card/index.wxml").expect("path")),
        json: Some(ProjectPath::new("components/my-card/index.json").expect("path")),
        property_facts: wxml_project::ComponentPropertyFacts {
            properties: vec![wxml_project::ComponentProperty {
                name: "title".to_owned(),
                type_text: "string | number".to_owned(),
                default_text: Some("''".to_owned()),
                documentation: Some("标题".to_owned()),
                script: ProjectPath::new("components/my-card/index.ts").expect("path"),
                range: TextRange::new(20, 25).expect("range"),
            }],
            complete: true,
            reason: None,
        },
        origin: crate::project_host::ComponentOrigin::Local {
            file: Some(ProjectPath::new("pages/index/index.json").expect("path")),
        },
    }
}

#[test]
fn builtin_tags_attributes_and_values_have_static_docs() {
    let text = "<button type=\"primary\" wx:if=\"ok\">";
    let components: Vec<ViewComponent> = Vec::new();
    let tag = target(text, 2).expect("tag");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
        panic!("tag docs");
    };
    assert!(markdown.contains("按钮"), "{markdown}");
    assert!(markdown.contains("[官方文档]"), "{markdown}");
    assert!(markdown.contains("component/button.html"), "{markdown}");

    let attribute = target(text, 10).expect("attribute");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &attribute) else {
        panic!("attribute docs");
    };
    assert!(markdown.contains("primary"), "{markdown}");
    assert!(markdown.contains("default"), "{markdown}");
    assert!(markdown.contains("可选值："), "{markdown}");
    assert!(markdown.contains("\n- `default`：默认按钮"), "{markdown}");
    assert!(!markdown.contains("Values:"), "{markdown}");

    let offset = text.find("primary").expect("value") + 2;
    let value = target(text, offset).expect("value");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &value) else {
        panic!("value docs");
    };
    assert!(markdown.contains("主要按钮"), "{markdown}");

    let offset = text.find("wx:if").expect("directive") + 2;
    let directive = target(text, offset).expect("directive");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &directive) else {
        panic!("directive docs");
    };
    assert!(markdown.contains("条件渲染"), "{markdown}");
    assert!(markdown.contains("WXML 指令"), "{markdown}");
}

#[test]
fn component_tag_and_property_have_static_docs_and_definitions() {
    let components = vec![component_fixture()];
    let text = "<my-card title=\"x\" />";

    let tag = target(text, 3).expect("tag");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
        panic!("component docs");
    };
    assert!(markdown.contains("自定义组件"), "{markdown}");
    assert!(markdown.contains("定义："), "{markdown}");
    assert!(markdown.contains("声明位置："), "{markdown}");
    assert!(
        markdown.contains("components/my-card/index.wxml"),
        "{markdown}"
    );
    assert!(markdown.contains("pages/index/index.json"), "{markdown}");
    assert!(markdown.contains("title"), "{markdown}");
    assert!(markdown.contains("string | number"), "{markdown}");
    assert_eq!(
        definition(&components, &tag),
        DefinitionAnswer::Location(StaticLocation {
            path: ProjectPath::new("components/my-card/index.wxml").expect("path"),
            range: TextRange::new(0, 0).expect("range"),
        })
    );

    let attribute = target(text, text.find("title").expect("attribute") + 2).expect("attribute");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &attribute) else {
        panic!("property docs");
    };
    assert!(markdown.contains("标题"), "{markdown}");
    assert_eq!(
        definition(&components, &attribute),
        DefinitionAnswer::Location(StaticLocation {
            path: ProjectPath::new("components/my-card/index.ts").expect("path"),
            range: TextRange::new(20, 25).expect("range"),
        })
    );
}

#[test]
fn unresolved_components_and_unknown_properties_do_not_fabricate_targets() {
    let mut plugin = component_fixture();
    plugin.kind = ComponentTargetKind::Plugin;
    plugin.component = None;
    plugin.wxml = None;
    plugin.json = None;
    plugin.property_facts.properties.clear();
    plugin.specifier = "plugin://demo/card".to_owned();
    let components = vec![plugin];
    let text = "<my-card title=\"x\" />";
    let tag = target(text, 3).expect("tag");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
        panic!("plugin docs");
    };
    assert!(markdown.contains("插件组件"), "{markdown}");
    assert_eq!(definition(&components, &tag), DefinitionAnswer::None);

    let attribute = target(text, text.find("title").expect("attribute") + 2).expect("attribute");
    assert_eq!(
        definition(&components, &attribute),
        DefinitionAnswer::NotStatic
    );
    let HoverAnswer::Document { markdown, .. } = hover(&components, &attribute) else {
        panic!("unknown property docs");
    };
    assert!(markdown.contains("暂无属性说明"), "{markdown}");
    assert!(
        !markdown.contains("Local metadata is unavailable"),
        "{markdown}"
    );
}

#[test]
fn incomplete_component_properties_are_not_reported_as_empty() {
    let mut component = component_fixture();
    component.property_facts = wxml_project::ComponentPropertyFacts {
        properties: Vec::new(),
        complete: false,
        reason: Some("spread properties were not statically extracted"),
    };
    let components = vec![component];
    let text = "<my-card />";
    let tag = target(text, 3).expect("tag");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
        panic!("component docs");
    };
    assert!(!markdown.contains("未找到静态声明的属性"), "{markdown}");
    assert!(!markdown.contains("属性："), "{markdown}");
    assert!(
        !markdown.contains("spread properties were not statically extracted"),
        "{markdown}"
    );

    let text = "<my-card title=\"x\" />";
    let attribute = target(text, text.find("title").expect("attribute") + 2).expect("attribute");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &attribute) else {
        panic!("incomplete property docs");
    };
    assert!(markdown.contains("暂无属性说明"), "{markdown}");
}

#[test]
fn builtin_tag_names_win_over_same_named_components() {
    let mut component = component_fixture();
    component.name = "view".to_owned();
    let components = vec![component];
    let text = "<view title=\"x\" />";
    let tag = target(text, 2).expect("tag");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
        panic!("builtin tag docs");
    };
    assert!(markdown.contains("官方文档"), "{markdown}");
    assert!(!markdown.contains("自定义组件"), "{markdown}");
    assert_eq!(definition(&components, &tag), DefinitionAnswer::NotStatic);

    let attribute = target(text, text.find("title").expect("attribute") + 2).expect("attribute");
    assert_eq!(hover(&components, &attribute), HoverAnswer::NotStatic);
    assert_eq!(
        definition(&components, &attribute),
        DefinitionAnswer::NotStatic
    );
}

#[test]
fn binder_is_a_component_property_not_an_event_prefix() {
    let mut component = component_fixture();
    component.property_facts.properties[0].name = "binder".to_owned();
    let components = vec![component];
    let text = "<my-card binder=\"x\" />";
    let attribute = target(text, text.find("binder").expect("attribute") + 2).expect("attribute");
    assert_eq!(
        definition(&components, &attribute),
        DefinitionAnswer::Location(StaticLocation {
            path: ProjectPath::new("components/my-card/index.ts").expect("path"),
            range: TextRange::new(20, 25).expect("range"),
        })
    );
    let offset = text.find("binder").expect("value") + "binder=\"".len() + 1;
    let value = target(text, offset).expect("value");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &value) else {
        panic!("property value docs");
    };
    assert!(markdown.contains("标题"), "{markdown}");
}

#[test]
fn unresolved_component_hover_is_one_short_line() {
    let mut component = component_fixture();
    component.wxml = None;
    component.json = None;
    component.property_facts = wxml_project::ComponentPropertyFacts {
        properties: Vec::new(),
        complete: false,
        reason: Some("component metadata is unavailable"),
    };
    let components = vec![component];
    let text = "<my-card />";
    let tag = target(text, 3).expect("tag");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
        panic!("component docs");
    };
    assert!(markdown.contains("未能解析组件路径"), "{markdown}");
    assert!(
        !markdown.contains("properties are unavailable")
            && !markdown.contains("static property list is incomplete")
            && !markdown.contains("属性列表"),
        "{markdown}"
    );
}

#[test]
fn builtin_tag_docs_link_to_the_official_page() {
    let components: Vec<ViewComponent> = Vec::new();
    let text = "<view>";
    let tag = target(text, 2).expect("tag");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
        panic!("tag docs");
    };
    assert!(markdown.contains("视图容器"), "{markdown}");
    assert!(
        markdown.contains(
            "[官方文档](https://developers.weixin.qq.com/miniprogram/dev/component/view.html)"
        ),
        "{markdown}"
    );

    let text = "<wxs module=\"m\">";
    let tag = target(text, 2).expect("tag");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
        panic!("wxs docs");
    };
    assert!(markdown.contains("reference/wxs/"), "{markdown}");
}

#[test]
fn enumerated_values_are_a_bullet_list() {
    let components: Vec<ViewComponent> = Vec::new();
    let text = "<button open-type=\"share\">";
    let attribute =
        target(text, text.find("open-type").expect("attribute") + 2).expect("attribute");
    let HoverAnswer::Document { markdown, .. } = hover(&components, &attribute) else {
        panic!("attribute docs");
    };
    assert!(markdown.contains("可选值：\n"), "{markdown}");
    assert!(
        markdown.contains("\n- `contact`：打开客服会话"),
        "{markdown}"
    );
    assert!(markdown.contains("\n- `share`：分享页面"), "{markdown}");
    assert!(!markdown.contains("Values:"), "{markdown}");
}

#[test]
fn complete_component_facts_leave_unknown_attributes_to_the_backend() {
    let components = vec![component_fixture()];
    let text = "<my-card mystery=\"x\" />";
    let attribute = target(text, text.find("mystery").expect("attribute") + 2).expect("attribute");
    assert_eq!(hover(&components, &attribute), HoverAnswer::NotStatic);
}
