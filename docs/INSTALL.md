# Installing Evolvo

This document gets you from `git clone` to `cargo tauri dev` printing
`server listening` in under 15 minutes on macOS. Linux is supported with
a few extra steps noted inline. Windows is not tested in v1; contributions
welcome.

You need: **Rust**, **a C linker**, **Tauri CLI 2**, **Trunk**, the
**wasm32-unknown-unknown** target, and **at least one agent backend**. Each
section below is independently completable — you do not have to install all
four agent backends.

---

## 1. System prerequisites

### macOS (primary)

```bash
# Xcode Command Line Tools (provides clang, git, and macOS SDK headers)
xcode-select --install
```

Install [Homebrew](https://brew.sh) if you don't have it, then:

```bash
# Optional but recommended: prevents "too many open files" on large builds
brew install pkg-config
```

### Linux (secondary)

Debian / Ubuntu:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential \
  pkg-config \
  libssl-dev \
  libgtk-3-dev \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  libxcb1-dev \
  libxrandr-dev \
  libdbus-1-dev
```

Arch / Manjaro:

```bash
sudo pacman -S --needed \
  base-devel gtk3 webkit2gtk-4.1 \
  libayatana-appindicator librsvg \
  xdotool libxcb libxrandr
```

> **Note:** The `xcap` crate (used for window screenshots) requires
> `libxcb` and `libxrandr` on Linux. If you see linker errors mentioning
> `xcb`, install the packages above.

### Windows

Not tested in v1. Tauri 2 supports Windows in principle; if you get it
running, please open a PR adding the steps here.

---

## 2. Rust toolchain

Evolvo requires **Rust stable, edition 2021**. The repo is tested against
the current stable channel. There is no `rust-toolchain.toml` pinning a
specific version; if you hit a build error on a very old stable, upgrade
first.

```bash
# Install rustup if you don't have it
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Confirm (1.87+ recommended; tested on 1.91)
rustc --version
```

### Add the WASM target

The Leptos UI crate compiles to WebAssembly. The target must be present
before `cargo tauri dev` will succeed:

```bash
rustup target add wasm32-unknown-unknown
```

---

## 3. Tauri CLI

Tauri CLI orchestrates the Rust host build and spawns Trunk for the
frontend. Install it via Cargo:

```bash
cargo install tauri-cli --version "^2" --locked
# Verify
cargo tauri --version   # should print tauri-cli 2.x.y
```

> **Tested against:** `tauri-cli 2.9.4`. Any `2.x` release should work.
> If you see API mismatches, pin to `2.9.4` with
> `--version "=2.9.4"`.

---

## 4. Trunk

Trunk is the WASM bundler / dev server for the Leptos frontend. Tauri
invokes it automatically via `app/ui/scripts/trunk-dev.sh` — you do not
run Trunk manually.

```bash
cargo install trunk --locked
# Verify
trunk --version   # should print trunk 0.21.x
```

> **Tested against:** `trunk 0.21.14`. If you see `trunk: command not
> found` after install, check that `~/.cargo/bin` is on your `PATH`.

---

## 5. Agent backends

Evolvo ships with four agent backends. Install any combination; the
Feedback panel greys out chips whose binary is not on your `PATH`. You
can add more later — relaunch Evolvo after installing a new CLI and the
availability check re-runs automatically.

Evolvo probes the following directories in order (prepended before the
system `PATH`):

```
~/.cargo/bin
~/.local/bin
~/.bun/bin
~/.forge/bin
~/.volta/bin
~/.nvm/versions/node/current/bin
~/.npm-global/bin
/opt/homebrew/bin
/opt/homebrew/sbin
/usr/local/bin
/usr/local/sbin
<existing $PATH>
```

This means you do not have to restart Evolvo after installing a CLI to a
standard location — but if you install to a non-standard path, the probe
will miss it unless you add that directory to your system `PATH`.

### 5a. Claude Code (binary: `claude`)

Claude Code is the default backend and the most thoroughly tested with
Evolvo.

```bash
npm install -g @anthropic-ai/claude-code
# or with bun:
bun add -g @anthropic-ai/claude-code
```

**Auth:** Claude Code uses your Anthropic account subscription by default.
Run `claude` once to complete the OAuth login:

```bash
claude
# Follow the browser prompt, then Ctrl-C out
```

> **Important:** Evolvo deliberately removes `ANTHROPIC_API_KEY` and
> `ANTHROPIC_AUTH_TOKEN` from the environment before spawning `claude`.
> This forces the subscription login path — a zero-balance API key would
> shadow a valid subscription and cause every run to fail with a billing
> error. If you use the API key path intentionally, set the key in your
> shell's profile so Claude Code finds it through its own config, not via
> the environment variable.

**Context file:** `CLAUDE.md` at the worktree root.

### 5b. Codex CLI (binary: `codex`)

```bash
npm install -g @openai/codex
# or with bun:
bun add -g @openai/codex
```

**Auth:** Codex reads `OPENAI_API_KEY` from the environment. Add it to
your shell profile:

```bash
export OPENAI_API_KEY=sk-...
```

Evolvo spawns Codex with `codex exec --json
--dangerously-bypass-approvals-and-sandbox <prompt>`. The bypass flag is
required for non-interactive lineage runs; it mirrors the permission
envelope Claude's `--dangerously-skip-permissions` provides.

**Context file:** `AGENTS.md` at the worktree root (symlinked from
`CLAUDE.md` by the Evolvo runner).

### 5c. Gemini CLI (binary: `gemini`)

```bash
npm install -g @google/gemini-cli
# or with bun:
bun add -g @google/gemini-cli
```

**Auth:** On first run, Gemini CLI opens a browser to complete Google
OAuth. Run it once manually:

```bash
gemini
# Complete the browser login, then Ctrl-C
```

Evolvo spawns Gemini with `gemini --yolo --skip-trust -p <prompt>`.
`--skip-trust` suppresses the interactive workspace-trust prompt that
Gemini shows the first time it enters a directory; without it, every
lineage run blocks waiting for TTY input.

**Context files:** `GEMINI.md` and `AGENTS.md` (both symlinked from
`CLAUDE.md` by the runner).

### 5d. Forge (binary: `forge`)

Forge ([forgecode.dev](https://forgecode.dev)) is a provider-agnostic
agent CLI. Install it per the upstream README:

```bash
# Homebrew (macOS / Linux)
brew install forge-cli
# or via the install script:
curl -sSL https://forgecode.dev/install.sh | sh
```

**Auth / provider selection:** Forge uses `forge.yaml` at the worktree
root for provider/model selection. Run `forge provider login` once to
set up credentials for your preferred provider:

```bash
forge provider login
```

Evolvo spawns Forge with `forge -p <prompt>`.

**Context file:** `AGENTS.md` at the worktree root (symlinked from
`CLAUDE.md` by the runner).

---

## 6. First boot

```bash
git clone https://github.com/saimirbaci/Evolvo
cd Evolvo/app/src-tauri
cargo tauri dev
```

What to expect:

1. **Trunk builds the WASM frontend** — first build takes 1–3 minutes.
   Subsequent hot-reloads are seconds. You'll see:
   ```
   server listening at:
       http://127.0.0.1:1530/
   ```
2. **The host Rust binary compiles** — another 1–2 minutes on first build.
3. **The Evolvo window opens** — a blank canvas with the app bar and the
   Feedback FAB (bottom right).

The workspace is created automatically at `~/.evolvo/evolvo_workspace/`
on first run.

**Using a project-specific workspace:**

```bash
EVOLVO_WORKSPACE_ROOT=/path/to/my-project/workspace cargo tauri dev
```

**Iteration port note:** Each lineage iteration runs its app on its own
port (`1530 + N`). Iteration 3 serves at `http://127.0.0.1:1533`. The
Lineage tab shows the port for each job.

---

## 7. Troubleshooting

### "Failed to load resource: Could not connect to the server"

A stale `trunk serve` process from a previous session is holding port
1530. Check:

```bash
lsof -iTCP -sTCP:LISTEN -P -n | grep trunk
```

Kill the stale PID (`kill <PID>`), then restart `cargo tauri dev`.

### "error[E0463]: can't find crate for `std`" or similar wasm32 errors

The `wasm32-unknown-unknown` target is missing:

```bash
rustup target add wasm32-unknown-unknown
```

### Agent chip is greyed out in the Feedback panel

The agent's binary is not on the enriched PATH (see the probe list in
section 5). Verify:

```bash
which claude   # or codex, gemini, forge
```

If the binary exists but Evolvo doesn't see it, its install location is
not in the enriched PATH list. Add the directory to your shell profile's
`PATH` and relaunch Evolvo.

### `cargo tauri dev` fails with "tauri-cli: command not found"

`~/.cargo/bin` is not on your `PATH`:

```bash
source "$HOME/.cargo/env"
# or add to your shell profile:
export PATH="$HOME/.cargo/bin:$PATH"
```

### Trunk build fails with "trunk: command not found" inside Tauri

Same cause as above — Tauri spawns `trunk-dev.sh` as a subprocess and
inherits `PATH` from the launch environment, which may be minimal on
macOS when launched from Finder. Fix:

```bash
# Confirm trunk is in ~/.cargo/bin
ls ~/.cargo/bin/trunk
# Then ensure ~/.cargo/bin is in your login shell PATH, not just interactive:
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.profile
```

Re-open your terminal (or `source ~/.profile`) and retry `cargo tauri dev`.
