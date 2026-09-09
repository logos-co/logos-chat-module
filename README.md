# logos-chat-module

A Rust [Logos Module](https://github.com/logos-co/logos-liblogos) that wraps
[libchat](https://github.com/logos-messaging/libchat) and exposes
e2e-encrypted chat over the Logos IPC bus. Loaded as a `cdylib` module by
`liblogos_core`; depends on `delivery_module` at runtime (declared in
`metadata.json`).

A companion QML UI App lives in
[`logos-chat-ui`](https://github.com/logos-co/logos-chat-ui).

## Build

```bash
nix build .#chat_module    # the full Qt plugin
```

`nix build` is the entry point and needs no manual hash bookkeeping:
`logos-module-builder` runs `logos-lidl-gen` to emit the module-impl scaffold,
fetches the Cargo deps recorded in `rust-lib/Cargo.lock`, and compiles the
staticlib. Bumping the `libchat` pin is just `cargo metadata` (or `cargo update
-p`) to refresh `rust-lib/Cargo.lock`; the next `nix build` picks it up.

For a bare `cargo build`, first run `nix run .#generate`. It materialises the two
gitignored inputs `rust-lib/` references into the working tree: the SDK source
tree (`logos-rust-sdk-src/`) and the generated scaffold (`rust-lib/generated/`),
both from the rev the builder pins. Then cargo works in `rust-lib/` directly:

```bash
nix run .#generate                                          # stage SDK source + scaffold
cargo build --release --manifest-path rust-lib/Cargo.toml   # Rust staticlib only
```

`cargo` requires `pkg-config`, `perl`, and a C toolchain — `libchat`'s
storage/crypto stack pulls in `openssl-src`, which compiles OpenSSL from source.

## API

The contract consumers call is
[`rust-lib/chat_module.lidl`](rust-lib/chat_module.lidl) (`interface: cdylib`) —
the single source of truth. It declares the records the module exchanges, the
methods it answers and the events it emits, and every client is generated from
it: `metadata.json#codegen` drives `logos-lidl-gen` to produce this module's own
implementation scaffold, and a consumer runs the same generator over the same
file to get a typed caller.

The published **[API reference](https://logos-co.github.io/logos-chat-module/latest/pages/api_reference.html)**
is rendered from that contract, so it cannot drift from it. Alongside it are a
guide to using the API, how the module is put together, and what it logs. See
[Documentation](#documentation) below to build the site locally.

## Doc-tests

The specs under [`doctests/`](doctests/) are executable usage tutorials: each
loads `chat_module` into headless
[`logoscore`](https://github.com/logos-co/logos-logoscore-cli) daemons and
drives a real, end-to-end-encrypted exchange between them over the live
delivery network, documenting the module's API by example.
[`chat-module-exchange.test.yaml`](doctests/chat-module-exchange.test.yaml) is
the two-instance 1:1 round-trip;
[`chat-module-group.test.yaml`](doctests/chat-module-group.test.yaml) runs a
three-instance GroupV2 conversation (create, grow member by member, fan-out
messages with sender attribution). They run on every PR via
[`.github/workflows/doctests.yml`](.github/workflows/doctests.yml) (the
[shared doctest CLI](https://github.com/logos-co/logos-doctest) builds the
commit under test), which also makes them an integration check. Run one locally
against latest master (add `--release-for logos-chat-module=<branch-or-sha>` to
pin it to a pushed commit instead):

```bash
nix run github:logos-co/logos-doctest -- run doctests/chat-module-exchange.test.yaml
```

## Documentation

The module's documentation is published at
**<https://logos-co.github.io/logos-chat-module/>** — the API reference, a guide
to using it, and the internal docs.

### Building the documentation

The site is `docs/lidl2rst.py` (API extraction from the `.lidl` contract) →
Sphinx (rendering), with the Markdown guides in `docs/pages/` pulled in via
myst-parser. Everything it needs is Python:

```bash
python3 -m venv .venv && source .venv/bin/activate
pip install -r docs/requirements-dev.txt

make docs            # build into docs/_build/html
make docs-preview    # rebuild and reload the browser as you edit
```

Publishing is automatic: `.github/workflows/docs.yml` deploys to the `gh-pages`
branch when a release is published, under `latest/` and the release tag. Pushing
to a branch builds the site and uploads it as a `docs-preview` artifact instead,
so a docs change can be previewed before it ships. Adding a new release to the
version dropdown means editing
[`docs/_root/switcher.json`](docs/_root/switcher.json).
