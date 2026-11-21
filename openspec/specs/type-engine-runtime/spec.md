# type-engine-runtime Specification

## Purpose

本能力规定 WXML 语言服务器如何承载 TypeScript 类型引擎：引擎与语言服务器同进程运行，版本和 AST 协议在启动时固定，标准库随二进制内嵌。所有源文件、typings 和标准库都通过只读虚拟文件系统提供，类型查询不得启动外部引擎进程、依赖 Node 运行时或回退到宿主文件系统。

## Requirements

### Requirement: 类型引擎在语言服务器进程内运行

服务器 SHALL 在语言服务器进程内完成类型查询，不得启动外部 TypeScript 引擎进程，也不得依赖 Node 运行时。

#### Scenario: 类型查询不产生外部引擎进程

- **GIVEN** 已启动且类型能力可用的语言服务器
- **WHEN** 处理一次 hover 或类型查询
- **THEN** 查询由进程内类型引擎完成
- **AND** 不创建外部引擎子进程

### Requirement: 引擎版本与协议在启动时固定

服务器 SHALL 在提供类型能力前验证引擎的 TypeScript 版本、AST 协议版本、头部大小和节点步长与构建时固定值一致；任一不一致 MUST 使类型能力不可用，不得部分启用。

#### Scenario: 版本或 AST schema 不匹配

- **GIVEN** 引擎报告的版本或 AST schema 与固定值不同
- **WHEN** 初始化类型能力
- **THEN** 类型能力标记为不可用并给出原因
- **AND** 不执行任何类型查询

### Requirement: 标准库随类型引擎内嵌

服务器 SHALL 从进程内引擎提供 TypeScript 标准库，不得要求安装目录中存在外部标准库文件；项目类型声明仍 MUST 来自受信任的 typings 资产。

#### Scenario: 没有外部标准库文件

- **GIVEN** 安装目录只有 typings、没有 TypeScript 标准库文件
- **WHEN** 打开项目并查询内置类型
- **THEN** 标准库类型可正常解析
- **AND** 项目 typings 仍按受信任资产校验

### Requirement: 虚拟文件系统只读且禁止宿主回退

类型引擎的所有文件读取 SHALL 只访问当前快照的虚拟文件表和受信任资产根；对宿主路径的读取 MUST 返回不存在，写入、追加、删除和修改时间 MUST 被拒绝。

#### Scenario: 引擎尝试读取宿主文件

- **GIVEN** 虚拟文件系统未包含某个宿主真实文件
- **WHEN** 类型引擎请求该路径
- **THEN** 返回文件不存在
- **AND** 不读取宿主文件内容

#### Scenario: 引擎尝试写入

- **GIVEN** 类型引擎对任意路径发起写入或删除
- **WHEN** 虚拟文件系统处理该请求
- **THEN** 返回错误
- **AND** 宿主文件不被修改

### Requirement: 单个请求失败不影响语言服务器存活

单个类型请求发生 panic 或返回内部错误时，服务器 SHALL 将该请求报告为失败并保持语言服务器可响应；若引擎状态无法继续使用，类型能力 MUST 标记为不可用，而不是让进程退出。

#### Scenario: 单个请求 panic

- **GIVEN** 类型引擎可用
- **WHEN** 某个查询触发 panic
- **THEN** 该请求返回错误
- **AND** 语言服务器继续处理其他请求

### Requirement: 关闭时释放快照与句柄

服务器关闭类型能力时 SHALL 释放所有快照、租约和缓存，并 MUST NOT 留下需要回收的外部引擎进程。

#### Scenario: 正常关闭

- **GIVEN** 已打开项目且存在活动快照
- **WHEN** 语言服务器关闭类型能力
- **THEN** 所有快照与租约被释放
- **AND** 没有外部引擎进程需要等待
