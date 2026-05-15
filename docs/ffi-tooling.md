# FFI tooling choice for chat_module

`chat_module` uses **cbindgen** to generate `include/chat_module.h` from `src/lib.rs`. FFI entries are `#[no_mangle] pub extern "C"` with raw C-primitive types. `safer-ffi` is not used.

## Why not safer-ffi

`logos-cpp-generator --from-c-header` parses our header to emit the Qt plugin glue (`logos-cpp-sdk/cpp-generator/experimental/c_header_parser.cpp`). The parser:

- Skips any line starting with `typedef`, `struct`, `union`, or `enum`.
- Detects heap-string returns by the literal regex `^char\s*\*\s*$`; the result drives whether the generated Qt wrapper calls `chat_module_free_string` on the return.
- Falls through to opaque `"any"` for type names outside `{void, bool, char, int{32,64}_t, uint{32,64}_t, double, float}`.
- Matches the free function by exact name `<prefix>_free_string`.

`safer-ffi`'s typed-ownership returns (`char_p::Box`, `c_slice::Box`, …) are emitted as typedef'd structs. The parser would skip those typedefs and fail to recognise the `free_string` ownership — every returned string would leak. Restricting `safer-ffi` to raw `*mut c_char` removes the type-safety win and leaves only automatic `catch_unwind` wrapping, which is dead code under `panic = "abort"` (forced transitively by `libchat`).

cbindgen emits plain `extern "C"` declarations with primitive C types — exactly what the parser handles.

## What cbindgen buys

- `src/lib.rs` is the single source of truth for FFI signatures and doc comments.
- `include/chat_module.h` is a build artifact regenerated on every `cargo build`.
- Build-time only; no runtime overhead.

## Wiring

- `Cargo.toml`: `cbindgen` under `[build-dependencies]`.
- `cbindgen.toml`: C language, `CHAT_MODULE_H` guard, doxy comments, `sort_by = "None"`, `i32 → int32_t` rename.
- `build.rs`: runs cbindgen every `cargo build`.
- `flake.nix`: `chatModuleLib` installs the regenerated header alongside `libchat_module.a`; `chatModule.preConfigure` stages both into the Qt-plugin build sandbox.

## Panic boundary

`panic = "abort"` is mandatory (`safer-ffi` transitive via `libchat` requires it). A `catch_unwind` wrapper at each FFI entry would be dead code — the abort runs first. `src/panic_hook.rs` installs `std::panic::set_hook` on first init to print panic location + payload to stderr before the abort; `libchat` crashes inside the `logos_host_qt` subprocess become locatable.

## When to revisit

If `logos-cpp-generator` grows a real C parser (typedefs, opaque handles, ownership annotations), `safer-ffi` becomes reachable and worth the migration. Until then, cbindgen is the right tool.
