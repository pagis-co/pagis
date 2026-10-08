# 0001: Pagis owns its agent loop

Status: accepted.

## Context

An agent platform can run on an existing harness or on its own loop. Pagis
needs hooks at every turn: memory injection, memory commits, agent-to-agent
messages in Threads, tool assembly in the capability broker, and the handback
of computer control. A host framework owns those points.

## Decision

Pagis writes its own agent loop in Rust on `crates/llm-router`. An external
harness, such as a coding CLI, connects only as a guest coding harness over
ACP.

ACP stays the guest protocol. A native driver, such as the Codex app-server
or pi RPC, is allowed only where ACP lacks a capability that Pagis needs
(ADR-0033).

## Consequences

- Pagis controls every turn, and no host framework competes for it.
- Pagis maintains the loop itself. The loop stays small, and borrowed code
  stays at the edges: MCP clients, telephony and media.
