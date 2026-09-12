# Orchard Cluster Console

The repo contains a static web UI in [`web/`](web/README.md) and the macOS
[`ciderd`](crates/ciderd/README.md) telemetry crate. The crate
exports a Rust library and a foreground daemon binary. The web UI still uses
its fixture data; heartbeat ingestion and dashboard integration are separate work.

Collect a local snapshot without sending telemetry:

```sh
cargo run --bin ciderd -- snapshot --seconds 3
```

Build and check the Rust workspace:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Development environment

Install [Nix](https://nix.dev/install-nix) and enable `nix-command` and `flakes`.
For installations where these are not already enabled, add this line to
`~/.config/nix/nix.conf`:

```ini
extra-experimental-features = nix-command flakes
```

From the repo root:

```sh
nix develop
```

The flake targets Apple Silicon macOS (`aarch64-darwin`). It supplies Rust, Cargo,
rustfmt, Clippy, rust-analyzer, standard library sources, a C toolchain, pkg-config,
OpenSSL, Python 3, and nixfmt. Package versions, including the Rust toolchain, are
pinned together by `flake.lock`.

To preview the UI:

```sh
python3 -m http.server 8080 --bind 127.0.0.1 --directory web
```

Open <http://localhost:8080>. The page loads JavaScript and fonts from CDNs, so
it needs an internet connection. There is no Node.js install or build step.

You can also run a command without opening an interactive shell:

```sh
nix develop --command python3 -m http.server 8080 --bind 127.0.0.1 --directory web
nix develop --command rustc --version
```

Run the Cargo commands above from the repository root inside the shell.
OpenSSL headers and
libraries are exposed through pkg-config for crates with native TLS dependencies.
Start your editor from this shell to make the pinned rust-analyzer and Rust
sources available to it, or use your editor's direnv integration.

## Optional direnv integration

With [direnv](https://direnv.net/docs/hook.html) installed and hooked into your
shell, run `direnv allow` in the repo root. The included `.envrc` loads the flake
automatically. [nix-direnv](https://github.com/nix-community/nix-direnv) is optional
and adds caching for faster shell entry.

## Maintaining the environment

```sh
nix fmt
nix flake check
nix flake update nixpkgs
```

Keep `flake.nix` and `flake.lock` in Git and commit lockfile updates so everyone
uses the same package versions. `nix flake check` validates the development
environment; run the Cargo checks separately for the telemetry crate.
