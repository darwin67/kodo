# Kodo Strategy & Architecture Plan

Kodo is a model-agnostic, terminal-based coding agent written in Rust. It follows
the agentic loop pattern — gather context, take action, verify results — inspired by
[Claude Code](https://code.claude.com/docs/en/how-claude-code-works) and
[OpenCode](https://opencode.ai/docs/).

This document captures the architecture, design decisions, and implementation roadmap.

---

## Design Decisions

| Decision        | Choice                                                              |
|-----------------|---------------------------------------------------------------------|
| Language        | Rust 2024 edition                                                   |
| LLM Providers   | Anthropic (first), then OpenAI, Gemini, Ollama                      |
| LLM Abstraction | Custom `Provider` trait                                             |
| Tool Calling    | Native with text-based fallback                                     |
| TUI             | ratatui + crossterm                                                 |
| Storage         | SQLite                                                              |
| Auth            | Env vars first, OAuth browser flow later                            |
| Permissions     | Plan mode + Build mode (prompt on high-risk in Build)               |
| Streaming       | Essential, day one                                                  |
| Command Palette | `Ctrl+K` default, fully customizable keybinds                       |
| LSP             | Auto-feed diagnostics after edits + explicit `get_diagnostics` tool |
| Formatters      | Silent auto-format after file writes, log changes in UI             |
| Config Files    | Deferred; hardcode defaults for now                                 |

---

## Architecture Overview

```
+-------------------------------------------------------------+
|                     Terminal UI (ratatui)                   |
|  +----------+  +----------+  +-------------+  +-----------+ |
|  |  Input   |  |  Output  |  |  Command    |  |  Status   | |
|  |  Panel   |  |  Stream  |  |  Palette    |  |  Bar      | |
|  +----------+  +----------+  +-------------+  +-----------+ |
+-------------------------------------------------------------+
|                     Agentic Loop (core)                     |
|  +---------------+  +----------+  +-----------+             |
|  | Context Mgr   |  | Planner  |  | Executor  |             |
|  | (gather)      |  | (reason) |  | (act)     |             |
|  +---------------+  +----------+  +-----------+             |
+-------------------------------------------------------------+
|                      Tool System                            |
|  +------+ +-------+ +--------+ +-------+ +---------+        |
|  | File | | Shell | | Search | |  Web  | | Diag-   |        |
|  | Ops  | | Exec  | | (glob/ | | Fetch | | nostics |        |
|  |      | |       | | regex) | |       | | (LSP)   |        |
|  +------+ +-------+ +--------+ +-------+ +---------+        |
+-------------------------------------------------------------+
|  Formatter          |  LSP Manager          | Permissions   |
|  (auto-format       |  (diagnostics,        | (plan/build,  |
|   after writes)     |   server lifecycle)   |  checkpoints) |
+-------------------------------------------------------------+
|                   LLM Provider Layer                        |
|  +----------+ +--------+ +--------+ +-----------+           |
|  | Anthropic| | OpenAI | | Gemini | | Ollama    |           |
|  +----------+ +--------+ +--------+ +-----------+           |
+-------------------------------------------------------------+
|                   Persistence (SQLite)                      |
|  +----------+ +----------+ +------------+ +----------+      |
|  | Sessions | | Messages | | Checkpoints| | Auth     |      |
|  +----------+ +----------+ +------------+ +----------+      |
+-------------------------------------------------------------+
```

---

## Project Structure

```
kodo/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── kodo-core/              # agentic loop, messages, context, modes
│   │   ├── agent.rs            # main agentic loop
│   │   ├── message.rs          # Message, Role, ContentBlock, ToolCall, ToolResult
│   │   ├── context.rs          # context window tracking & compaction
│   │   └── mode.rs             # Plan vs Build mode
│   ├── kodo-llm/               # provider trait + implementations
│   │   ├── provider.rs         # Provider trait (complete + stream)
│   │   ├── types.rs            # CompletionRequest, StreamChunk, etc.
│   │   ├── anthropic.rs        # Anthropic Claude
│   │   ├── openai.rs           # OpenAI GPT
│   │   ├── gemini.rs           # Google Gemini
│   │   └── ollama.rs           # Ollama (local)
│   ├── kodo-tools/             # tool trait + built-in tools
│   │   ├── tool.rs             # Tool trait
│   │   ├── registry.rs         # ToolRegistry & dispatch
│   │   ├── file_read.rs
│   │   ├── file_write.rs
│   │   ├── file_edit.rs
│   │   ├── shell.rs
│   │   ├── glob_search.rs
│   │   ├── grep_search.rs
│   │   └── web_fetch.rs
│   ├── kodo-tui/               # terminal UI
│   │   ├── app.rs              # app state, event loop
│   │   ├── input.rs            # user input handling
│   │   ├── output.rs           # streaming output, markdown rendering
│   │   ├── palette.rs          # Ctrl+K command palette modal
│   │   ├── keybinds.rs         # keybind registry & customization
│   │   ├── theme.rs            # color themes
│   │   └── status.rs           # status bar (mode, model, tokens)
│   ├── kodo-store/             # SQLite persistence
│   │   ├── db.rs               # connection, migrations
│   │   ├── session.rs          # session CRUD
│   │   ├── auth.rs             # auth token storage
│   │   ├── checkpoint.rs       # file snapshots for undo
│   │   └── memory.rs           # project memory
│   ├── kodo-lsp/               # LSP client integration
│   │   ├── manager.rs          # start/stop servers, route by extension
│   │   ├── client.rs           # single LSP server connection (JSON-RPC/stdio)
│   │   ├── diagnostics.rs      # collect & format diagnostics for LLM
│   │   └── config.rs           # built-in server configs + custom support
│   └── kodo-fmt/               # auto-formatting
│       ├── registry.rs         # formatter registry: extension -> command
│       ├── runner.rs           # execute formatter, capture result
│       └── config.rs           # built-in configs + custom support
├── src/
│   └── main.rs                 # CLI entry point
└── docs/
    └── STRATEGY.md             # this file
```

---

## Core Concepts

### Agentic Loop

The main loop drives all interactions:

```
User Input
    |
    v
Build Messages (system prompt + history + user input)
    |
    v
+-> Send to LLM (streaming) --> Display tokens --+
|                                                 |
|   Tool call detected?                           |
|     YES -> Check permissions (Plan/Build mode)  |
|          -> Execute tool                        |
|          -> Run formatter (if file write/edit)  |
|          -> Collect LSP diagnostics             |
|          -> Append result to messages           |
|          -> Loop back to LLM  <-----------------+
|     NO  -> Response complete, wait for user input
+--------------------------------------------------
```

### Permission Modes

Two modes, switchable via command palette or keybind:

| Mode      | Description                   | File Read | File Write | Shell | Web  |
|-----------|-------------------------------|-----------|------------|-------|------|
| **Plan**  | Read-only analysis & planning | Auto      | Deny       | Deny  | Auto |
| **Build** | Full execution                | Auto      | Auto       | Auto* | Auto |

*In Build mode, high-risk commands trigger a confirmation prompt. High-risk is
determined by a deny-list of patterns: `rm -rf`, `git push --force`, `DROP TABLE`,
`shutdown`, etc.

### LLM Provider Trait

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
    async fn stream(&self, request: CompletionRequest)
        -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>>>>>;
    fn tool_calling_support(&self) -> ToolCallingSupport;
    fn name(&self) -> &str;
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
}

enum ToolCallingSupport {
    Native,     // Anthropic, OpenAI, Gemini
    TextBased,  // Parse XML/JSON from text output
    None,       // Pure text completion
}
```

### Tool Trait

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn permission_level(&self) -> PermissionLevel;  // Read, Write, Execute

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput>;
}
```

### Command Palette (`Ctrl+K`)

A fuzzy-searchable modal overlay for all kodo actions:

| Category | Command         | Description                          |
|----------|-----------------|--------------------------------------|
| Session  | New Session     | Start a fresh session                |
| Session  | Switch Session  | List and switch to existing sessions |
| Session  | Fork Session    | Fork current session                 |
| Session  | Export Session  | Export conversation as markdown      |
| Model    | Switch Model    | Pick from available models           |
| Model    | Switch Provider | Change the active LLM provider       |
| Mode     | Plan Mode       | Switch to read-only mode             |
| Mode     | Build Mode      | Switch to full execution mode        |
| Theme    | Change Theme    | Pick from color themes               |
| System   | Compact Context | Summarize to free context window     |
| System   | Undo Last Edit  | Revert last file change              |
| System   | Redo            | Redo last undone change              |

The palette trigger defaults to `Ctrl+K` and is fully customizable. Direct keybind
shortcuts (leader key + letter) are also supported for frequent actions.

### LSP Integration

After every file write or edit:
1. Notify the LSP server of the change (`textDocument/didChange`)
2. Collect diagnostics (`textDocument/publishDiagnostics`)
3. Inject errors/warnings into the agentic loop as context
4. The LLM can then fix issues in the next iteration

Additionally, a `get_diagnostics` tool is available for the LLM to explicitly request
diagnostics for any file at any time.

**Phase 1 built-in LSP servers:**

| Server        | Extensions                   | Requirements              |
|---------------|------------------------------|---------------------------|
| rust-analyzer | `.rs`                        | `rust-analyzer` available |
| gopls         | `.go`                        | `go` command available    |
| typescript    | `.ts`, `.tsx`, `.js`, `.jsx` | `typescript` in project   |
| pyright       | `.py`, `.pyi`                | `pyright` installed       |

Expand to full OpenCode parity later (35+ servers).

### Formatter Integration

After every file write or edit, the appropriate formatter runs silently. Changes are
logged in the UI but not fed back to the LLM (reduces noise).

**Phase 1 built-in formatters:**

| Formatter | Extensions                                         | Requirements          |
|-----------|----------------------------------------------------|-----------------------|
| cargo fmt | `.rs`                                              | `cargo fmt` available |
| gofmt     | `.go`                                              | `gofmt` available     |
| prettier  | `.js`, `.ts`, `.jsx`, `.tsx`, `.html`, `.css`, etc | `prettier` in project |
| ruff      | `.py`, `.pyi`                                      | `ruff` available      |

Expand to full OpenCode parity later (30+ formatters).

### Session Persistence (SQLite)

Tables:
- **sessions** — id, directory, branch, created_at, updated_at
- **messages** — id, session_id, role, content, tool_calls, tool_results, created_at
- **checkpoints** — id, session_id, file_path, content_snapshot, created_at
- **auth_tokens** — id, provider, token, refresh_token, expires_at
- **memory** — id, key, value, scope (global/project), created_at

---

## Key Dependencies

| Purpose             | Crate                              |
|---------------------|------------------------------------|
| Async runtime       | `tokio`                            |
| HTTP client         | `reqwest`                          |
| Serialization       | `serde`, `serde_json`              |
| SQLite              | `sqlx` (sqlite + migrate features) |
| TUI framework       | `ratatui`, `crossterm`             |
| Streaming           | `tokio-stream`, `futures`          |
| CLI parsing         | `clap`                             |
| Error handling      | `anyhow`, `thiserror`              |
| Regex / glob        | `regex`, `glob`                    |
| Syntax highlighting | `syntect`                          |
| Markdown rendering  | `termimad` or `pulldown-cmark`     |
| Fuzzy matching      | `nucleo`                           |
| Logging             | `tracing`, `tracing-subscriber`    |
| LSP protocol types  | `lsp-types`                        |

---

## Milestones

### Phase 1 — Skeleton & End-to-End Chat

Get a working chat loop: user types a message, Anthropic responds with streaming.

- [x] Set up Cargo workspace with all crate stubs
- [x] Define core message types (`Message`, `Role`, `ContentBlock`, `ToolCall`, `ToolResult`)
- [x] Define `Provider` trait in `kodo-llm` with `complete()` and `stream()`
- [x] Implement Anthropic provider with streaming (SSE parsing)
- [x] Basic terminal I/O (readline-style input, print streamed tokens)
- [x] Wire up agentic loop: user input -> build messages -> send to LLM -> stream response
- [x] Auth via `ANTHROPIC_API_KEY` env var

### Phase 2 — Tool System & Formatters

Give the agent the ability to act on the codebase.

- [x] Define `Tool` trait and `ToolRegistry`
- [x] Implement `file_read` tool
- [x] Implement `file_write` tool
- [x] Implement `file_edit` tool (string replacement)
- [x] Implement `shell` execution tool
- [x] Implement `glob_search` tool
- [x] Implement `grep_search` tool
- [x] Implement `web_fetch` tool
- [x] Wire tool calling into agentic loop (LLM tool_use -> dispatch -> feed result back)
- [ ] Text-based tool calling fallback (XML/JSON parsing for models without native support)
- [x] `kodo-fmt` crate: formatter registry + runner
- [x] Hook formatters into file_write / file_edit (silent format, log to UI)
- [x] Built-in formatters: cargo fmt, gofmt, prettier, ruff

### Phase 3 — Permissions & Safety

Protect the user from destructive actions.

- [x] Plan mode: restrict tools to read-only (file_read, glob, grep, web_fetch)
- [x] Build mode: all tools enabled
- [x] High-risk command detection (deny-list patterns)
- [x] Confirmation prompt for high-risk actions in Build mode
- [x] File checkpoints: snapshot content before each edit
- [x] Undo: revert to last checkpoint

### Phase 4 — More Providers

Make kodo truly model-agnostic.

- [ ] OpenAI provider (Chat Completions API + streaming)
- [ ] Google Gemini provider
- [ ] Ollama provider (local, OpenAI-compatible API)
- [ ] Model switching command (change provider/model mid-session)

### Phase 5 — Persistence & Sessions

Remember conversations across restarts.

- [ ] SQLite setup: schema and migrations
- [ ] Session creation with auto-generated ID
- [ ] Session listing and resume
- [ ] Session fork
- [ ] Conversation history persistence (save/load messages)
- [ ] Auth token storage in DB
- [ ] `KODO.md` project memory file: load at session start

### Phase 6 — Full TUI (ratatui)

Upgrade from readline to a proper terminal UI.

- [ ] Ratatui app scaffold: event loop, terminal setup/teardown
- [ ] Layout: input panel (bottom), output panel (scrollable), status bar
- [ ] Streaming output rendering with markdown support
- [ ] Syntax highlighting for code blocks
- [ ] Command palette (`Ctrl+K`): modal overlay, fuzzy search, command dispatch
- [ ] Keybind system: registry, customizable mappings, leader key support
- [ ] Theme system: built-in dark/light themes, switchable via palette
- [ ] Status bar: current mode, model, provider, token usage, session info
- [ ] Mode toggle via palette or keybind

### Phase 7 — LSP Integration

Give the agent awareness of type errors and diagnostics.

- [ ] `kodo-lsp` crate: LSP client over stdio (JSON-RPC)
- [ ] `LspManager`: detect file extensions, auto-start appropriate server
- [ ] Built-in configs: rust-analyzer, gopls, typescript, pyright
- [ ] Hook into agentic loop: after file edits -> notify LSP -> collect diagnostics -> inject into context
- [ ] `get_diagnostics` tool: explicit on-demand diagnostics for the LLM
- [ ] Graceful server lifecycle (start on first relevant file, shutdown on session end)

### Phase 8 — Polish & Future

Refinements and advanced features.

- [ ] Context window management: track token counts, auto-compact when nearing limit
- [ ] OAuth browser login flows (Anthropic, OpenAI)
- [ ] Prompt caching (Anthropic)
- [ ] Subagent support (spawn isolated agent tasks)
- [ ] MCP server integration
- [ ] Config file support (YAML + TOML)
- [ ] Custom LSP servers and formatters via config
- [ ] Auto-install LSP servers when missing

---

## References

- [Claude Code — How it works](https://code.claude.com/docs/en/how-claude-code-works)
- [OpenCode — LSP Servers](https://opencode.ai/docs/lsp/)
- [OpenCode — Formatters](https://opencode.ai/docs/formatters/)
- [OpenCode — Keybinds](https://opencode.ai/docs/keybinds/)
- [OpenCode — Commands](https://opencode.ai/docs/commands/)
