# query-benchmarks Specification

## Purpose

为一组预先选定的 WXML 查询路径提供可重复、可比较、结果可靠的耗时基准，使查询实现
的性能变化可以被测量而不是靠猜测。

## Requirements

### Requirement: 基准按查询路径独立组织

基准 SHALL 为每条生产查询路径建立独立计时的 case，并用稳定的组名标识路径与变体，
使单条路径可以被单独过滤和比较。变量类型、标签信息和 class 三组 SHALL 至少覆盖
本变更声明的路径集合。

#### Scenario: 变量类型查询单独计时

- **GIVEN** 一个含脚本类型信息、已打开 WXML 文档的 fixture 项目
- **WHEN** 运行变量类型组
- **THEN** 根变量、成员访问和模板局部变量各自产生独立的计时结果
- **AND** 每个结果可通过组名单独过滤

#### Scenario: 标签信息查询单独计时

- **GIVEN** 含内置标签、自定义组件、属性和属性值的 WXML fixture
- **WHEN** 运行标签信息组
- **THEN** 标签、属性名和属性值 hover 各自产生独立的计时结果

#### Scenario: class 查询按操作分别计时

- **GIVEN** 含多条 class 属性与 WXSS 规则的 fixture
- **WHEN** 运行 class 组
- **THEN** 类名提取、类名识别、class hover、class 补全和 class 定义跳转分别计时
- **AND** 类名提取与识别不依赖样式索引，hover/补全/定义使用样式索引

### Requirement: 预热与首次查询分别报告

基准 SHALL 分别测量已就绪会话上的重复查询和新会话上的首次查询，并在结果中区分两
者；首次查询 MUST NOT 与预热数据混合平均。

#### Scenario: 预热查询复用已建立的索引

- **GIVEN** 项目、后端会话和样式索引已经建立
- **WHEN** 重复执行同一查询
- **THEN** 计时只覆盖该次查询本身
- **AND** 结果标记为预热模式

#### Scenario: 首次查询包含准备工作

- **GIVEN** 一个尚未查询过的新会话
- **WHEN** 执行首次查询
- **THEN** 计时包含该路径实际发生的发现、索引构建和文档打开成本
- **AND** 结果标记为首次查询模式，不与预热结果混算

### Requirement: 结果正确性先于计时

每个 case SHALL 在执行计时前校验查询返回了预期结果。空结果、降级状态或能力
`Unavailable` MUST 使该 case 失败，而不是产生看似更快的计时。

#### Scenario: 类型查询返回具体类型

- **GIVEN** 变量类型 case 的预期类型
- **WHEN** 校验查询结果
- **THEN** 结果包含该具体类型
- **AND** 仅当校验通过后才进入计时

#### Scenario: 无效结果导致失败

- **GIVEN** 后端返回空结果或不可用状态
- **WHEN** 运行对应 case
- **THEN** 基准以明确错误失败
- **AND** 不输出该 case 的耗时数据

### Requirement: 真实引擎缺失时明确跳过

依赖真实类型查询的 case SHALL 在真实类型门禁未启用或进程内类型引擎初始化失败时跳过，并说明原因；这些 case MUST NOT 用降级结果代替真实类型查询。

#### Scenario: 缺少引擎环境变量

- **GIVEN** 未启用真实类型门禁或缺少 `TSRS_TYPINGS_ROOT`
- **WHEN** 运行变量类型组
- **THEN** 基准报告跳过原因
- **AND** 不产生该组的耗时数据

#### Scenario: 未启用真实类型门禁

- **GIVEN** 未启用真实类型查询门禁
- **WHEN** 运行变量类型组
- **THEN** 基准报告跳过原因
- **AND** 不产生该组的耗时数据

#### Scenario: 进程内引擎初始化失败

- **GIVEN** 真实类型门禁已启用但进程内引擎初始化失败
- **WHEN** 运行变量类型组
- **THEN** 基准报告失败或跳过原因
- **AND** 不使用降级结果计时

### Requirement: 基准可过滤且结果可比较

基准 SHALL 使用 Criterion 输出，组名稳定，允许按路径过滤运行，并支持与 baseline
比较；默认不加入 CI 门禁。

#### Scenario: 只运行一条路径

- **GIVEN** 已构建的 bench 目标
- **WHEN** 使用组名过滤运行
- **THEN** 只执行匹配的 case
- **AND** 其他路径不参与本次测量
