# AGENTS.md

ezpdf — a Tauri 2 desktop **real-time PDF translation reader** for Windows, built with Vue 3 + TypeScript + Vite. See `PLAN.md` for the approved architecture and phased plan (OCR via an in-repo Python FastAPI service, LLM translation, Rust-side scheduling; both reader panes are self-drawn with `pdfjs-dist` — do not reintroduce pdf-vue3). Current state: UI skeleton (shell/sidebar/toolbar/dual-pane reader/status strip/settings) wired to real repo IPC (`.ezrepo` flat index via `gettree_from_config`; `load_book` for opening books); no mock data — everything runs on real repos.

## Commands

Package manager is **pnpm** (no lint or test scripts exist yet).

- `pnpm dev` — Vite dev server only (frontend in browser)
- `pnpm build` — typecheck + build (`vue-tsc --noEmit && vite build`); this is the typecheck command
- `pnpm tauri dev` — full desktop app dev (starts Vite automatically via `beforeDevCommand`)
- `pnpm tauri build` — release build/bundle (Rust release profile uses LTO, strip, panic=abort)
- Rust checks: run `cargo check` / `cargo build` inside `src-tauri/`

## Architecture

Two halves, communicating only through Tauri's IPC:

- `src/` — Vue 3 frontend (`<script setup>` SFCs). Entry `src/main.ts` (installs Pinia); app shell is `src/components/shell/AppShell.vue`. UI state lives in Pinia stores under `src/stores/` (library/reader/parse/settings); domain types mirroring the Rust serde structs live in `src/types/domain.ts`.
- `src-tauri/` — Rust backend. Commands live in `src-tauri/src/lib.rs` (registered in `generate_handler![]`), invoked from the frontend with `invoke("command_name", {...})` from `@tauri-apps/api/core`. Command args are camelCase in JS and snake_case in Rust.
- `src-tauri/capabilities/default.json` — permission allowlist. New Tauri plugin APIs won't work until their permissions are added here.
- `src-tauri/gen/` — generated schemas; never hand-edit.

## Gotchas

- **Port 1420 is strict** (`vite.config.ts`): Vite fails if it's taken, and `tauri.conf.json` points `devUrl` at it. HMR uses 1421 when `TAURI_DEV_HOST` is set.
- **`testfiles/` is gitignored** and reserved for manual-test PDFs (text+table+formula+image). It is no longer referenced by code; the reader renders books from the library store (real repo data via IPC only — mock data was removed).
- Vite is configured to not watch `**/src-tauri/**` — Rust changes require restarting `pnpm tauri dev`.
- Windows dev machine: the Rust lib is named `ezpdf_lib` (with `_lib` suffix) to avoid a Windows bin/lib name conflict — keep the suffix.
- TypeScript is strict with `noUnusedLocals`/`noUnusedParameters`; unused imports will fail `pnpm build`.
- No CSP is set (`"csp": null` in `tauri.conf.json`) — fine for dev, revisit before shipping.

## User Harness
- After smoke testing, should kill port 1420 task with powershell to ensure user can test.
