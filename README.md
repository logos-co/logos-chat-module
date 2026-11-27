# logos-chat-module

A Rust [Logos Module](https://github.com/logos-co/logos-liblogos) that wraps
[libchat](https://github.com/logos-messaging/logos-chat) and exposes
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
staticlib. It resolves delivery's contract from the pinned flake input, so
the generated client matches the delivery module at build time. Bumping the
`libchat` pin is just `cargo metadata` (or `cargo update
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
is rendered from that contract, so it cannot drift from it, and alongside it are
the internal docs: how the module is put together, and what it logs. For a
walk-through of the calls in order, with working code, see
[building a module that uses the Chat module API](https://docs.logos.co/messaging/chat-module/build-logos-module-that-uses-chat-module-api).
See [Documentation](#documentation) below to build the site locally.

## Runtime

End-to-end chat needs a `delivery_module` available to the host at runtime; the
flake pins [`logos-delivery-module`](https://github.com/logos-co/logos-delivery-module)
master, which runs logos-delivery past v0.41.0-rc.2. Sending on `logos.test` or
`logos.dev` also needs delivery's three RLN modules, `rln_module`, `rln_lez_module`
and `membership_allocation_client_module`, installed beside it (see delivery's
[RLN guide](https://github.com/logos-co/logos-delivery-module/blob/master/docs/pages/rln.md)).
Load `chat_module` via `logoscore` or Basecamp.

Bring-up is `init(config)`, taking a `ChatConfig` record whose every field is
optional: `delivery_preset` (empty or absent → `logos.test`) and `log_level`.
What it does, and how you learn the module is ready, is on
[`init`](https://logos-co.github.io/logos-chat-module/latest/pages/api_reference.html#init)
in the API reference.

The first `init` makes the module's account and keeps its key in the instance
directory, so the address `get_address` reports and the name `set_display_name`
gives it outlive a restart. The key is obfuscated there, not encrypted.

A generated client passes the record itself. From the CLI, prefix the record's
JSON with `json:` so `logoscore call` sends it as an object:

```bash
logoscore call chat_module init 'json:{"delivery_preset":"logos.test","log_level":"debug"}'
```

Two environment variables, read at `init` from the environment the module
process inherits from its host, point the module at other infrastructure:

| Variable | Effect |
|---|---|
| `LOGOS_CHAT_DELIVERY_CONFIG_PATH` | Path to a JSON file whose object is passed to `delivery_module`'s `createNode` as is, instead of the config built from `delivery_preset`. An unreadable file or one that is not a JSON object puts delivery in the `error` state. |
| `LOGOS_CHAT_CONTACT_REGISTRY_URL` | Contact registry to publish to and resolve accounts from, instead of `https://devnet.chat-kc.logos.co`. |

For example, to join your own delivery nodes:

```bash
cat > delivery.json <<'EOF'
{
  "mode": "Core",
  "preset": "logos.dev",
  "messagingOverrides": {
    "entry-node": ["/ip4/203.0.113.10/tcp/30303/p2p/16Uiu2HAm..."],
    "storenode": "/ip4/203.0.113.10/tcp/30303/p2p/16Uiu2HAm..."
  }
}
EOF
LOGOS_CHAT_DELIVERY_CONFIG_PATH=$PWD/delivery.json logosctl daemon start
```

Keep the config in `delivery_module`'s layered shape (only `mode`, `preset`
and `messagingOverrides` at the top level), so the node picks its own ports.

## Doc-tests

The specs under [`doctests/`](doctests/) are executable usage tutorials: each
loads `chat_module` into headless
[`logoscore`](https://github.com/logos-co/logos-logoscore-cli) daemons and
drives a real, end-to-end-encrypted exchange between them over the live
delivery network, documenting the module's API by example.
[`chat-module-exchange.test.yaml`](doctests/chat-module-exchange.test.yaml) is
the two-instance 1:1 round-trip;
[`chat-module-group.test.yaml`](doctests/chat-module-group.test.yaml) runs a
three-instance group conversation (create, grow member by member, fan-out
messages with sender attribution). They run on every PR via
[`.github/workflows/doctests.yml`](.github/workflows/doctests.yml) (the
[shared doctest CLI](https://github.com/logos-co/logos-doctest) builds the
commit under test), which also makes them an integration check. Run one locally
against latest master (add `--release-for logos-chat-module=<branch-or-sha>` to
pin it to a pushed commit instead):

```bash
nix run github:logos-co/logos-doctest -- run doctests/chat-module-exchange.test.yaml
```

CI runs them on a local delivery fleet rather than logos.dev, so a pull request
does not depend on a public fleet being up: [`doctests/fleet/`](doctests/fleet/)
starts a boot node and a Store node in docker, and
`LOGOS_CHAT_DELIVERY_CONFIG_PATH` points every daemon's `chat_module` at them. A
nightly run on master joins logos.dev. Every run uses the devnet contact
registry. To run a spec on the local fleet:

```bash
docker compose -f doctests/fleet/docker-compose.yml up -d --wait
LOGOS_CHAT_DELIVERY_CONFIG_PATH=$PWD/doctests/fleet/delivery.json \
  nix run github:logos-co/logos-doctest -- run doctests/chat-module-exchange.test.yaml
docker compose -f doctests/fleet/docker-compose.yml down
```

## Documentation

The module's documentation is published at
**<https://logos-co.github.io/logos-chat-module/>** — the API reference
generated from the contract, and the internal docs.

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
