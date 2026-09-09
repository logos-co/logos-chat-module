# Versioning

This module uses [Semantic Versioning](https://semver.org/) (`MAJOR.MINOR.PATCH`) with the following conventions, matching the [delivery module](https://github.com/logos-co/logos-delivery-module)'s.

## Scheme

```
0 . <testnet> . <patch>
│        │           └── module release within a testnet
│        └────────────── tracks the Logos testnet number
└─────────────────────── fixed at 0; breaking changes are expected
```

### MAJOR — fixed at `0`

The major version is kept at `0` for the foreseeable future. A value of `0` signals that the API is **not yet stable**: breaking changes may be introduced in any release. Once the API stabilises across testnets this policy will be revisited.

### MINOR — Logos testnet number

The minor version is incremented to match the Logos testnet with which this module is compatible. Bumping MINOR resets PATCH to `0`.

Example: `0.2.0` targets Logos Testnet 2.

### PATCH — module release within a testnet

The patch version is incremented for every module release made against the same testnet: bug fixes, dependency updates, performance improvements, or any other change that does not change the target testnet.

Example: `0.2.3` is the fourth release of the module targeting Logos Testnet 2.

## Three files carry the version

The module's version is written down in three places, and they move together: `metadata.json` (what the host reads), `rust-lib/Cargo.toml` (the crate), and the `version` clause in `rust-lib/chat_module.lidl` (the contract, so a consumer reading the `.lidl` alone can tell which release it describes).

## Releasing

1. Update the version in [`metadata.json`](https://github.com/logos-co/logos-chat-module/blob/master/metadata.json), [`rust-lib/Cargo.toml`](https://github.com/logos-co/logos-chat-module/blob/master/rust-lib/Cargo.toml) and [`rust-lib/chat_module.lidl`](https://github.com/logos-co/logos-chat-module/blob/master/rust-lib/chat_module.lidl).
2. Add the new version to [`docs/_root/switcher.json`](https://github.com/logos-co/logos-chat-module/blob/master/docs/_root/switcher.json) and mark it `"preferred"` — the docs version dropdown is not generated, so a release missing from that file is missing from the dropdown.
3. Tag the commit: `git tag v<version>`.
4. Push the tag: `git push origin v<version>`.
5. Publish the release on GitHub. That is what triggers the docs build: the site is published to `<tag>/`, and to `latest/` when the tag is the newest and not a prerelease.
