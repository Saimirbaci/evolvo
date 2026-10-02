# claude_cli — real model calls through the local Claude Code CLI

Gives the NewApp's **own logic** a model: agents that actually do their
work, assistants that reply, summaries, classification, generated
content. Calls run through the user's installed `claude` CLI, so there is
**no API key to manage** — it uses whatever account `claude` is logged
into. Prefer this over `openrouter` unless the user asked for a choice of
providers.

Use it for every `backend.behaviors[]` entry with `"engine": "llm"`.

## What you get

`host.rs` (copy to `app/src-tauri/src/integrations/claude_cli.rs`):

- `LlmEngine` trait — `fn complete(&self, &LlmRequest) -> Result<LlmReply, String>`.
- `ClaudeCli` — production engine (`claude -p --tools "" --output-format json`).
  The model gets no tools: it reads your prompt and answers, nothing else.
- `ScriptedEngine` — deterministic test engine (`always`, `sequence`) that
  records requests. **Unit tests must use this**, never the real CLI.
- `LlmRequest::new(system, prompt).with_model("sonnet").with_schema(json!(…))`
  — `with_schema` forces JSON output, parsed into `LlmReply::structured`.
- `claude_cli_status` Tauri command — lets the UI show "install / log in to
  Claude Code" before the first call.

## Wire-up

1. Copy `host.rs` → `app/src-tauri/src/integrations/claude_cli.rs`; add
   `pub mod claude_cli;` to `integrations/mod.rs` (create it, and
   `pub mod integrations;` in `lib.rs`, if missing).
2. Register `integrations::claude_cli::claude_cli_status` in
   `invoke_handler!` and add a typed wrapper in `app/ui/src/interop.rs`.
3. Put the engine in app state so behaviours and tests share one seam:

   ```rust
   pub struct AppState { /* … */ pub llm: Arc<dyn LlmEngine> }
   // main.rs / lib.rs:  llm: Arc::new(ClaudeCli)
   // tests:             llm: Arc::new(ScriptedEngine::always("Done: tests pass"))
   ```

4. Call it from the behaviour on a **background thread** — a model call
   takes seconds. Persist the "working" state first, the result (or the
   error) after, then notify the UI:

   ```rust
   pub fn run_agent_turn(store: Store, llm: Arc<dyn LlmEngine>, app: AppHandle, agent_id: String) {
       std::thread::spawn(move || {
           let _ = set_agent_status(&store, &agent_id, AgentStatus::Active);
           let req = LlmRequest::new(agent_system_prompt(&agent), thread_prompt(&messages))
               .with_model("sonnet");
           match llm.complete(&req) {
               Ok(reply) => { store.save_agent_message(&reply_message(&agent, &reply.text)); }
               Err(err)  => { mark_agent_failed(&store, &agent_id, &err); }
           }
           let _ = set_agent_status(&store, &agent_id, AgentStatus::Idle);
           let _ = app.emit("app://changed", ());
       });
   }
   ```

   Keep the body in a `_impl` that takes `&dyn LlmEngine` so tests drive
   it synchronously with `ScriptedEngine`.

## Rules

- Never fabricate model output (canned strings, random picks, sleeps) —
  that is a simulated behaviour and fails the pipeline's mockup checks.
- Surface failures: a missing CLI or logged-out account comes back as
  `Err(String)`; store it on the entity and show it in the UI.
- Keep prompts small and specific; pass the entity's real data (role,
  task title, recent messages) rather than the whole database.
- Model choice: `sonnet` for most behaviours; `opus` for long-form
  generation where quality matters; `haiku` only for trivial labels.
