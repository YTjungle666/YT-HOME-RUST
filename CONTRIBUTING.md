# Contributing to YT HOME RUST

This repository now ships a Rust backend workspace and a Vue 3 frontend. Contributions are expected to keep the current UI and functional behavior stable while improving structure, safety, and maintainability.

## Prerequisites

- Rust `1.88.0` with `rustfmt` and `clippy`
- Node.js `24`
- `npm`
- Docker or Podman if you want to validate container builds

## Local Setup

```bash
git clone https://github.com/YTjungle666/YT-HOME-RUST
cd YT-HOME-RUST
```

Install frontend dependencies and build assets:

```bash
cd frontend
npm ci
npm run build
cd ..
```

Build the Rust backend and fetch the matching `sing-box` runtime:

```bash
cargo build --release -p app
sh ./scripts/fetch-sing-box.sh linux amd64 ./target/release 1.13.5
```

For a local debug run:

```bash
SUI_SING_BOX_BIN=./target/release/sing-box \
SUI_DB_FOLDER=db \
SUI_WEB_DIR=frontend/dist \
cargo run -p app
```

If you prefer the bundled helper:

```bash
./runSUI.sh
```

## Quality Gates

All pull requests are expected to pass the same gates enforced in CI:

```bash
cd frontend
npm ci
npm run lint -- --max-warnings=0
npm run typecheck
npm run build
npm audit --audit-level=high
cd ..

cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo audit
cargo deny check
```

## Project Structure

- `crates/app`: process startup and runtime wiring
- `crates/http-api`: Axum routes and HTTP DTOs
- `crates/domain-*`: business domains
- `crates/infra-*`: persistence, scheduling, observability
- `frontend/`: Vue 3 + TypeScript + Vuetify UI
- `scripts/`: build and runtime helper scripts

## Contribution Rules

- Keep user-visible behavior stable unless the change is explicitly discussed first.
- Preserve QR code, subscription link, and old-client compatibility behavior.
- Remove dead code and unused files as part of the change.
- Do not introduce warnings into lint, typecheck, clippy, or build output.
- Prefer small, reviewable commits and clear pull request descriptions.

## Pull Requests

When opening a PR, include:

1. What changed.
2. Why it changed.
3. Which validation commands were executed.
4. Any boundary or compatibility considerations.

If your change touches runtime behavior, configuration format, login/session behavior, or subscription output, call that out explicitly in the PR description.
