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
    detail: "元素 id",
};
const ATTR_CLASS: AttributeSpec = AttributeSpec {
    name: "class",
    detail: "样式类列表",
};
const ATTR_STYLE: AttributeSpec = AttributeSpec {
    name: "style",
    detail: "内联样式",
};
const ATTR_HIDDEN: AttributeSpec = AttributeSpec {
    name: "hidden",
    detail: "隐藏元素",
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
        detail: "绑定 tap 事件（冒泡）",
    },
    AttributeSpec {
        name: "catchtap",
        detail: "绑定 tap 事件并阻止冒泡",
    },
    AttributeSpec {
        name: "bind:tap",
        detail: "绑定 tap 事件（冒泡，另一种写法）",
    },
    AttributeSpec {
        name: "catch:tap",
        detail: "绑定 tap 事件并阻止冒泡（另一种写法）",
    },
    AttributeSpec {
        name: "capture-bind:tap",
        detail: "捕获阶段绑定 tap 事件",
    },
    AttributeSpec {
        name: "capture-catch:tap",
        detail: "捕获阶段绑定 tap 事件并阻止传播",
    },
    AttributeSpec {
        name: "mut-bind:tap",
        detail: "互斥的 tap 事件绑定",
    },
    AttributeSpec {
        name: "bindinput",
        detail: "绑定输入事件",
    },
    AttributeSpec {
        name: "bindchange",
        detail: "绑定值变化事件",
    },
    AttributeSpec {
        name: "bindconfirm",
        detail: "绑定确认事件",
    },
    AttributeSpec {
        name: "bindsubmit",
        detail: "绑定表单提交事件",
    },
    AttributeSpec {
        name: "bindreset",
        detail: "绑定表单重置事件",
    },
    AttributeSpec {
        name: "bindscroll",
        detail: "绑定滚动事件",
    },
    AttributeSpec {
        name: "bindload",
        detail: "绑定加载完成事件",
    },
    AttributeSpec {
        name: "binderror",
        detail: "绑定错误事件",
    },
    AttributeSpec {
        name: "bindtouchstart",
        detail: "绑定触摸开始事件",
    },
    AttributeSpec {
        name: "bindtouchmove",
        detail: "绑定触摸移动事件",
    },
    AttributeSpec {
        name: "bindtouchend",
        detail: "绑定触摸结束事件",
    },
];

/// `wx:` directives.
pub(crate) const DIRECTIVE_ATTRIBUTES: &[AttributeSpec] = &[
    AttributeSpec {
        name: "wx:if",
        detail: "条件渲染",
    },
    AttributeSpec {
        name: "wx:elif",
        detail: "否则如果分支",
    },
    AttributeSpec {
        name: "wx:else",
        detail: "否则分支",
    },
    AttributeSpec {
        name: "wx:for",
        detail: "列表渲染数据源",
    },
    AttributeSpec {
        name: "wx:for-item",
        detail: "列表项变量名",
    },
    AttributeSpec {
        name: "wx:for-index",
        detail: "列表索引变量名",
    },
    AttributeSpec {
        name: "wx:key",
        detail: "列表 key 表达式",
    },
];

const TAG_VIEW: TagSpec = TagSpec {
    name: "view",
    detail: "视图容器，可包裹其他组件",
    attributes: &[
        AttributeSpec {
            name: "hover-class",
            detail: "按下时应用的样式类",
        },
        AttributeSpec {
            name: "hover-stop-propagation",
            detail: "阻止按住样式传播",
        },
        AttributeSpec {
            name: "hover-start-time",
            detail: "按住延迟（毫秒）",
        },
        AttributeSpec {
            name: "hover-stay-time",
            detail: "按住持续时间（毫秒）",
        },
    ],
    values: &[(
        "hover-stop-propagation",
        &[
            ValueSpec {
                value: "true",
                detail: "启用",
            },
            ValueSpec {
                value: "false",
                detail: "禁用",
            },
        ],
    )],
};

const TAG_TEXT: TagSpec = TagSpec {
    name: "text",
    detail: "内联文本",
    attributes: &[
        AttributeSpec {
            name: "selectable",
            detail: "允许文本选中",
        },
        AttributeSpec {
            name: "user-select",
            detail: "允许文本选中（新版）",
        },
        AttributeSpec {
            name: "space",
            detail: "空白符处理",
        },
        AttributeSpec {
            name: "decode",
            detail: "解码 HTML 实体",
        },
    ],
    values: &[(
        "space",
        &[
            ValueSpec {
                value: "ensp",
                detail: "半角空格",
            },
            ValueSpec {
                value: "emsp",
                detail: "全角空格",
            },
            ValueSpec {
                value: "nbsp",
                detail: "不换行空格",
            },
        ],
    )],
};

const BUTTON_TYPES: &[ValueSpec] = &[
    ValueSpec {
        value: "default",
        detail: "默认按钮",
    },
    ValueSpec {
        value: "primary",
        detail: "主要按钮",
    },
    ValueSpec {
        value: "warn",
        detail: "警告按钮",
    },
];

const BUTTON_SIZES: &[ValueSpec] = &[
    ValueSpec {
        value: "default",
        detail: "默认尺寸",
    },
    ValueSpec {
        value: "mini",
        detail: "小尺寸",
    },
];

const TAG_BUTTON: TagSpec = TagSpec {
    name: "button",
    detail: "按钮",
    attributes: &[
        AttributeSpec {
            name: "type",
            detail: "样式类型",
        },
        AttributeSpec {
            name: "size",
            detail: "按钮尺寸",
        },
        AttributeSpec {
            name: "plain",
            detail: "镂空样式",
        },
        AttributeSpec {
            name: "disabled",
            detail: "禁用按钮",
        },
        AttributeSpec {
            name: "loading",
            detail: "显示加载状态",
        },
        AttributeSpec {
            name: "form-type",
            detail: "表单提交/重置角色",
        },
        AttributeSpec {
            name: "open-type",
            detail: "小程序开放能力",
        },
        AttributeSpec {
            name: "hover-class",
            detail: "按下时应用的样式类",
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
                    detail: "提交表单",
                },
                ValueSpec {
                    value: "reset",
                    detail: "重置表单",
                },
            ],
        ),
        (
            "open-type",
            &[
                ValueSpec {
                    value: "contact",
                    detail: "打开客服会话",
                },
                ValueSpec {
                    value: "share",
                    detail: "分享页面",
                },
                ValueSpec {
                    value: "getPhoneNumber",
                    detail: "获取手机号",
                },
                ValueSpec {
                    value: "getUserInfo",
                    detail: "获取用户信息",
                },
                ValueSpec {
                    value: "openSetting",
                    detail: "打开设置",
                },
                ValueSpec {
                    value: "feedback",
                    detail: "打开意见反馈",
                },
            ],
        ),
    ],
};

const TAG_IMAGE: TagSpec = TagSpec {
    name: "image",
    detail: "图片",
    attributes: &[
        AttributeSpec {
            name: "src",
            detail: "图片地址",
        },
        AttributeSpec {
            name: "mode",
            detail: "裁剪/缩放模式",
        },
        AttributeSpec {
            name: "lazy-load",
            detail: "懒加载",
        },
        AttributeSpec {
            name: "binderror",
            detail: "加载出错",
        },
        AttributeSpec {
            name: "bindload",
            detail: "加载完成",
        },
    ],
    values: &[(
        "mode",
        &[
            ValueSpec {
                value: "scaleToFill",
                detail: "拉伸填满",
            },
            ValueSpec {
                value: "aspectFit",
                detail: "完整显示并保持比例",
            },
            ValueSpec {
                value: "aspectFill",
                detail: "填充并保持比例",
            },
            ValueSpec {
                value: "widthFix",
                detail: "固定宽度，高度自适应",
            },
            ValueSpec {
                value: "heightFix",
                detail: "固定高度，宽度自适应",
            },
            ValueSpec {
                value: "center",
                detail: "居中且不缩放",
            },
            ValueSpec {
                value: "top",
                detail: "顶部对齐",
            },
            ValueSpec {
                value: "bottom",
                detail: "底部对齐",
            },
            ValueSpec {
                value: "left",
                detail: "左对齐",
            },
            ValueSpec {
                value: "right",
                detail: "右对齐",
            },
        ],
    )],
};

const INPUT_TYPES: &[ValueSpec] = &[
    ValueSpec {
        value: "text",
        detail: "文本输入",
    },
    ValueSpec {
        value: "number",
        detail: "数字输入",
    },
    ValueSpec {
        value: "idcard",
        detail: "身份证输入",
    },
    ValueSpec {
        value: "digit",
        detail: "带小数点的数字输入",
    },
    ValueSpec {
        value: "safe-password",
        detail: "密码输入",
    },
    ValueSpec {
        value: "nickname",
        detail: "昵称输入",
    },
];

const TAG_INPUT: TagSpec = TagSpec {
    name: "input",
    detail: "单行输入框",
    attributes: &[
        AttributeSpec {
            name: "type",
            detail: "输入类型",
        },
        AttributeSpec {
            name: "value",
            detail: "当前值",
        },
        AttributeSpec {
            name: "placeholder",
            detail: "占位提示文字",
        },
        AttributeSpec {
            name: "disabled",
            detail: "禁用输入框",
        },
        AttributeSpec {
            name: "maxlength",
            detail: "最大长度",
        },
        AttributeSpec {
            name: "focus",
            detail: "聚焦输入框",
        },
        AttributeSpec {
            name: "confirm-type",
            detail: "键盘确认按钮文案",
        },
        AttributeSpec {
            name: "password",
            detail: "密码输入框",
        },
    ],
    values: &[
        ("type", INPUT_TYPES),
        (
            "confirm-type",
            &[
                ValueSpec {
                    value: "send",
                    detail: "发送",
                },
                ValueSpec {
                    value: "search",
                    detail: "搜索",
                },
                ValueSpec {
                    value: "next",
                    detail: "下一步",
                },
                ValueSpec {
                    value: "go",
                    detail: "前往",
                },
                ValueSpec {
                    value: "done",
                    detail: "完成",
                },
            ],
        ),
    ],
};

const TAG_PICKER: TagSpec = TagSpec {
    name: "picker",
    detail: "选择器",
    attributes: &[
        AttributeSpec {
            name: "mode",
            detail: "选择器类型",
        },
        AttributeSpec {
            name: "range",
            detail: "可选数据",
        },
        AttributeSpec {
            name: "range-key",
            detail: "用于显示的字段名",
        },
        AttributeSpec {
            name: "value",
            detail: "选中项索引/值",
        },
        AttributeSpec {
            name: "start",
            detail: "开始日期/时间",
        },
        AttributeSpec {
            name: "end",
            detail: "结束日期/时间",
        },
        AttributeSpec {
            name: "fields",
            detail: "日期粒度",
        },
        AttributeSpec {
            name: "custom-item",
            detail: "自定义地区项",
        },
        AttributeSpec {
            name: "level",
            detail: "地区选择层级",
        },
        AttributeSpec {
            name: "bindchange",
            detail: "选择变化",
        },
        AttributeSpec {
            name: "bindcolumnchange",
            detail: "多列变化",
        },
        AttributeSpec {
            name: "bindcancel",
            detail: "取消选择",
        },
    ],
    values: &[
        (
            "mode",
            &[
                ValueSpec {
                    value: "selector",
                    detail: "单列选择器",
                },
                ValueSpec {
                    value: "multiSelector",
                    detail: "多列选择器",
                },
                ValueSpec {
                    value: "time",
                    detail: "时间选择器",
                },
                ValueSpec {
                    value: "date",
                    detail: "日期选择器",
                },
                ValueSpec {
                    value: "region",
                    detail: "省市区选择器",
                },
            ],
        ),
        (
            "fields",
            &[
                ValueSpec {
                    value: "year",
                    detail: "选择年",
                },
                ValueSpec {
                    value: "month",
                    detail: "选择月",
                },
                ValueSpec {
                    value: "day",
                    detail: "选择日",
                },
            ],
        ),
        (
            "level",
            &[
                ValueSpec {
                    value: "province",
                    detail: "省级",
                },
                ValueSpec {
                    value: "city",
                    detail: "市级",
                },
                ValueSpec {
                    value: "district",
                    detail: "区县级",
                },
            ],
        ),
    ],
};

const TAG_SCROLL_VIEW: TagSpec = TagSpec {
    name: "scroll-view",
    detail: "可滚动容器",
    attributes: &[
        AttributeSpec {
            name: "scroll-x",
            detail: "水平滚动",
        },
        AttributeSpec {
            name: "scroll-y",
            detail: "垂直滚动",
        },
        AttributeSpec {
            name: "enable-flex",
            detail: "启用 flex 布局",
        },
        AttributeSpec {
            name: "scroll-top",
            detail: "垂直滚动位置",
        },
        AttributeSpec {
            name: "scroll-left",
            detail: "水平滚动位置",
        },
        AttributeSpec {
            name: "enable-back-to-top",
            detail: "双击返回顶部",
        },
        AttributeSpec {
            name: "bindscroll",
            detail: "绑定滚动事件",
        },
        AttributeSpec {
            name: "bindscrolltolower",
            detail: "滚动到底部",
        },
    ],
    values: &[(
        "scroll-y",
        &[
            ValueSpec {
                value: "true",
                detail: "启用",
            },
            ValueSpec {
                value: "false",
                detail: "禁用",
            },
        ],
    )],
};

const TAG_SWIPER: TagSpec = TagSpec {
    name: "swiper",
    detail: "轮播容器",
    attributes: &[
        AttributeSpec {
            name: "indicator-dots",
            detail: "显示指示点",
        },
        AttributeSpec {
            name: "autoplay",
            detail: "自动播放",
        },
        AttributeSpec {
            name: "interval",
            detail: "自动播放间隔（毫秒）",
        },
        AttributeSpec {
            name: "duration",
            detail: "过渡时长（毫秒）",
        },
        AttributeSpec {
            name: "circular",
            detail: "循环轮播",
        },
        AttributeSpec {
            name: "vertical",
            detail: "垂直方向",
        },
        AttributeSpec {
            name: "bindchange",
            detail: "当前项变化",
        },
    ],
    values: &[
        (
            "indicator-dots",
            &[
                ValueSpec {
                    value: "true",
                    detail: "启用",
                },
                ValueSpec {
                    value: "false",
                    detail: "禁用",
                },
            ],
        ),
        (
            "autoplay",
            &[
                ValueSpec {
                    value: "true",
                    detail: "启用",
                },
                ValueSpec {
                    value: "false",
                    detail: "禁用",
                },
            ],
        ),
        (
            "circular",
            &[
                ValueSpec {
                    value: "true",
                    detail: "启用",
                },
                ValueSpec {
                    value: "false",
                    detail: "禁用",
                },
            ],
        ),
    ],
};

const TAG_NAVIGATOR: TagSpec = TagSpec {
    name: "navigator",
    detail: "导航链接",
    attributes: &[
        AttributeSpec {
            name: "url",
            detail: "目标地址",
        },
        AttributeSpec {
            name: "open-type",
            detail: "打开方式",
        },
        AttributeSpec {
            name: "delta",
            detail: "返回层数",
        },
        AttributeSpec {
            name: "hover-class",
            detail: "按下样式",
        },
    ],
    values: &[(
        "open-type",
        &[
            ValueSpec {
                value: "navigate",
                detail: "打开新页面",
            },
            ValueSpec {
                value: "redirect",
                detail: "替换当前页面",
            },
            ValueSpec {
                value: "switchTab",
                detail: "切换到 tab 页",
            },
            ValueSpec {
                value: "reLaunch",
                detail: "关闭所有页面后打开",
            },
            ValueSpec {
                value: "navigateBack",
                detail: "返回",
            },
            ValueSpec {
                value: "exit",
                detail: "退出小程序",
            },
        ],
    )],
};

const TAG_TEXTAREA: TagSpec = TagSpec {
    name: "textarea",
    detail: "多行输入框",
    attributes: &[
        AttributeSpec {
            name: "value",
            detail: "当前值",
        },
        AttributeSpec {
            name: "placeholder",
            detail: "占位提示文字",
        },
        AttributeSpec {
            name: "maxlength",
            detail: "最大长度",
        },
        AttributeSpec {
            name: "auto-height",
            detail: "随内容自动增高",
        },
        AttributeSpec {
            name: "disabled",
            detail: "禁用输入框",
        },
    ],
    values: &[(
        "auto-height",
        &[
            ValueSpec {
                value: "true",
                detail: "启用",
            },
            ValueSpec {
                value: "false",
                detail: "禁用",
            },
        ],
    )],
};

const TAG_FORM: TagSpec = TagSpec {
    name: "form",
    detail: "表单容器",
    attributes: &[
        AttributeSpec {
            name: "bindsubmit",
            detail: "绑定提交事件",
        },
        AttributeSpec {
            name: "bindreset",
            detail: "绑定重置事件",
        },
    ],
    values: &[],
};

const TAG_LABEL: TagSpec = TagSpec {
    name: "label",
    detail: "表单标签",
    attributes: &[AttributeSpec {
        name: "for",
        detail: "关联控件的 id",
    }],
    values: &[],
};

const TAG_CHECKBOX: TagSpec = TagSpec {
    name: "checkbox",
    detail: "复选框",
    attributes: &[
        AttributeSpec {
            name: "value",
            detail: "复选框的值",
        },
        AttributeSpec {
            name: "checked",
            detail: "选中状态",
        },
        AttributeSpec {
            name: "disabled",
            detail: "禁用复选框",
        },
    ],
    values: &[(
        "checked",
        &[
            ValueSpec {
                value: "true",
                detail: "选中",
            },
            ValueSpec {
                value: "false",
                detail: "未选中",
            },
        ],
    )],
};

const TAG_RADIO: TagSpec = TagSpec {
    name: "radio",
    detail: "单选框",
    attributes: &[
        AttributeSpec {
            name: "value",
            detail: "单选框的值",
        },
        AttributeSpec {
            name: "checked",
            detail: "选中状态",
        },
        AttributeSpec {
            name: "disabled",
            detail: "禁用单选框",
        },
    ],
    values: &[(
        "checked",
        &[
            ValueSpec {
                value: "true",
                detail: "选中",
            },
            ValueSpec {
                value: "false",
                detail: "未选中",
            },
        ],
    )],
};

const TAG_TEMPLATE: TagSpec = TagSpec {
    name: "template",
    detail: "模板引用",
    attributes: &[
        AttributeSpec {
            name: "is",
            detail: "模板名称",
        },
        AttributeSpec {
            name: "data",
            detail: "模板数据",
        },
    ],
    values: &[],
};

const TAG_BLOCK: TagSpec = TagSpec {
    name: "block",
    detail: "不渲染的包裹元素",
    attributes: &[],
    values: &[],
};

const TAG_SWIPER_ITEM: TagSpec = TagSpec {
    name: "swiper-item",
    detail: "轮播项",
    attributes: &[],
    values: &[],
};

const TAG_WXS: TagSpec = TagSpec {
    name: "wxs",
    detail: "内联 WXS 模块",
    attributes: &[
        AttributeSpec {
            name: "module",
            detail: "模块名",
        },
        AttributeSpec {
            name: "src",
            detail: "外部 WXS 文件",
        },
    ],
    values: &[],
};

const TAG_VIEWS: &[TagSpec] = &[
    TagSpec {
        name: "movable-area",
        detail: "可移动区域",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "movable-view",
        detail: "可移动元素",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "cover-view",
        detail: "原生覆盖层容器",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "cover-image",
        detail: "原生覆盖层图片",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "icon",
        detail: "内置图标",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "progress",
        detail: "进度条",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "rich-text",
        detail: "富文本",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "slider",
        detail: "滑动条",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "switch",
        detail: "开关",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "checkbox-group",
        detail: "复选框组",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "radio-group",
        detail: "单选框组",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "picker-view",
        detail: "自定义选择器容器",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "picker-view-column",
        detail: "自定义选择器列",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "map",
        detail: "地图",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "canvas",
        detail: "画布",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "video",
        detail: "视频",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "audio",
        detail: "音频",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "camera",
        detail: "相机",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "live-player",
        detail: "音视频直播播放器",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "live-pusher",
        detail: "音视频直播推流器",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "web-view",
        detail: "嵌入网页",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "open-data",
        detail: "开放数据视图",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "ad",
        detail: "广告",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "official-account",
        detail: "公众号",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "page-meta",
        detail: "页面元信息",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "navigation-bar",
        detail: "自定义导航栏",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "root-portal",
        detail: "渲染到页面根节点",
        attributes: &[],
        values: &[],
    },
    TagSpec {
        name: "match-media",
        detail: "媒体查询容器",
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

/// Returns the official `WeChat` documentation URL for one built-in tag.
pub(crate) fn docs_url(tag: &str) -> String {
    match tag {
        "block" => {
            "https://developers.weixin.qq.com/miniprogram/dev/reference/wxml/block.html".to_owned()
        }
        "template" => {
            "https://developers.weixin.qq.com/miniprogram/dev/reference/wxml/template.html"
                .to_owned()
        }
        "wxs" => "https://developers.weixin.qq.com/miniprogram/dev/reference/wxs/".to_owned(),
        other => format!("https://developers.weixin.qq.com/miniprogram/dev/component/{other}.html"),
    }
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

/// Finds a `wx:` directive by name.
pub(crate) fn directive(name: &str) -> Option<&'static AttributeSpec> {
    DIRECTIVE_ATTRIBUTES
        .iter()
        .find(|attribute| attribute.name == name)
}

/// Finds a tag-specific attribute by exact name.
pub(crate) fn tag_attribute(tag_name: &str, attribute: &str) -> Option<&'static AttributeSpec> {
    tag(tag_name)?
        .attributes
        .iter()
        .find(|candidate| candidate.name == attribute)
}

/// Finds a global attribute by exact name.
pub(crate) fn global_attribute(attribute: &str) -> Option<&'static AttributeSpec> {
    GLOBAL_ATTRIBUTES
        .iter()
        .find(|candidate| candidate.name == attribute)
}

/// Finds a documented event-binding attribute by exact name.
pub(crate) fn event_attribute(attribute: &str) -> Option<&'static AttributeSpec> {
    EVENT_ATTRIBUTES
        .iter()
        .find(|candidate| candidate.name == attribute)
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

/// Completed event-binding attribute names for handler-value routing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EventAttributeClass {
    /// A completed `bind`/`catch`/`capture-bind`/`capture-catch`/`mut-bind`
    /// binding that owns an event handler value.
    Handler,
    /// A binding-shaped name that cannot own an event handler value.
    Rejected(EventAttributeRejection),
    /// An attribute unrelated to event bindings.
    Other,
}

/// Why a binding-shaped attribute name is not an event-handler value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EventAttributeRejection {
    /// Bare prefix such as `bind` or `catch`.
    MissingEventName,
    /// A colon form with no event name (`bind:`).
    EmptyEventName,
    /// A colon form with an illegal event segment (`bind::tap`).
    InvalidEventName,
    /// A direct form whose suffix is not all ASCII lowercase
    /// (`bind-get-phonenumber`, `bindTap`).
    InvalidDirectSuffix,
    /// `bindex` is an ordinary attribute that collides with `bind` + `ex`.
    ConflictingAttribute,
}

/// Returns whether a completed attribute name owns an event-handler value.
///
/// This is the value-routing classifier. [`is_event_prefix`] remains the
/// attribute-name completion classifier and deliberately accepts incomplete
/// suffixes such as `bind` or `bindt`; it must not decide value routing.
pub(crate) fn is_event_value_attribute(name: &str) -> bool {
    matches!(classify_event_attribute(name), EventAttributeClass::Handler)
}

/// Classifies one complete attribute name against the value-routing rules.
///
/// Colon forms accept any non-empty event name made of ASCII letters, digits,
/// `_` and `-`. Direct forms accept the compact `capture-bind`, `capture-catch`,
/// `mut-bind`, `bind` and `catch` prefixes followed by a non-empty event name.
/// `EVENT_NAMES` is not consulted: custom events such as `bindgetphonenumber`
/// and `bindSomeEvent` stay handlers.
pub(crate) fn classify_event_attribute(name: &str) -> EventAttributeClass {
    const COLON_PREFIXES: [&str; 5] =
        ["capture-bind", "capture-catch", "mut-bind", "bind", "catch"];
    const DIRECT_PREFIXES: [&str; 5] =
        ["capture-bind", "capture-catch", "mut-bind", "bind", "catch"];
    if name == "bindex" {
        return EventAttributeClass::Rejected(EventAttributeRejection::ConflictingAttribute);
    }
    for prefix in COLON_PREFIXES {
        let Some(rest) = name.strip_prefix(prefix) else {
            continue;
        };
        if rest.is_empty() {
            return EventAttributeClass::Rejected(EventAttributeRejection::MissingEventName);
        }
        if let Some(event) = rest.strip_prefix(':') {
            if event.is_empty() {
                return EventAttributeClass::Rejected(EventAttributeRejection::EmptyEventName);
            }
            return if event.bytes().all(is_event_name_byte) {
                EventAttributeClass::Handler
            } else {
                EventAttributeClass::Rejected(EventAttributeRejection::InvalidEventName)
            };
        }
        // `capture-bind`/`capture-catch`/`mut-bind` accept both colon and compact
        // forms; every direct family takes a non-empty event-name suffix.
        return if DIRECT_PREFIXES.contains(&prefix) && rest.bytes().all(is_event_name_byte) {
            EventAttributeClass::Handler
        } else {
            EventAttributeClass::Rejected(EventAttributeRejection::InvalidDirectSuffix)
        };
    }
    EventAttributeClass::Other
}

/// Event-name bytes accepted after a colon-form prefix.
const fn is_event_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
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

    #[test]
    fn value_routing_accepts_direct_and_colon_event_bindings() {
        for attribute in [
            "bindtap",
            "catchtap",
            "bindgetphonenumber",
            "bindchooseavatar",
            "bindplay",
            "bindpause",
            "bindended",
            "bindwaiting",
            "bindprogress",
            "bindfullscreenchange",
            "bindmarkertap",
            "bindcallouttap",
            "bindcontroltap",
            "bindpoitap",
            "bindanchorpointtap",
            "binddragstart",
            "binddragend",
            "bind:tap",
            "catch:tap",
            "capture-bind:tap",
            "capture-catch:tap",
            "mut-bind:tap",
            "bind:my-event",
            "bind:my_event2",
        ] {
            assert!(is_event_value_attribute(attribute), "{attribute}");
            assert_eq!(
                classify_event_attribute(attribute),
                EventAttributeClass::Handler,
                "{attribute}"
            );
            // Value routing must stay a subset of the script parser's forms.
            assert!(
                wxml_backend_tsgo::script::events::parse_event_attribute(attribute).is_some(),
                "{attribute}"
            );
        }
    }

    #[test]
    fn value_routing_rejects_malformed_bindings_with_reasons() {
        for (attribute, rejection) in [
            ("bind", EventAttributeRejection::MissingEventName),
            ("catch", EventAttributeRejection::MissingEventName),
            ("capture-bind", EventAttributeRejection::MissingEventName),
            ("capture-catch", EventAttributeRejection::MissingEventName),
            ("mut-bind", EventAttributeRejection::MissingEventName),
            ("bind:", EventAttributeRejection::EmptyEventName),
            ("catch:", EventAttributeRejection::EmptyEventName),
            ("bind::tap", EventAttributeRejection::InvalidEventName),
            ("bind:foo:bar", EventAttributeRejection::InvalidEventName),
            ("bindex", EventAttributeRejection::ConflictingAttribute),
        ] {
            assert_eq!(
                classify_event_attribute(attribute),
                EventAttributeClass::Rejected(rejection),
                "{attribute}"
            );
            assert!(!is_event_value_attribute(attribute), "{attribute}");
        }
        // Compact forms accept kebab and camel event names, like the template layer.
        for attribute in [
            "bind-get-phonenumber",
            "binding-mode",
            "catch-up",
            "bindTap",
            "bindSomeEvent",
            "bindsome-event",
            "capture-bindtap",
            "capture-catchSomeEvent",
            "mut-bindtap",
        ] {
            assert_eq!(
                classify_event_attribute(attribute),
                EventAttributeClass::Handler,
                "{attribute}"
            );
        }
        for ordinary in ["", "bi", "class", "wx:if", "data-onTap"] {
            assert_eq!(
                classify_event_attribute(ordinary),
                EventAttributeClass::Other,
                "{ordinary}"
            );
        }
    }

    #[test]
    fn value_routing_does_not_narrow_to_the_event_name_catalog() {
        for name in [
            "getphonenumber",
            "chooseavatar",
            "play",
            "pause",
            "ended",
            "waiting",
            "progress",
            "fullscreenchange",
            "markertap",
            "callouttap",
            "controltap",
            "poitap",
            "anchorpointtap",
            "dragstart",
            "dragend",
        ] {
            assert!(!EVENT_NAMES.contains(&name), "{name}");
            assert!(is_event_value_attribute(&format!("bind{name}")), "{name}");
        }
    }
}
