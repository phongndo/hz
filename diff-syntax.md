# Diff syntax highlighting plan

`hz diff` syntax highlighting should stay fast, explicit, and safe:

- `hz diff` never downloads parsers while rendering.
- Users opt in with `hz ts add <language>` and remove with `hz ts rm <language>`.
- Missing parsers, missing highlight queries, oversized hunks, and oversized lines fall back to plain diff text.
- Diff backgrounds stay authoritative; syntax highlighting only supplies foreground color.
- Parser/highlight work must not block scrolling.

## Phase 0: MVP foundation

Status: implemented as first step.

- Add `hz-syntax` as the Tree-sitter integration boundary.
- Add `hz ts` / `hz tree-sitter` language management commands.
- Store enabled languages in user-local config.
- Use `tree-sitter-language-pack` for parser downloads/cache management.
- Use `tree-sitter-highlight` for highlight events.
- Start hunk-local highlighting in the TUI.
- Run highlighting on a background worker.
- Cache highlighted hunk sides by generation/file/hunk/side.
- Add hard caps for hunk bytes, line bytes, and cache entries.
- Render plain diff immediately and repaint when syntax results arrive.

## Phase 1: harden the MVP

- Replace arbitrary cache eviction with a small LRU cache.
- Add a bounded worker queue so fast scrolling cannot enqueue unlimited work.
- Prioritize visible rows, then one viewport of prefetch ahead/behind.
- Drop stale queued jobs when live reload increments the generation.
- Add tests proving `hz diff` does not download parsers.
- Add tests for missing parser, missing query, huge hunk, and huge line fallback.
- Improve `hz ts rm` output to distinguish config removal from physical parser deletion.
- Add clearer `hz ts doctor` checks for stale config, missing cache files, and load failures.

## Phase 2: benchmark and tune performance

- Extend benchmark scenarios with syntax-enabled runs.
- Measure initial TUI open latency, scroll latency, cache hit rate, queue depth, and memory growth.
- Tune defaults for:
  - max hunk bytes
  - max line bytes
  - cache entries
  - queue capacity
  - prefetch distance
- Add stress coverage for generated/minified files, large single files, and many-small-file diffs.
- Keep the fallback path cheap enough that unsupported languages remain as fast as plain diff.

## Phase 3: polish language management

- Add `hz ts available --installed` and `hz ts available --enabled` filters.
- Add `hz ts update <language>` and `hz ts update --all`.
- Add language groups:
  - `compiler`: `llvm`, `mlir`, `asm`, `nasm`, `tablegen`, `cmake`, `ninja`
  - `systems`: `rust`, `c`, `cpp`, `go`, `zig`, `bash`, `make`, `cmake`
  - `web`: `javascript`, `typescript`, `tsx`, `jsx`, `html`, `css`, `json`, `yaml`, `toml`
- Add `hz ts add --group <name>`.
- Add aliases for common user inputs like `c++`, `shell`, `sh`, and extensions.
- Consider a `hz ts doctor --repair` mode for stale enabled languages.

## Phase 4: theme and config support

- Add user-local syntax config for performance and theme knobs.
- Keep repo-local `.hz/hz.toml` out of parser download decisions.
- Add configurable syntax theme names such as `terminal-dark`, `terminal-light`, and `minimal`.
- Allow disabling syntax globally without removing installed parsers.
- Keep color layering stable:
  1. diff row background
  2. syntax foreground
  3. inline change emphasis
  4. gutter stays muted

## Phase 5: full-file highlighting for correctness

Current hunk-local highlighting is fast and patch-compatible, but can be wrong when multiline parser state starts before a hunk.

- For repo-backed diffs, load full old/new file contents.
- Highlight whole old/new sides and map styled spans back to diff lines.
- Keep hunk-local highlighting for patch input and unavailable file contents.
- Handle worktree, staged, unstaged, base, range, untracked, renamed, and deleted files explicitly.
- Reuse full-file highlighted spans across hunks in the same file.

## Phase 6: inline diff emphasis

- Add word/token-level changed-region emphasis.
- Compute inline differences independently of syntax highlighting.
- Layer inline emphasis over syntax foreground and diff backgrounds.
- Keep split and unified layouts visually consistent.
- Cap expensive inline diff work for very long lines.

## Phase 7: semantic diff experiments

Only start this after syntax highlighting is stable and benchmarked.

- Use Tree-sitter ASTs to identify moved/renamed syntax nodes.
- Experiment with syntax-aware hunk summaries.
- Explore language-specific structure navigation inside diffs.
- Keep semantic diff optional; plain Git patch semantics remain the source of truth.

## Non-goals for now

- No parser downloads from the interactive renderer.
- No JavaScript/WASM highlighter runtime.
- No required network access for ordinary `hz diff` use.
- No semantic diff rewrite until line-oriented syntax highlighting is fast and stable.
