# Rust conventions — Evolvo

Applies to both `app/src-tauri` (host, native target) and `app/ui` (WASM target).

## Edition / toolchain

- Workspace edition: **2021** (see `Cargo.toml`). Don't bump to 2024 in a single crate — workspace-wide only.
- MSRV: whatever stable Rust ships when Tauri 2 + Leptos 0.8 supports it. Don't pin an MSRV in code without updating CI.
- Targets: host crate = native (`aarch64-apple-darwin` on dev), UI crate = `wasm32-unknown-unknown`.

## Error handling

- Library code returns `Result<T, StoreError>` (or a crate-local error enum). NEVER `.unwrap()` / `.expect()` outside of tests and `main()` startup.
- Tauri commands return `Result<T, String>` — stringify errors at the boundary (see `commands::store_error`). Do not leak `anyhow::Error` or `io::Error` types across IPC.
- Never return a `StoreError::Io` with a raw path the user didn't originate — leak the minimum needed to debug.

## Serde & wire types

- Every type crossing the Tauri boundary is `#[derive(Serialize, Deserialize)]` with `#[serde(rename_all = "camelCase")]`.
- Be **forgiving on decode** by default (tolerate unknown fields). Only add `deny_unknown_fields` to types that are a trust/security boundary, and document why.
- Use `#[serde(default)]` on collection / option fields — forward compatibility matters; the app may read JSON written by older versions.
- Money / precision: there's no money here today, but if there ever is: `rust_decimal::Decimal`, never `f64`.

## Filesystem

- All disk writes go through `Store` in `store.rs`. Do not spread `fs::write` calls across the crate.
- All user-supplied filenames go through `sanitise_filename`. Path traversal is the only realistic local attack surface here — take it seriously.
- Workspace root comes from `default_workspace_root()` (honours `NOIDE_WORKSPACE_ROOT`). Do not hardcode `~/.evolvo` anywhere else.

## Tests

- Every filesystem test uses `tempfile::tempdir()` — never write to the real workspace from a test.
- Unit tests co-locate with the module (`#[cfg(test)] mod tests`).
- Integration tests for Tauri commands test the underlying logic (store + engine), not the `#[tauri::command]` shim (see the pattern in `commands.rs::tests::submit_feedback_stores_record_and_spawns_job`).
- Run `cargo test -p noide_desktop` before claiming a host-side change is done.

## Clippy / formatting

- `cargo clippy -p noide_desktop -- -D warnings` must pass.
- `cargo fmt --all` — default rustfmt, no custom config. Don't argue with the formatter.

## Dependencies

- The release profile optimises for size (`opt-level = "z"`, `lto`, `strip`). New host deps are cheap; new **UI** deps are not — every WASM byte ships to every user. Justify UI additions.
- Prefer `chrono` features already enabled (`default-features = false, features = ["clock"]`) over pulling `time` in parallel.
- `base64 = "0.22"` API: use `STANDARD.encode` / `STANDARD.decode` (the `Engine` trait). No 0.21-era free functions.
