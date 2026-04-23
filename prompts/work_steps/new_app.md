4. **This is a `NewApp` feedback.** The user is asking you to build a NEW APP from scratch on top of the Evolvo shell. Build the new app in `app/ui/src/app.rs` (and any new modules you add alongside it) mounted inside `<Shell>` — the NewApp content area is yours to rewrite freely. Do NOT touch `app/ui/src/shell.rs`: the shell (app bar, Lineage nav + page, Star Us link, Feedback FAB, Canvas overlay) is permanent and is what guarantees the four product invariants (Feedback Overlay, per-page Canvas overlay, Inbox, Lineage pipeline) survive the rewrite. Everything inside the shell — the old app's domain, pages, data model, even the choice of Leptos if you replace the whole UI stack — is up for replacement, as long as the equivalent of `<Shell>` keeps wrapping the new content and keeps those invariants reachable from every page.

   **Ship a real end-to-end vertical slice, not a stub UI.** "NewApp" means a working app on iteration 1 — a frontend wired to a real backend, not placeholder screens with `TODO` handlers. Concretely, this iteration MUST include all of:

   - **Backend (Rust / Tauri host)**: define the domain types in `app/src-tauri/src/types.rs` (or new modules alongside it), add persistent storage in `app/src-tauri/src/store.rs` (or a new module) backed by real JSON files under the Evolvo workspace root — no in-memory-only stores, no hard-coded fixtures. Follow the existing `Store` patterns (sanitised filenames, `StoreError`, tempdir-based tests).
   - **Tauri commands**: every user action the UI offers must be backed by a `#[tauri::command]` in `app/src-tauri/src/commands.rs`, registered in the `invoke_handler` in `app/src-tauri/src/lib.rs`, and mirrored by a typed wrapper in `app/ui/src/interop.rs`. No UI button may be a no-op or call a mocked function.
   - **Frontend**: real Leptos views that call the interop wrappers, render server state, and reflect writes back from disk (not local-only `RwSignal` fakes that disappear on reload).
   - **Host-side tests**: add `#[cfg(test)]` unit tests for the new store and command logic using `tempfile::tempdir()`. `cargo test -p evolvo_desktop` must pass with the new tests included.
   - **End-to-end verification**: actually run `cargo tauri dev` on the iteration's port, exercise the primary CRUD flow in the live app, and confirm data persists across a page reload. If you can't get the full stack running, surface the blocker in the summary — don't ship a green `cargo check` as "done".

   A frontend-only prototype with stubbed handlers is an incomplete iteration and will be rejected. If the scope the user described is larger than one iteration can cover end-to-end, pick the smallest vertical slice (one domain entity, create + list + persist) and implement it fully rather than sketching every screen.

   **Reach for `templates/integrations/` before hand-rolling.** The repo ships drop-in scaffolds so common service integrations can land on iteration 1 instead of getting pushed to "later". Each template has a `README.md` with wiring steps, a `host.rs` with Tauri commands, a `ui.rs` with Leptos helpers, and a `deps.toml` listing Cargo deps to merge. Available templates:

   - `templates/integrations/openrouter/` — multi-model LLM chat (GPT-4, Claude, Gemini, Llama, …) with persistent sessions that include the app name as a system prompt. If the NewApp has *any* AI / assistant / "help me draft" surface, copy this template and mount `<ChatPanel app_name="<your app>"/>`.
   - `templates/integrations/google_oauth/` — Google Sign-In via OAuth2 PKCE + loopback. Unlocks Gmail / Drive / Calendar. Ships a placeholder `GOOGLE_CLIENT_ID` the user must swap for their own.
   - `templates/integrations/http_json/` — generic REST helper for arbitrary JSON APIs, with keyring-backed bearer tokens.
   - `templates/integrations/sqlite/` — `rusqlite` + a migration runner when JSON-per-entity isn't enough.
   - `templates/integrations/webhook_ingest/` — loopback HTTP server for receiving third-party callbacks.
   - `templates/integrations/file_picker/` — native open/save dialogs + CSV / XLSX import.

   Using a template is mandatory when it matches the NewApp's shape: copying `openrouter/` is strictly better than hand-writing a half-broken `fetch('https://api.openai.com/...')` in the UI. Read `templates/integrations/README.md` first for the full index and the copy-merge-register procedure.
