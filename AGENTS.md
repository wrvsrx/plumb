# AGENTS.md

This file provides guidance to AI coding agents working in this repository.

`AGENTS.md` is the canonical instruction file. Tool-specific files, such as
`CLAUDE.md`, should point agents here rather than duplicating these
instructions.

## What this is

**plumb** (working name — see `docs/vision.plumb`) is a
**strict markup language** and its tooling, built for personal use. Where
[Djot](https://djot.net) and Markdown are deliberately error-tolerant, plumb is
deliberately strict: malformed syntax is a hard parse error rather than a silent
fallback to literal text. The current design direction is "special spellings are
always special" rather than "every punctuation character is globally special."

Start with the design docs, in this order:

- `docs/requirements.plumb` — the current source of truth for goals, non-goals,
  design principles, and MVP requirements.
- `docs/vision.plumb` — why the language exists, its core philosophy, and the
  ecosystem strategy.
- `docs/spec.plumb` — the finalized block-level structure and lossless syntax-tree
  contract.
- `docs/inline.plumb` — the finalized MVP inline syntax. Read it after the
  block-level design in `docs/spec.plumb`.
- `docs/syntax-open-questions.plumb` — the syntax decision record and the
  deferred core AST questions that no longer block parser implementation.
- `docs/extensions.plumb` — the language-neutral contract for adding semantics
  outside core, including inputs, outputs, diagnostics, dependencies, and I/O
  boundaries.
- `docs/features.plumb` — the tool roadmap derived from the requirements.

## Current status

**Greenfield design reset.** This repository currently contains design docs and a
lenient tree-sitter mirror, but no strict parser yet. The MVP syntax is frozen in
`docs/requirements.plumb`, `docs/spec.plumb`, and `docs/inline.plumb`. The first
`plumb-core` implementation target is a hand-written strict parser producing a
lossless syntax tree plus `Vec<Diagnostic>`. Normalized AST lowering is deferred
until an extension provides a concrete consumer.

## Relationship to djot-tools

plumb is inspired by, and reuses architectural patterns from, the `djot-tools`
project, but it is a **separate project**:

- It does **not** use `jotdown` (or any existing markup parser). Its parser is
  hand-written so it can reject invalid input with precise diagnostics — see
  `docs/vision.plumb` for why tree-sitter and error-tolerant parsers cannot fill
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
   See `docs/vision.plumb` (the Pandoc/Docutils model).
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
  reserve heading, list, quote, or semantic marker spellings. Quote runs remain a
  syntax-level special case because they switch the payload to raw mode. Contains
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
- **`plumb-ls`** — everything LSP (`lsp_types`, `async-lsp`, UTF-16 positions);
  wires core + extensions, hosts request-scoped queries, and merges their
  diagnostics.
- **`plumb-export`** — itself an extension: valid typed view or exporter-specific
  normalized model → `pandoc_types` JSON → `pandoc` (writer only).
- **`plumb-notes`** — CEL query/edit CLI over directories of plumb documents.
- **`tree-sitter-plumb`** (eventually a separate repo) — the existing lenient
  grammar for editor ergonomics.

All binaries reuse `plumb-core` + the extensions without pulling in each other's
types.

## Tree-sitter generation workflow

`tree-sitter-plumb/grammar.js` and `tree-sitter-plumb/tree-sitter.json` are the
current tree-sitter sources of truth. Future hand-written scanners, queries,
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

## Runtime gotcha inherited from async-lsp (applies once `plumb-ls` exists)

async-lsp's omni-trait style (`Router::from_language_server` +
`impl LanguageServer`) pre-registers a *breaking* handler for every standard LSP
notification. **Whenever you advertise a capability that makes editors send a
new notification** (`didSave`, `didChangeWatchedFiles`, etc.), you MUST add that
method to `impl LanguageServer` — even as a no-op `ControlFlow::Continue(())` —
or the server crashes in real editors. A catch-all does not cover these.
(`$/`-prefixed notifications, `exit`, and `initialized` are exempt.)

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
