model = """export interface User { /** Display name from model */ Name: string; Age?: number; }"""
case(
    "local_docs",
    """interface User { /** Display name from interface */ Name: string; Age?: number; } Page({data:{user:{} as User}})""",
    "<view>{{ user.Name }} {{user.Age}} {{user.Nmae}}</view>",
    [("user.Name", 6), ("user.Age", 6)],
)
case(
    "literal_docs",
    """Page({data:{user:{/** Display name inline */ Name: 'Ada'}}})""",
    "<view>{{user.Name}}</view>",
    [("user.Name", 6)],
)
case(
    "relative_interface",
    """import type { User } from './model'; Page({data:{user:{} as User}})""",
    "<view>{{user.Name}} {{user.Age}} {{user.Nmae}}</view>",
    [("user.Name", 6), ("user.Age", 6)],
    {"model.ts": model},
)
case(
    "relative_alias",
    """import { User as Person } from './model'; Page({data:{user:{} as Person}})""",
    "<view>{{user.Name}}</view>",
    [("user.Name", 6)],
    {"model.ts": model},
)
case(
    "namespace_import",
    """import * as M from './model'; Page({data:{user:{} as M.User}})""",
    "<view>{{user}} {{user.Name}}</view>",
    ["user", ("user.Name", 6)],
    {"model.ts": model},
)
case(
    "path_alias",
    """import type { User } from '@/model'; Page({data:{user:{} as User}})""",
    "<view>{{user}} {{user.Name}}</view>",
    ["user", ("user.Name", 6)],
    {
        "model.ts": model,
        "tsconfig.json": json.dumps(
            {"compilerOptions": {"baseUrl": ".", "paths": {"@/*": ["./*"]}}}
        ),
    },
)
case(
    "transitive_interface",
    """import type { User } from './model'; Page({data:{user:{} as User}})""",
    "<view>{{user.profile}} {{user.profile.Name}}</view>",
    [("user.profile", 6), ("user.profile.Name", 14)],
    {
        "model.ts": """import type { Profile } from './profile'; export interface User { profile: Profile; }""",
        "profile.ts": """export interface Profile { /** profile name */ Name: string; }""",
    },
)
case(
    "reexport_interface",
    """import type { User } from './barrel'; Page({data:{user:{} as User}})""",
    "<view>{{user}} {{user.Name}}</view>",
    ["user", ("user.Name", 6)],
    {"model.ts": model, "barrel.ts": """export type { User } from './model';"""},
)
case(
    "forward_nested",
    """interface User { profile: Profile; } interface Profile { Name: string; } Page({data:{user:{} as User}})""",
    "<view>{{user.profile}} {{user.profile.Name}}</view>",
    [("user.profile", 6), ("user.profile.Name", 14)],
)
case(
    "backward_nested",
    """interface Profile { /** Nested name docs */ Name: string; } interface User { profile: Profile; } Page({data:{user:{} as User}})""",
    "<view>{{user.profile.Name}}</view>",
    [("user.profile.Name", 14)],
)
case(
    "inline_type",
    """interface User { profile: { /** Nested name docs */ Name: string }; } Page({data:{user:{} as User}})""",
    "<view>{{user.profile}} {{user.profile.Name}}</view>",
    [("user.profile", 6), ("user.profile.Name", 14)],
)
case(
    "const_annotation",
    """interface User { Name: string; Age?: number; } const user: User = {Name:'Ada'}; Page({data:{user}})""",
    "<view>{{user}} {{user.Age}}</view>",
    ["user", ("user.Age", 6)],
)
case(
    "import_then_const",
    """import { User } from './model'; const user = {} as User; Page({data:{user}})""",
    "<view>{{user}} {{user.Name}}</view>",
    ["user", ("user.Name", 6)],
    {"model.ts": model},
)
case(
    "extends_and_alias",
    """interface Base { Name: string; } interface User extends Base { Age: number; } type Alias = { Name: string }; Page({data:{user:{} as User, alias:{} as Alias}})""",
    "<view>{{user.Name}} {{user.Age}} {{alias.Name}}</view>",
    [("user.Name", 6), ("user.Age", 6), ("alias.Name", 7)],
)
case(
    "union_null",
    """interface User { Name: string; } Page({data:{user:null as User | null, state: 'a' as 'a' | 'b', nil: null}})""",
    """<view>{{user}} {{user.Name}} {{state}} {{nil.Name}}</view><view wx:if="{{user}}">{{user.Name}}</view>""",
    ["user", ("user.Name", 6), "state", ("user.Name", 6, 1)],
)
case(
    "union_field",
    """interface User { Name: string | number; Nick?: string; } Page({data:{user:{} as User}})""",
    "<view>{{user.Name}} {{user.Nick}}</view>",
    [("user.Name", 6), ("user.Nick", 6)],
)
case(
    "component",
    """Component({properties:{title:String,count:{type:Number,value:0},user:{type:Object,value:{Name:'Ada'}}},data:{enabled:true}})""",
    "<view>{{title}} {{count}} {{user.Name}} {{enabled}}</view>",
    ["title", "count", ("user.Name", 6), "enabled"],
)
case(
    "component_optional_types",
    """Component({properties:{value:{type:String,optionalTypes:[Number],value:'x'}}})""",
    "<view>{{value}}</view>",
    ["value"],
)
case(
    "page_generic",
    """interface Data { user: {Name:string} } Page<Data, {}>({data:{user:null}})""",
    "<view>{{user}} {{user.Name}}</view>",
    ["user", ("user.Name", 6)],
)
case(
    "loop_scope",
    """Page({data:{groups:[{Name:'group',children:[{Name:7}]}]}})""",
    """<view wx:for="{{groups}}" wx:key="Name">{{item.Name}} {{index}}<view wx:for="{{item.children}}" wx:key="Name">{{item.Name}}</view>{{item.Name}}</view><view>{{item.Name}}</view>""",
    [
        ("item.Name", 6, 0),
        ("item.children", 6),
        ("item.Name", 6, 1),
        ("item.Name", 6, 2),
        ("item.Name", 6, 3),
        "index",
    ],
)
case(
    "loop_alias",
    """Page({data:{users:[{Name:'Ada'}]}})""",
    """<view wx:for="{{users}}" wx:for-item="u" wx:for-index="i" wx:key="Name">{{u.Name}} {{i}} {{u.Nmae}}</view>""",
    [("u.Name", 3), ("{{i}}", 2)],
)
case(
    "loop_heterogeneous",
    """Page({data:{users:[{Name:'Ada'},{Name:1}]}})""",
    """<view wx:for="{{users}}" wx:key="Name">{{item}} {{item.Name}}</view>""",
    [("{{item}}", 2), ("item.Name", 6)],
)
case(
    "attributes",
    """Page({data:{obj:{Name:'Ada'},text:'not number',num:42}})""",
    """<slider value="{{text}}"/><view hidden="{{obj}}"/><my-card count="{{text}}"/><my-card count="{{obj}}"/>""",
    ["text", "obj"],
    {
        "index.json": json.dumps({"usingComponents": {"my-card": "./card"}}),
        "card.ts": "Component({properties:{count:Number}})",
        "card.wxml": "<view/>",
        "card.json": json.dumps({"component": True}),
    },
)
case(
    "expression_checks",
    """Page({data:{user:{Name:'Ada'},n:1}})""",
    """<view>{{user.Name * 2}}</view><view>{{n + missing}}</view><view>{{!missing}}</view><view>{{user['Nmae']}}</view><view>{{user.Name.Nmae}}</view><view>{{123}}</view>""",
    [("user.Name", 6), "missing", ("user.Name.Nmae", 12)],
)
case(
    "unicode_range",
    """Page({data:{user:{Name:'Ada'}}})""",
    "<view>中文😀{{ user.Nmae }}</view>",
    [],
)
uri = case(
    "unsaved_ts",
    """Page({data:{user:{Name:'Ada'}}})""",
    "<view>{{user.Name}}</view>",
    [("user.Name", 6)],
)
send(
    "textDocument/didOpen",
    {
        "textDocument": {
            "uri": uri.replace(".wxml", ".ts"),
            "languageId": "typescript",
            "version": 1,
            "text": "Page({data:{user:{Name:42}}})",
        }
    },
)
results[-1]["hover_after_unsaved_ts_open"] = query_hover(
    uri, "<view>{{user.Name}}</view>", "user.Name", 6
)
case(
    "import_const",
    """import { users } from './model'; Page({data:{users}})""",
    """<view wx:for="{{users}}" wx:key="Name">{{item.Name}}</view>""",
    [("item.Name", 6)],
    {
        "model.ts": """export interface User { Name:string; } export const users = [{Name:'Ada'}] as User[];"""
    },
)
case(
    "data_const_object",
    """const initial = {user:{Name:'Ada'}}; Page({data:initial})""",
    "<view>{{user.Name}}</view>",
    [("user.Name", 6)],
)
case(
    "data_spread",
    """const initial = {user:{Name:'Ada'}}; Page({data:{...initial}})""",
    "<view>{{user.Name}}</view>",
    [("user.Name", 6)],
)
case(
    "discriminated_union",
    """interface State { kind: 'a' | 'b'; payload: string | number; } Page({data:{state:{} as State}})""",
    """<view wx:if="{{state.kind === 'a'}}">{{state.payload}}</view><view wx:else>{{state.payload}}</view>""",
    [("state.payload", 8, 0), ("state.payload", 8, 1)],
)
case(
    "component_generic_assertion",
    """interface User { Name:string; } Component({properties:{user:{type:Object as WechatMiniprogram.PropType<User>}}})""",
    "<view>{{user}} {{user.Name}}</view>",
    ["user", ("user.Name", 6)],
)
case(
    "unicode_hover",
    """Page({data:{user:{Name:'Ada'}}})""",
    "<view>中文😀{{ user.Name }}</view>",
    [("user.Name", 6)],
)
case(
    "true_discriminated_union",
    """interface A { kind: 'a'; Name: string; } interface B { kind: 'b'; Count: number; } Page({data:{state:{} as A | B}})""",
    """<view wx:if="{{state.kind === 'a'}}">{{state.Name}}</view><view wx:else>{{state.Count}}</view>""",
    [("state.Name", 7), ("state.Count", 7)],
)
