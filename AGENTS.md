# AGENTS.md

This file provides guidance to AI coding agents working in this repository.

`AGENTS.md` is the canonical instruction file. Tool-specific files, such as
`CLAUDE.md`, should point agents here rather than duplicating these
instructions.

## What this is

**plumb** is a **strict markup language** and its tooling, built for personal use. Where
[Djot](https://djot.net) and Markdown are deliberately error-tolerant, plumb is
deliberately strict: malformed syntax is a hard parse error rather than a silent
fallback to literal text. The current design direction is "special spellings are
always special" rather than "every punctuation character is globally special."

Start at `docs/index.plumb`. For implementation work, read these sources of
truth in order:

- `docs/reference/core-syntax.plumb` — authoritative core syntax validity.
- `docs/reference/standard-extensions.plumb` — official semantic profile.
- `docs/reference/diagnostics.plumb` — diagnostic ownership and recovery policy.
- `docs/architecture/overview.plumb` — crates, data flow, and tool boundaries.
- `docs/architecture/extension-system.plumb` — extension inputs, outputs,
  dependencies, queries, operations, and effects.
- `docs/architecture/syntax-tree.plumb` — lossless tree and typed-view contract.
- `docs/architecture/editing.plumb` — owned syntax mutations and edit finalization.

User-facing material lives in `docs/guide/`; project direction lives in
`docs/project/`; completed design discussion lives in `docs/history/` and does
not override the references above.

## Current status

**Runnable 0.3 development line.** The repository contains the frozen core
syntax, a hand-written strict parser, typed extension/workspace layers, an LSP,
an exporter, notes tooling, and a lenient tree-sitter mirror. The parser already
produces a source-oriented tree plus `Vec<Diagnostic>`, but the remaining gaps in
`docs/project/roadmap.plumb` still block calling it a complete lossless parser release.
Normalized AST lowering remains deferred until an extension provides a concrete
consumer.

## Relationship to djot-tools

plumb is inspired by, and reuses architectural patterns from, the `djot-tools`
project, but it is a **separate project**:

- It does **not** use `jotdown` (or any existing markup parser). Its parser is
  hand-written so it can reject invalid input with precise diagnostics — see
  `docs/project/vision.plumb` for why tree-sitter and error-tolerant parsers cannot fill
  this role.
- It has its own versioning, its own release line, and (eventually) its own
  `tree-sitter-plumb` grammar repo.
- LSP scaffolding, byte-offset↔position conversion, and workspace-indexing
  patterns are **copied** from djot-tools' `djot-ls`, not shared as a
  dependency, to keep the two projects decoupled while they are both young.

## Core principles

1. **The hand-written strict parser is the single source of truth for *syntax*.**
   It is *reject-but-recover*: it reports every syntactic error it can (recovering
   at line/block boundaries) and always produces a lossless source-oriented tree.
   A document with syntactic errors is not valid input for authoritative semantic
   analysis or export. Recovered-tree editor queries such as completion and
   syntax-aware assists remain available. Strictness is **syntactic only**.
2. **The core is semantics-neutral; all meaning lives in extensions.**
   The first `plumb-core` phase produces one recovered lossless syntax tree per
   revision. Extensions initially consume typed recovered/valid views over that
   tree; a normalized AST is materialized only if a concrete consumer needs it
   (`{#id .class k=v}` remain opaque attributes). Everything semantic —
   metadata, link/anchor resolution, references, id generation, tasks, and
   lowering to HTML/pandoc — remains an
   **extension** (a language-neutral query, analysis, or operation over typed
   views and declared outputs; the exporter is itself an extension). Rust
   modules are one host implementation, not part of the extension definition.
   No registry, roles, or class-name validation exists in core.
   See `docs/project/vision.plumb` (the Pandoc/Docutils model).
3. **tree-sitter is intentionally lenient and ergonomics-only.** Its current
   grammar powers editor features (highlighting, text objects, folding,
   injections) and is *never* the strictness engine. Do **not** distort the
   language design to fit a CFG — strictness and good errors come from the hand
   parser regardless. Because core is semantics-neutral, core and tree-sitter
   cover the same (pure-syntax) scope, differing only in strict-vs-lenient.
4. **Export owns portability.** plumb is its own pandoc *reader*: the exporter
   (an extension) emits a `pandoc_types` JSON AST that is piped into `pandoc` as a
   *writer* only. This — not adopting a popular syntax — is the answer to "small
   ecosystem": output to PDF/HTML/etc. and a clean migration path out are always
   available.

## Intended architecture

A Cargo workspace (`crates/*`), mirroring djot-tools' deliberate split so the
semantics can be shared by more than one tool:

- **`plumb-core`** — semantics-neutral strict reader. Does no file I/O, works in
  byte offsets only. Hand-written lexer + line-oriented block scanner + strict
  inline parser, initially producing a lossless syntax tree and syntactic
  diagnostics. Ordinary marker and inline-kind tokens
  produce generic nodes carrying an opaque `{#id .class k=v}`; core does not
  reserve heading, list, quote, or semantic marker spellings. Zero-or-more quote
  runs strengthen inline verbatim delimiters; an attribute-only block opener
  switches to an indented verbatim payload. Contains
  **no** anchors, references, metadata, tasks, outline, or resolution logic.
- **extensions** — statically composed Rust implementations of the
  language-neutral extension contract, initially consuming typed views and
  adding semantics plus their own diagnostics and edit proposals:
  outline, anchors/references, target resolution, workspace, metadata, tasks.
  The official toolchain may implement and compose them as Rust modules, without
  making Rust part of the semantic contract. (These are djot-tools' `djot-core`
  analysis, relocated out of core.)
- **`plumb-workspace`** — document snapshots, last-valid extension outputs,
  dependency invalidation, cross-file indexes, and guarded workspace edits.
- **`plumb-edit`** — protocol-neutral owned syntax, valid-tree mutations,
  format-aware authoring finalization, and validated minimal token rewrites.
- **`plumb`** — the single user-facing binary. Its `lsp`, `export`, `fmt`, `note`,
  and `task` subcommands adapt the shared libraries to stdio, files, and editor
  protocols. The LSP implementation owns `lsp_types`, `async-lsp`, and UTF-16
  positions; export remains an extension that emits Pandoc JSON.
- **`tree-sitter-plumb`** (eventually a separate repo) — the existing lenient
  grammar for editor ergonomics.

The unified binary reuses `plumb-core` and the shared libraries; subcommand
dispatch does not move syntax or semantic behavior into the CLI layer.

## Tree-sitter generation workflow

`tree-sitter-plumb/grammar.js` and `tree-sitter-plumb/tree-sitter.json` are the
current tree-sitter sources of truth. The hand-written scanner, queries, future
bindings, and corpus tests are source files too and must be committed normally.

The following paths are generated by `tree-sitter generate` and intentionally
ignored:

- `tree-sitter-plumb/src/grammar.json`
- `tree-sitter-plumb/src/node-types.json`
- `tree-sitter-plumb/src/parser.c`
- `tree-sitter-plumb/src/tree_sitter/`

**Never edit, stage, force-add, or review these generated files as source.** To
change them, edit `grammar.js`, `tree-sitter.json`, or another hand-written
input, then regenerate inside the locked development environment:

```sh
cd tree-sitter-plumb
nix develop .#tree-sitter-plumb -c tree-sitter generate
```

After grammar changes, run generation before relevant tests so the local parser
matches the source. If a generated file appears in Git status, fix the ignore or
index state instead of committing it. Do not ignore the whole `src/` directory:
a future `src/scanner.c` would be hand-written and must remain trackable.

Generated parser sources and binary artifacts are release outputs, not repository
sources. Release automation must regenerate them from the target commit inside
the locked Nix environment, run the grammar tests, and package the required
generated files. Git consumers are not promised a directly buildable generated
parser; they should consume release artifacts or run the documented generation
step.

## Runtime gotcha inherited from async-lsp (applies to `plumb lsp`)

async-lsp's omni-trait style (`Router::from_language_server` +
`impl LanguageServer`) pre-registers a *breaking* handler for every standard LSP
notification. **Whenever you advertise a capability that makes editors send a
new notification** (`didSave`, `didChangeWatchedFiles`, etc.), you MUST add that
method to `impl LanguageServer` — even as a no-op `ControlFlow::Continue(())` —
or the server crashes in real editors. A catch-all does not cover these.
(`$/`-prefixed notifications, `exit`, and `initialized` are exempt.)

## Workspace graph demo workflow

When running `plumb graph` for development, browser checks, or a user-facing demo,
always pass an explicit dedicated test port with `--port`. Do not rely on the
random-port default, reuse a port occupied by another instance, or stop a graph
server that the user started independently. Track and stop only the process
started for the current test, by its PID or its exact test port.

Use the same explicit port in browser automation and health checks so restarting
the demo never requires rewriting test scripts and cannot collide with the user's
own `plumb graph` instance.

## Commit workflow

Start from `main` and create a short-lived topic branch (`feat/…`, `fix/…`)
before changing code; do not implement features or fixes directly on `main`.
Commit each coherent piece as it is completed; split non-trivial work into small
logical commits. Prefer this order: protocol-agnostic core data/model changes
first, core behavior with focused unit tests next, LSP/CLI integration and
black-box tests after the shared behavior exists, docs/roadmap updates last.
Before each commit, check `git status --short` and `git diff` so unrelated
changes are not included, and run the warning gate
(`cargo check --workspace --all-targets`) once there is a workspace. Use
conventional-style messages: `feat(core): …`, `fix(ls): …`, `docs: …`.

## Release workflow

Semantic-ish `0.x.y` while pre-1.0: release `0.x.(y+1)` when the release
contains only fixes; release `0.(x+1).y` when it includes features or behavior
changes. Until the Cargo workspace exists, `tree-sitter-plumb/tree-sitter.json`
is the version source. Once it exists, bump `[workspace.package].version` in
`Cargo.toml` and let Cargo update `Cargo.lock` (never edit it by hand). Commit the
release, tag it, then bump to the next `-dev` version.

## Docs language note

`AGENTS.md`/`CLAUDE.md` are in English by convention. The `docs/` design
material is in Chinese, matching the design conversation that produced it.
Design docs are being migrated to plumb for dogfooding. Keep `AGENTS.md` and
`CLAUDE.md` in Markdown because external agent tools consume them; use `.plumb`
for project documentation where the current syntax can represent it faithfully.

Guides describe only behavior available in the current release. Do not copy
implementation progress, TODOs, or "not yet implemented" lists into
`docs/guide/`: unfinished or exploratory work belongs in
`docs/project/roadmap.plumb`, directly actionable work belongs in
`docs/project/tasks.plumb`, and normative exclusions belong in the references.
`cargo test -p plumb-core --test project_documents` enforces this ownership and
also verifies all project `.plumb` documents plus bundled-skill plumb examples
with the strict parser.
