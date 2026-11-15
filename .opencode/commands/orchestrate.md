---
description: Orchestrate multiple agents for complex tasks
agent: planner
subagent: true
---

# Orchestrate Command

Orchestrate multiple specialized agents for this complex task: $ARGUMENTS

## Your Task

1. **Analyze task complexity** and break into subtasks
2. **Identify optimal agents** for each subtask
3. **Create execution plan** with dependencies
4. **Coordinate execution** - parallel where possible
5. **Synthesize results** into unified output

## Available Agents

| Agent | Specialty | Use For |
|-------|-----------|---------|
| planner | Implementation planning | Complex feature design |
| architect | System design | Architectural decisions |
| code-reviewer | Code quality | Review changes |
| rust-security-reviewer | Rust security | unsafe/FFI, untrusted input, dependencies |
| rust-tdd-guide | Rust test-driven dev | Feature implementation and tests |
| rust-build-resolver | Rust builds | Rust compiler and clippy errors |
| rust-reviewer | Rust code | Idiomatic Rust review |
| doc-updater | Documentation | Updating docs |
| docs-lookup | Library docs | Current API references |
| harness-optimizer | Agent harness | Config and throughput tuning |
| loop-operator | Autonomous loops | Loop monitoring and intervention |

## Orchestration Patterns

### Sequential Execution
```
planner → rust-tdd-guide → code-reviewer → rust-security-reviewer
```
Use when: Later tasks depend on earlier results

### Parallel Execution
```
┌→ rust-security-reviewer
planner →├→ code-reviewer
└→ architect
```
Use when: Tasks are independent

### Fan-Out/Fan-In
```
         ┌→ agent-1 ─┐
planner →├→ agent-2 ─┼→ synthesizer
         └→ agent-3 ─┘
```
Use when: Multiple perspectives needed

## Execution Plan Format

### Phase 1: [Name]
- Agent: [agent-name]
- Task: [specific task]
- Depends on: [none or previous phase]

### Phase 2: [Name] (parallel)
- Agent A: [agent-name]
  - Task: [specific task]
- Agent B: [agent-name]
  - Task: [specific task]
- Depends on: Phase 1

### Phase 3: Synthesis
- Combine results from Phase 2
- Generate unified output

## Coordination Rules

1. **Plan before execute** - Create full execution plan first
2. **Minimize handoffs** - Reduce context switching
3. **Parallelize when possible** - Independent tasks in parallel
4. **Clear boundaries** - Each agent has specific scope
5. **Single source of truth** - One agent owns each artifact

---

**NOTE**: Complex tasks benefit from multi-agent orchestration. Simple tasks should use single agents directly.
