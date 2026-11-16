//! Static WXML catalog for completion.
//!
//! Provenance: `docs/reference/wxml-language-feature-baseline.md` (builtin
//! tags, common attributes, enumerated values, `wx:` directives and event
//! bindings). The tables are intentionally a *bounded subset*, not a claim of
//! full Mini Program coverage; unknown tags/attributes keep working in the
//! template and simply produce no static suggestions.

/// One enumerated attribute value.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ValueSpec {
    /// Literal inserted into the document.
    pub(crate) value: &'static str,
    /// Short explanation shown in the completion list.
    pub(crate) detail: &'static str,
}

/// One supported attribute.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AttributeSpec {
    /// Attribute name.
    pub(crate) name: &'static str,
    /// Short explanation.
    pub(crate) detail: &'static str,
}

/// One builtin tag and its attributes/values.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TagSpec {
    /// Tag name.
    pub(crate) name: &'static str,
    /// Short explanation.
    pub(crate) detail: &'static str,
    /// Attributes that are specific to this tag.
    pub(crate) attributes: &'static [AttributeSpec],
    /// Enumerated values keyed by attribute name.
    pub(crate) values: &'static [(&'static str, &'static [ValueSpec])],
}

const ATTR_ID: AttributeSpec = AttributeSpec {
    name: "id",
    detail: "Element id",
};
const ATTR_CLASS: AttributeSpec = AttributeSpec {
    name: "class",
    detail: "Style class list",
};
const ATTR_STYLE: AttributeSpec = AttributeSpec {
    name: "style",
    detail: "Inline style",
};
const ATTR_HIDDEN: AttributeSpec = AttributeSpec {
    name: "hidden",
    detail: "Hide the element",
};

/// Attributes accepted by every element.
pub(crate) const GLOBAL_ATTRIBUTES: &[AttributeSpec] =
    &[ATTR_ID, ATTR_CLASS, ATTR_STYLE, ATTR_HIDDEN];

/// Commonly bound component events (`bindtap`, `catchtap`, ...).
pub(crate) const EVENT_NAMES: &[&str] = &[
    "tap",
    "longpress",
    "longtap",
    "touchstart",
    "touchmove",
    "touchend",
    "touchcancel",
    "input",
    "change",
    "confirm",
    "blur",
    "focus",
    "submit",
    "reset",
    "scroll",
    "scrolltoupper",
    "scrolltolower",
    "load",
    "error",
    "message",
    "animationstart",
    "animationiteration",
    "animationend",
    "transitionend",
    "cancel",
    "columnchange",
    "timeupdate",
    "regionchange",
    "animationfinish",
    "linechange",
];

/// Event binding attributes, including the `bind:`/`catch:` forms.
pub(crate) const EVENT_ATTRIBUTES: &[AttributeSpec] = &[
    AttributeSpec {
        name: "bindtap",
        detail: "Bind tap (bubbling)",
    },
    AttributeSpec {
        name: "catchtap",
        detail: "Bind tap and stop bubbling",
    },
    AttributeSpec {
        name: "bind:tap",
        detail: "Bind tap (bubbling, alternate form)",
    },
    AttributeSpec {
        name: "catch:tap",
        detail: "Bind tap and stop bubbling (alternate form)",
    },
    AttributeSpec {
        name: "capture-bind:tap",
        detail: "Capture-phase tap binding",
    },
    AttributeSpec {
        name: "capture-catch:tap",
        detail: "Capture-phase tap that stops propagation",
    },
    AttributeSpec {
        name: "mut-bind:tap",
        detail: "Mutually exclusive tap binding",
    },
    AttributeSpec {
        name: "bindinput",
        detail: "Bind input",
    },
    AttributeSpec {
        name: "bindchange",
        detail: "Bind value change",
    },
    AttributeSpec {
        name: "bindconfirm",
        detail: "Bind confirm",
    },
    AttributeSpec {
        name: "bindsubmit",
        detail: "Bind form submit",
    },
    AttributeSpec {
        name: "bindreset",
        detail: "Bind form reset",
    },
    AttributeSpec {
        name: "bindscroll",
        detail: "Bind scroll",
    },
    AttributeSpec {
        name: "bindload",
        detail: "Bind load",
    },
    AttributeSpec {
        name: "binderror",
        detail: "Bind error",
    },
    AttributeSpec {
        name: "bindtouchstart",
        detail: "Bind touch start",
    },
    AttributeSpec {
        name: "bindtouchmove",
        detail: "Bind touch move",
    },
    AttributeSpec {
        name: "bindtouchend",
        detail: "Bind touch end",
    },
];

/// `wx:` directives.
pub(crate) const DIRECTIVE_ATTRIBUTES: &[AttributeSpec] = &[
    AttributeSpec {
        name: "wx:if",
        detail: "Conditional rendering",
    },
    AttributeSpec {
        name: "wx:elif",
        detail: "Else-if branch",
    },
    AttributeSpec {
        name: "wx:else",
        detail: "Else branch",
    },
    AttributeSpec {
        name: "wx:for",
        detail: "List rendering source",
    },
    AttributeSpec {
        name: "wx:for-item",
        detail: "List item variable name",
    },
    AttributeSpec {
        name: "wx:for-index",
        detail: "List index variable name",
    },
    AttributeSpec {
        name: "wx:key",
        detail: "List key expression",
    },
];

const TAG_VIEW: TagSpec = TagSpec {
    name: "view",
    detail: "Block container",
    attributes: &[
        AttributeSpec {
            name: "hover-class",
            detail: "Class applied while pressed",
        },
        AttributeSpec {
            name: "hover-stop-propagation",
            detail: "Stop hover propagation",
        },
        AttributeSpec {
            name: "hover-start-time",
            detail: "Hover delay in ms",
        },
        AttributeSpec {
            name: "hover-stay-time",
            detail: "Hover duration in ms",
        },
    ],
    values: &[(
        "hover-stop-propagation",
        &[
            ValueSpec {
                value: "true",
                detail: "Enable",
            },
            ValueSpec {
                value: "false",
                detail: "Disable",
            },
        ],
    )],
};

const TAG_TEXT: TagSpec = TagSpec {
    name: "text",
    detail: "Inline text",
    attributes: &[
        AttributeSpec {
            name: "selectable",
            detail: "Allow text selection",
        },
        AttributeSpec {
            name: "user-select",
            detail: "Allow text selection (newer)",
        },
        AttributeSpec {
            name: "space",
            detail: "Whitespace handling",
        },
        AttributeSpec {
            name: "decode",
            detail: "Decode HTML entities",
        },
    ],
    values: &[(
        "space",
        &[
            ValueSpec {
                value: "ensp",
                detail: "En space",
            },
            ValueSpec {
                value: "emsp",
                detail: "Em space",
            },
            ValueSpec {
                value: "nbsp",
                detail: "Non-breaking space",
            },
        ],
    )],
};

const BUTTON_TYPES: &[ValueSpec] = &[
    ValueSpec {
        value: "default",
        detail: "Default button",
    },
    ValueSpec {
        value: "primary",
        detail: "Primary button",
    },
    ValueSpec {
        value: "warn",
        detail: "Warning button",
    },
];

const BUTTON_SIZES: &[ValueSpec] = &[
    ValueSpec {
        value: "default",
        detail: "Default size",
    },
    ValueSpec {
        value: "mini",
        detail: "Mini size",
    },
];

const TAG_BUTTON: TagSpec = TagSpec {
    name: "button",
    detail: "Button",
    attributes: &[
        AttributeSpec {
            name: "type",
            detail: "Visual type",
        },
        AttributeSpec {
            name: "size",
            detail: "Button size",
        },
        AttributeSpec {
            name: "plain",
            detail: "Hollow style",
        },
        AttributeSpec {
            name: "disabled",
            detail: "Disable the button",
        },
        AttributeSpec {
            name: "loading",
            detail: "Show a loading indicator",
        },
        AttributeSpec {
            name: "form-type",
            detail: "Form submit/reset role",
        },
        AttributeSpec {
            name: "open-type",
            detail: "Mini Program open capability",
        },
        AttributeSpec {
            name: "hover-class",
            detail: "Class applied while pressed",
        },
    ],
    values: &[
        ("type", BUTTON_TYPES),
        ("size", BUTTON_SIZES),
        (
            "form-type",
            &[
                ValueSpec {
                    value: "submit",
                    detail: "Submit the form",
                },
                ValueSpec {
                    value: "reset",
                    detail: "Reset the form",
                },
            ],
        ),
        (
            "open-type",
            &[
                ValueSpec {
                    value: "contact",
                    detail: "Open customer service",
                },
                ValueSpec {
                    value: "share",
                    detail: "Share the page",
                },
                ValueSpec {
                    value: "getPhoneNumber",
                    detail: "Request a phone number",
                },
                ValueSpec {
                    value: "getUserInfo",
                    detail: "Request user information",
                },
                ValueSpec {
                    value: "openSetting",
                    detail: "Open settings",
                },
                ValueSpec {
                    value: "feedback",
                    detail: "Open feedback",
                },
            ],
        ),
    ],
};

const TAG_IMAGE: TagSpec = TagSpec {
    name: "image",
    detail: "Image",
    attributes: &[
        AttributeSpec {
            name: "src",
            detail: "Image URL",
        },
        AttributeSpec {
            name: "mode",
            detail: "Crop/scale mode",
        },
        AttributeSpec {
            name: "lazy-load",
            detail: "Lazy loading",
        },
        AttributeSpec {
            name: "binderror",
            detail: "Load error",
        },
        AttributeSpec {
            name: "bindload",
            detail: "Load complete",
        },
    ],
    values: &[(
        "mode",
        &[
            ValueSpec {
                value: "scaleToFill",
                detail: "Stretch to fill",
            },
            ValueSpec {
                value: "aspectFit",
                detail: "Fit inside, keep ratio",
            },
            ValueSpec {
                value: "aspectFill",
                detail: "Fill, keep ratio",
            },
            ValueSpec {
                value: "widthFix",
                detail: "Fix width, auto height",
            },
            ValueSpec {
                value: "heightFix",
                detail: "Fix height, auto width",
            },
            ValueSpec {
                value: "center",
                detail: "Center without scaling",
            },
            ValueSpec {
                value: "top",
                detail: "Align top",
            },
            ValueSpec {
                value: "bottom",
                detail: "Align bottom",
            },
            ValueSpec {
                value: "left",
                detail: "Align left",
            },
            ValueSpec {
                value: "right",
                detail: "Align right",
            },
        ],
    )],
};

const INPUT_TYPES: &[ValueSpec] = &[
    ValueSpec {
        value: "text",
        detail: "Text input",
    },
    ValueSpec {
        value: "number",
        detail: "Numeric input",
    },
    ValueSpec {
        value: "idcard",
        detail: "Identity card input",
    },
    ValueSpec {
        value: "digit",
        detail: "Digit input with decimal point",
    },
    ValueSpec {
        value: "safe-password",
        detail: "Password input",
    },
    ValueSpec {
        value: "nickname",
        detail: "Nickname input",
    },
];

const TAG_INPUT: TagSpec = TagSpec {
    name: "input",
    detail: "Single-line input",
    attributes: &[
        AttributeSpec {
            name: "type",
            detail: "Input type",
        },
        AttributeSpec {
            name: "value",
            detail: "Current value",
        },
        AttributeSpec {
            name: "placeholder",
            detail: "Placeholder text",
        },
        AttributeSpec {
            name: "disabled",
            detail: "Disable the input",
        },
        AttributeSpec {
            name: "maxlength",
            detail: "Maximum length",
        },
        AttributeSpec {
            name: "focus",
            detail: "Focus the input",
        },
        AttributeSpec {
            name: "confirm-type",
            detail: "Keyboard confirm label",
        },
        AttributeSpec {
            name: "password",
            detail: "Password field",
        },
    ],
    values: &[
        ("type", INPUT_TYPES),
        (
            "confirm-type",
            &[
                ValueSpec {
                    value: "send",
                    detail: "Send",
                },
                ValueSpec {
                    value: "search",
                    detail: "Search",
                },
                ValueSpec {
                    value: "next",
                    detail: "Next",
                },
                ValueSpec {
                    value: "go",
                    detail: "Go",
                },
                ValueSpec {
                    value: "done",
                    detail: "Done",
                },
            ],
        ),
    ],
};

const TAG_PICKER: TagSpec = TagSpec {
    name: "picker",
    detail: "Selector",
    attributes: &[
        AttributeSpec {
            name: "mode",
            detail: "Picker mode",
        },
        AttributeSpec {
            name: "range",
            detail: "Range data",
        },
        AttributeSpec {
            name: "range-key",
            detail: "Object key for display",
        },
        AttributeSpec {
            name: "value",
            detail: "Selected index/value",
        },
        AttributeSpec {
            name: "start",
            detail: "Start date/time",
        },
        AttributeSpec {
            name: "end",
            detail: "End date/time",
        },
        AttributeSpec {
            name: "bindchange",
            detail: "Selection change",
        },
        AttributeSpec {
            name: "bindcancel",
            detail: "Selection cancelled",
        },
    ],
    values: &[(
        "mode",
        &[
            ValueSpec {
                value: "selector",
                detail: "Single column selector",
            },
            ValueSpec {
                value: "multiSelector",
                detail: "Multi column selector",
            },
            ValueSpec {
                value: "time",
                detail: "Time picker",
            },
            ValueSpec {
                value: "date",
                detail: "Date picker",
            },
            ValueSpec {
                value: "region",
                detail: "Region picker",
            },
        ],
    )],
};

const TAG_SCROLL_VIEW: TagSpec = TagSpec {
    name: "scroll-view",
    detail: "Scrollable container",
    attributes: &[
        AttributeSpec {
            name: "scroll-x",
            detail: "Horizontal scrolling",
        },
        AttributeSpec {
            name: "scroll-y",
            detail: "Vertical scrolling",
        },
        AttributeSpec {
            name: "enable-flex",
            detail: "Enable flex layout",
        },
        AttributeSpec {
            name: "scroll-top",
            detail: "Vertical scroll offset",
        },
        AttributeSpec {
            name: "scroll-left",
            detail: "Horizontal scroll offset",
        },
        AttributeSpec {
            name: "enable-back-to-top",
            detail: "Back-to-top gesture",
        },
        AttributeSpec {
            name: "bindscroll",
            detail: "Scroll event",
        },
        AttributeSpec {
            name: "bindscrolltolower",
            detail: "Reached the bottom/end",
        },
    ],
    values: &[(
        "scroll-y",
        &[
            ValueSpec {
                value: "true",
                detail: "Enable",
            },
            ValueSpec {
                value: "false",
                detail: "Disable",
            },
        ],
    )],
};

const TAG_SWIPER: TagSpec = TagSpec {
    name: "swiper",
    detail: "Carousel container",
    attributes: &[
        AttributeSpec {
            name: "indicator-dots",
            detail: "Show indicator dots",
        },
        AttributeSpec {
            name: "autoplay",
            detail: "Auto play",
        },
        AttributeSpec {
            name: "interval",
            detail: "Autoplay interval in ms",
        },
        AttributeSpec {
            name: "duration",
            detail: "Transition duration in ms",
        },
        AttributeSpec {
            name: "circular",
            detail: "Circular sliding",
        },
        AttributeSpec {
            name: "vertical",
            detail: "Vertical direction",
        },
        AttributeSpec {
            name: "bindchange",
            detail: "Current item changed",
        },
    ],
    values: &[
        (
            "indicator-dots",
            &[
                ValueSpec {
                    value: "true",
                    detail: "Enable",
                },
                ValueSpec {
                    value: "false",
                    detail: "Disable",
                },
            ],
        ),
        (
            "autoplay",
            &[
                ValueSpec {
                    value: "true",
                    detail: "Enable",
                },
                ValueSpec {
                    value: "false",
                    detail: "Disable",
                },
            ],
        ),
        (
            "circular",
            &[
                ValueSpec {
                    value: "true",
                    detail: "Enable",
                },
                ValueSpec {
                    value: "false",
                    detail: "Disable",
                },
            ],
        ),
    ],
};

const TAG_NAVIGATOR: TagSpec = TagSpec {
    name: "navigator",
    detail: "Navigation link",
    attributes: &[
        AttributeSpec {
            name: "url",
            detail: "Target URL",
        },
        AttributeSpec {
            name: "open-type",
            detail: "Navigation mode",
        },
        AttributeSpec {
            name: "delta",
            detail: "Back step count",
        },
        AttributeSpec {
            name: "hover-class",
            detail: "Pressed style",
        },
    ],
    values: &[(
        "open-type",
        &[
            ValueSpec {
                value: "navigate",
                detail: "Push a new page",
            },
            ValueSpec {
                value: "redirect",
                detail: "Replace the current page",
            },
            ValueSpec {
                value: "switchTab",
                detail: "Switch to a tab page",
            },
            ValueSpec {
                value: "reLaunch",
                detail: "Close all pages and open",
            },
            ValueSpec {
                value: "navigateBack",
                detail: "Go back",
            },
            ValueSpec {
                value: "exit",
                detail: "Exit the Mini Program",
            },
        ],
    )],
};

const TAG_TEXTAREA: TagSpec = TagSpec {
    name: "textarea",
    detail: "Multi-line input",
    attributes: &[
        AttributeSpec {
            name: "value",
            detail: "Current value",
        },
        AttributeSpec {
            name: "placeholder",
            detail: "Placeholder text",
        },
        AttributeSpec {
            name: "maxlength",
            detail: "Maximum length",
        },
        AttributeSpec {
            name: "auto-height",
            detail: "Grow with content",
        },
        AttributeSpec {
            name: "disabled",
            detail: "Disable the input",
        },
    ],
    values: &[(
        "auto-height",
        &[
            ValueSpec {
                value: "true",
                detail: "Enable",
            },
            ValueSpec {
                value: "false",
                detail: "Disable",
            },
        ],
    )],
};

const TAG_FORM: TagSpec = TagSpec {
    name: "form",
    detail: "Form container",
    attributes: &[
        AttributeSpec {
            name: "bindsubmit",
            detail: "Submit event",
        },
        AttributeSpec {
            name: "bindreset",
            detail: "Reset event",
        },
    ],
    values: &[],
};

const TAG_LABEL: TagSpec = TagSpec {
    name: "label",
    detail: "Form label",
    attributes: &[AttributeSpec {
        name: "for",
        detail: "Associated control id",
    }],
    values: &[],
};

const TAG_CHECKBOX: TagSpec = TagSpec {
    name: "checkbox",
    detail: "Checkbox",
    attributes: &[
        AttributeSpec {
            name: "value",
            detail: "Checkbox value",
        },
        AttributeSpec {
            name: "checked",
            detail: "Checked state",
        },
        AttributeSpec {
            name: "disabled",
            detail: "Disable the checkbox",
        },
    ],
    values: &[(
        "checked",
        &[
            ValueSpec {
                value: "true",
                detail: "Checked",
            },
            ValueSpec {
                value: "false",
                detail: "Unchecked",
            },
        ],
    )],
};

const TAG_RADIO: TagSpec = TagSpec {
    name: "radio",
    detail: "Radio button",
    attributes: &[
        AttributeSpec {
            name: "value",
            detail: "Radio value",
        },
        AttributeSpec {
            name: "checked",
            detail: "Checked state",
        },
        AttributeSpec {
            name: "disabled",
            detail: "Disable the radio",
        },
    ],
    values: &[(
        "checked",
        &[
            ValueSpec {
                value: "true",
                detail: "Checked",
            },
            ValueSpec {
                value: "false",
                detail: "Unchecked",
            },
        ],
    )],
};

const TAG_TEMPLATE: TagSpec = TagSpec {
    name: "template",
    detail: "Template reference",
    attributes: &[
        AttributeSpec {
            name: "is",
            detail: "Template name",
        },
        AttributeSpec {
            name: "data",
            detail: "Template data",
        },
    ],
    values: &[],
};

const TAG_BLOCK: TagSpec = TagSpec {
    name: "block",
    detail: "Non-rendering wrapper",
    attributes: &[],
    values: &[],
};

const TAG_SWIPER_ITEM: TagSpec = TagSpec {
    name: "swiper-item",
    detail: "Carousel item",
    attributes: &[],
    values: &[],
};

const TAG_WXS: TagSpec = TagSpec {
    name: "wxs",
    detail: "Inline WXS module",
    attributes: &[
        AttributeSpec {
            name: "module",
            detail: "Module name",
        },
        AttributeSpec {
            name: "src",
            detail: "External WXS file",
        },
    ],
    values: &[],
};

const TAG_VIEWS: &[TagSpec] = &[
    TagSpec {
        name: "movable-area",
        detail: "Movable area",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "movable-view",
        detail: "Movable element",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "cover-view",
        detail: "Native overlay container",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "cover-image",
        detail: "Native overlay image",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "icon",
        detail: "Builtin icon",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "progress",
        detail: "Progress bar",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "rich-text",
        detail: "Rich text",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "slider",
        detail: "Slider",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "switch",
        detail: "Switch",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "checkbox-group",
        detail: "Checkbox group",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "radio-group",
        detail: "Radio group",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "picker-view",
        detail: "Custom picker container",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "picker-view-column",
        detail: "Custom picker column",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "map",
        detail: "Map",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "canvas",
        detail: "Canvas",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "video",
        detail: "Video",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "audio",
        detail: "Audio",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "camera",
        detail: "Camera",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "live-player",
        detail: "Live audio/video player",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "live-pusher",
        detail: "Live audio/video pusher",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "web-view",
        detail: "Embedded web page",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "open-data",
        detail: "Open data view",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "ad",
        detail: "Advertisement",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "official-account",
        detail: "Official account",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "page-meta",
        detail: "Page metadata",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "navigation-bar",
        detail: "Custom navigation bar",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "root-portal",
        detail: "Render into the page root",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "match-media",
        detail: "Media query container",
        attributes: &[],
        values: &[],
    },
];

/// Builtin tags in suggestion order.
pub(crate) const TAGS: &[&TagSpec] = &[
    &TAG_VIEW,
    &TAG_TEXT,
    &TAG_BUTTON,
    &TAG_IMAGE,
    &TAG_INPUT,
    &TAG_PICKER,
    &TAG_SCROLL_VIEW,
    &TAG_SWIPER,
    &TAG_SWIPER_ITEM,
    &TAG_NAVIGATOR,
    &TAG_TEXTAREA,
    &TAG_FORM,
    &TAG_LABEL,
    &TAG_CHECKBOX,
    &TAG_RADIO,
    &TAG_TEMPLATE,
    &TAG_BLOCK,
    &TAG_WXS,
];

/// Finds a tag by exact name.
pub(crate) fn tag(name: &str) -> Option<&'static TagSpec> {
    TAGS.iter()
        .copied()
        .find(|spec| spec.name == name)
        .or_else(|| TAG_VIEWS.iter().find(|spec| spec.name == name))
}

/// All builtin tag specs, including the ones without detailed attributes.
pub(crate) fn all_tags() -> impl Iterator<Item = &'static TagSpec> {
    TAGS.iter().copied().chain(TAG_VIEWS.iter())
}

/// Static values for `tag.attribute`.
pub(crate) fn values_for(tag_name: &str, attribute: &str) -> &'static [ValueSpec] {
    tag(tag_name)
        .and_then(|spec| {
            spec.values
                .iter()
                .find(|(name, _)| *name == attribute)
                .map(|(_, values)| *values)
        })
        .unwrap_or(&[])
}

/// True when the attribute name binds an event handler.
pub(crate) fn is_event_attribute(name: &str) -> bool {
    const PREFIXES: [&str; 5] = ["capture-bind", "capture-catch", "mut-bind", "bind", "catch"];
    for prefix in PREFIXES {
        if let Some(rest) = name.strip_prefix(prefix) {
            // `bindtap` / `bind:tap` / `bind` are events; `binding-mode` and
            // `bindex` are ordinary attributes.
            if rest.is_empty() || rest.starts_with(':') || EVENT_NAMES.contains(&rest) {
                return true;
            }
        }
    }
    false
}

/// True for an attribute-name prefix that is heading into an event binding
/// (`bind`, `bind:`, `bindta`, `capture-bind:`, ...). Unlike
/// [`is_event_attribute`] this accepts incomplete suffixes, so generated
/// `bind*`/`catch*` names appear while the user is still typing.
pub(crate) fn is_event_prefix(name: &str) -> bool {
    const PREFIXES: [&str; 5] = ["capture-bind", "capture-catch", "mut-bind", "bind", "catch"];
    PREFIXES.iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|rest| {
            rest.is_empty()
                || rest.starts_with(':')
                || rest.bytes().all(|byte| byte.is_ascii_lowercase())
        })
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_the_required_enums() {
        let button = tag("button").expect("button");
        assert!(
            button
                .values
                .iter()
                .any(|(name, values)| *name == "type" && values.len() == 3)
        );
        let picker = tag("picker").expect("picker");
        assert!(picker.values.iter().any(|(name, _)| *name == "mode"));
        let input = tag("input").expect("input");
        assert!(input.values.iter().any(|(name, _)| *name == "type"));
        assert!(!values_for("picker", "mode").is_empty());
        assert!(values_for("picker", "type").is_empty());
        assert!(values_for("view", "class").is_empty());
    }

    #[test]
    fn partial_event_prefixes_are_recognised() {
        for prefix in [
            "bind",
            "bind:",
            "bindl",
            "catch",
            "capture-bind:",
            "mut-bind:",
        ] {
            assert!(is_event_prefix(prefix), "{prefix}");
        }
        for ordinary in ["", "bi", "binding-mode", "class", "catch-up"] {
            assert!(!is_event_prefix(ordinary), "{ordinary}");
        }
    }

    #[test]
    fn event_and_directive_prefixes_are_recognised() {
        for attribute in [
            "bindtap",
            "catchtap",
            "bind:tap",
            "capture-bind:tap",
            "mut-bind:tap",
            // Tag-declared event attributes must also route to the backend.
            "bindcancel",
            "bindcolumnchange",
            "bindtimeupdate",
            "bindregionchange",
            "bindanimationfinish",
            "bindlinechange",
        ] {
            assert!(is_event_attribute(attribute), "{attribute}");
        }
        for ordinary in ["class", "binding-mode", "bindex"] {
            assert!(!is_event_attribute(ordinary), "{ordinary}");
        }
        assert!(
            DIRECTIVE_ATTRIBUTES
                .iter()
                .any(|attribute| attribute.name == "wx:for")
        );
    }
}
