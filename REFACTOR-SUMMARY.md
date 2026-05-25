# Native Azure SDK Refactor — Honest Status

**Branch**: `refactor/native-azure-sdk` (8 commits on top of v0.19.4 base, not pushed)

**Objective**: Remove all `az` CLI shell-outs from the operator-side Rust binary. Replace 17 `az` call sites (15 in `crates/azcluster-cli/src/main.rs`, 2 in `crates/azcluster-cli/src/timings.rs`) with native ARM REST + native OAuth2. Add Bastion tunneling so `azcluster ssh` works when the login VM has no public IP.

**Approach**: Vertical slicing — one call site at a time, end-to-end (login → real cached token → ARM call → wired into existing CLI command → live-validated → committed).

---

## What actually works on this branch

- `src/auth/cache.rs` — token cache at `~/.azure/azcli_tokens.json` (mode 0o600). JSON round-trip + expiry detection. 3 unit tests.
- `src/auth/mod.rs` — OAuth2 endpoint helpers + `list_subscriptions()` blocking ARM call (the bootstrap call used by `azcluster login`).
- `src/auth/interactive.rs` — **REAL** browser OAuth2 authorization-code flow with PKCE (S256). Binds a localhost TCP listener, builds the authorize URL, opens the browser via `webbrowser` crate, accepts the redirect, parses `code`/`state`, validates `state` (CSRF), exchanges code for tokens at `login.microsoftonline.com/{tenant}/oauth2/v2.0/token`. 2 unit tests on the PKCE generator.
- `src/auth/device_code.rs` — **REAL** OAuth2 device-code flow for headless / ssh sessions. Polls token endpoint with `authorization_pending` / `slow_down` handling.
- `src/auth/token_provider.rs`:
  - `TokenProvider::get_token()`: returns cached token if valid, else refreshes via `refresh_token` grant. On no cached credentials, bails with "Run: azcluster login".
  - `run_interactive_login()` / `run_device_code_login()`: drive the new flows, persist account to cache under a `_pending:{tenant}` key.
  - `bind_subscription(tenant, sub_id)`: re-keys the pending entry under the chosen subscription id after `list_subscriptions()` resolves.
  - `extract_username()`: JWT payload decoder pulls `upn` / `preferred_username` / `unique_name` / `email` claim. 3 unit tests.
  - **`get_token_from_az_cli()` REMOVED.** No more shell-out to `az`.
- `src/main.rs`:
  - `CliCommand::Login(LoginArgs)` with `--device-code`, `--tenant <id>`, `--subscription <id>` flags.
  - `login()` function: runs the chosen flow → lists subscriptions → picks first (or `--subscription`) → binds → prints account/subscription summary. If multiple subs are visible, prints them all with `--subscription` hint.
  - `get_access_token()` + `current_subscription_id()` helpers read straight from the cache (no `az` shell-out). Currently `#[allow(dead_code)]` — will become live when slice 1 wires them up.

## What does NOT work yet

- **Live-validation pending.** `azcluster login` builds and the CLI surface is correct (`--help` prints all three flags), but the browser OAuth round-trip has never actually been run end-to-end. The user must click through a real Azure login to confirm the cache populates with `access_token` + `refresh_token` and `azcluster login` exits 0 with sensible output.
- Any actual `az` call site replacement in the existing commands (`deploy`, `delete`, `scale`, etc.) — zero done. Still call `az_json()` / `ensure_az()`.
- Bastion tunneling — protocol understood, no working code.
- ARM client / LRO / config modules (`src/arm/`): now explicitly `#![allow(dead_code, unused_imports)]` because nothing calls them yet. They'll either get used (rewired in slices 1-8) or deleted.
- Bastion client module (`src/bastion/client.rs`): same — `#![allow(dead_code)]`.

## Dependencies added on this branch

Workspace `Cargo.toml`: `reqwest 0.11` (json + blocking), `chrono 0.4` (serde), `uuid 1` (v4/serde), `webbrowser 1`, `tokio-tungstenite 0.21`, `toml 0.8`, `base64 0.22`, `sha2 0.10`, `url 2`, `rand 0.8`. `Cargo.lock` grew ~1300 lines vs main.

## Verification status

- `cargo fmt --all` — clean
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `cargo test --workspace` — **81 passed** (was 51 on the rogue branch + 30 prior), 0 failed
- `azcluster login --help` — prints the three new flags correctly
- **Live validation** — NOT YET DONE. Awaits the user.

## 17 `az` call sites to replace (vertical-slice order)

1. `az account show --query id` → reuse cached `subscription_id` from `azcluster login`
2. `az account show --query tenantId` → reuse cached `tenant_id` from `azcluster login`
3. `az ad signed-in-user show` + `az ad sp show` → Microsoft Graph `GET /me` + `/servicePrincipals`
4. `az group create` / `az group delete --no-wait` → ARM RG PUT / DELETE
5. `az deployment sub create` + polling → ARM sub-deployment PUT + LRO
6. `az deployment sub show --query properties.outputs` → ARM deployment GET
7. `az deployment operation {sub,group} list` (timings.rs ×2) → ARM operations GET
8. `az vmss scale` / `az vmss show --query sku.capacity` → ARM Compute VMSS PATCH/GET
9. `az grafana dashboard create` / `az grafana show` → Grafana HTTP API + ARM GET

After all land: delete `az_json()` / `ensure_az()` helpers; remove `az CLI logged in` from README prerequisites; bump version + release.

## Next action

Operator must run `cargo run --release -p azcluster-cli -- login` and click through the browser. On success, `cat ~/.azure/azcli_tokens.json | jq` should show `access_token`, `refresh_token`, `expires_at`, `username`, `subscription_id`, `tenant_id`. After that, vertical slice 1 unblocks.
