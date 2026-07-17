# App-shape scaffold contract

Every scaffold under `templates/app_shapes/<name>/` MUST satisfy this
contract, end-to-end. The contract is what lets the planner pick a
scaffold without reading its source, and what lets the implementer apply
it mechanically.

## Required files

```
templates/app_shapes/<name>/
├── manifest.toml          # the machine-readable summary (this schema)
├── SKILL.md               # the human + agent-readable contract
├── host.rs                # Tauri commands + <Name>Store trait + default impl
├── ui_app.rs              # the NewApp content area (mounts inside <Shell>)
├── ui_modules/            # OPTIONAL: additional Leptos modules
│   └── *.rs
├── styles.css             # CSS to append to app/ui/styles/app.css — must
│                          #   use var(--...) tokens from styles/tokens.css,
│                          #   no raw hex/rgb/hsl (see design lint below)
└── deps.toml              # Cargo dependencies to merge
```

## `manifest.toml` schema

```toml
[template]
name = "markdown_editor"
description = "Two-pane markdown editor: list + source + preview."
tags = ["text-editing", "single-document", "writing"]

# Where the implementer copies the scaffold's files. All paths are
# relative to the worktree root unless stated otherwise. The implementer
# MUST honour these; the validator checks the resulting tree.
[layout]
host_module = "app/src-tauri/src/app_shapes/markdown_editor.rs"
register_in = "app/src-tauri/src/lib.rs"          # invoke_handler! list
ui_app      = "app/ui/src/app_shape_markdown_editor.rs"
ui_modules  = [                                    # additional UI files
  "app/ui/src/markdown_editor_doc_list.rs",
  "app/ui/src/markdown_editor_preview.rs",
]
styles_file = "app/ui/styles/app.css"              # appended, not replaced —
                                                    # the iteration-owned layer;
                                                    # see styles/tokens.css

# The scaffold's data interface. The planner uses this to know what the
# scaffold persists; the implementer renders these as the entities and
# Tauri commands the BackendPlan stage records in plan.json.
#
# `kind` is one of:
#   "json_store"  — default: writes JSON files via the existing host Store
#                   under <workspace>/<storage_path>/{id}.json
#   "sqlite"      — single sqlite table; requires the sqlite integration
#   "rest"        — proxies to a REST endpoint via http_json
#   "drive"       — Google Drive file; requires google_oauth + drive scope
#
# `default` MUST always be the kind whose backend ships in the scaffold's
# host.rs (no extra integration). Currently always "json_store".
[data_interface]
default     = "json_store"
storage_path = "markdown_editor/documents"   # under workspace root

  [data_interface.entity]
  name   = "Document"
  fields = [
    { name = "id",       ty = "String", required = true  },
    { name = "title",    ty = "String", required = true  },
    { name = "body",     ty = "String", required = true  },
    { name = "updatedAt", ty = "u64",    required = true  },
  ]

  [data_interface.commands]
  list   = "list_documents"     # () -> Vec<Document>
  load   = "load_document"      # EntityIdPayload -> Document
  save   = "save_document"      # SaveDocumentPayload -> Document
  delete = "delete_document"    # EntityIdPayload -> bool

# OPTIONAL: integration templates that can replace the scaffold's default
# backend. The planner records the chosen binding in
# plan.templates.backendBindings; the implementer reads the integration
# template's `seam_<scaffold-name>.rs` (when present) to swap in the
# adapter. Each entry is a one-line "what this seam does" description.
[backend_seams]
sqlite       = "Store documents in a sqlite `documents` table; survives JSON corruption better at scale."
http_json    = "Proxy CRUD to a remote REST API; useful when documents must sync across devices."
google_oauth = "Persist documents as files in the user's Google Drive (requires google_oauth + drive scope)."

# Markers the agent must replace before this scaffold's iteration is
# accepted. The validator (when implemented) greps the diff for any
# remaining `// FILL: …` after BackendImpl + FrontendImpl finish.
[fill_markers]
required = [
  "FILL: domain-specific actions",       # e.g. "publish to blog"
  "FILL: extra metadata fields",         # e.g. tags, slug, archived
]
optional = [
  "FILL: keyboard shortcuts",
]
```

## `SKILL.md` required sections

In this exact order:

1. **`# <Scaffold name>`** — h1 with the scaffold name.
2. **`## What you get`** — one paragraph + a bullet list of the moving
   parts (entities, Tauri commands, UI components, default backend).
3. **`## Mount points`** — repeats `[layout]` from the manifest in prose
   so the agent doesn't have to re-parse TOML.
4. **`## Data interface`** — the entity, the four CRUD commands, and the
   default backend's storage path. State the trait name (e.g.
   `DocumentStore`) and where its default impl lives in `host.rs`.
5. **`## Backend seams`** — every entry in the manifest's
   `[backend_seams]`, with one paragraph each describing what the
   integration's `seam_<name>.rs` is expected to provide and how to wire
   it. If a seam isn't yet implemented in the integration template, say
   so in plain prose ("not yet shipped — would require X").
6. **`## FILL markers`** — every `// FILL: …` marker present in the
   scaffold's code, with a one-line description and a one-line example
   of what a *filled* version might look like.
7. **`## Mounting inside the Shell`** — one paragraph reminding the
   implementer that `App` in `app/ui/src/app.rs` must still wrap the
   scaffold's root component in `<Shell>` and that the four product
   invariants (Lineage, Feedback Overlay, per-page Canvas, single FAB)
   are non-negotiable.
8. **`## Verify`** — the exact `cargo check` / `cargo test` commands the
   implementer must run before declaring the iteration done.

## Coding conventions inside the scaffold

- The scaffold's code lives outside the Rust workspace (the `templates/`
  directory is not a Cargo crate). Authors should still write
  syntactically valid Rust so editors and `rust-analyzer` don't choke.
- Reference identifiers that already exist in the host crate (`AppState`,
  `Store`, `current_time_unix_ms`, `EntityIdPayload`) so the scaffold
  drops in without inventing new infrastructure.
- Use `// FILL: …` comments for any blank that requires iteration-specific
  code. Match the markers exactly to the manifest's `[fill_markers]`.
- The `*Store` trait in `host.rs` MUST be object-safe (no generics on
  methods, no `Self`-returning methods) so a backend integration can
  replace the default impl by handing back a `Box<dyn DocumentStore>`.
- The scaffold's UI files MUST NOT touch `shell.rs`, `canvas.rs`,
  `feedback_panel.rs`, `voice.rs`, or `toolbar.rs`. Those belong to the
  invariant chrome.
- The scaffold's `styles.css` MUST use `var(--...)` tokens from
  `app/ui/styles/tokens.css` for every color, font size, and shadow — no
  raw hex/rgb/hsl literals, and `font-size:`/`box-shadow:` declarations
  must reference `var(--text-*)` / `var(--shadow-*)`. `validate_frontend_impl`
  lints the merged `app/ui/styles/app.css` for exactly these rules; a
  scaffold that fails the lint blocks every iteration that picks it.

## Forward compatibility

`manifest.toml` is parsed leniently — unknown top-level tables are
ignored, and `[backend_seams]` may be absent (a scaffold with no seams
is fine, it just means no integration can swap its backend yet). New
fields can be added without breaking older planners; the schema rev is
tracked in this file's git history rather than in a `schema_version` key.
