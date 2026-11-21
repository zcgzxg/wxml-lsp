# Spec Delta

## MODIFIED Requirements

### Requirement: 真实引擎缺失时明确跳过

依赖真实类型查询的 case SHALL 在真实类型门禁未启用或进程内类型引擎初始化失败时跳过，并说明原因；这些 case MUST NOT 用降级结果代替真实类型查询。

#### Scenario: 缺少引擎环境变量

- **GIVEN** 未启用真实类型门禁或缺少 `TSGO_TYPINGS_ROOT`
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
