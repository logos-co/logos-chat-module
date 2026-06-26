{
  description = "Logos Chat Module";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";

    # Pinned to the commit (post-v0.1.2) carrying the zerokit/RLN nix build fix
    # (delivery-module #49). Kept in lockstep with logos-chat-ui's pin. Not yet
    # tagged — re-pin to the release tag once one is cut.
    logos-delivery-module.url = "github:logos-co/logos-delivery-module/2577383f6e0de24793b523d6ea4991aa6339afd8";
  };

  outputs = inputs@{ self, logos-module-builder, logos-delivery-module, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = fn: nixpkgs.lib.genAttrs systems fn;

      # The builder runs logos-lidl-gen to emit the module-impl C ABI scaffold
      # (the `ChatModule` trait + logos_module_* exports) at rust-lib/generated/,
      # compiles the staticlib, and stages it — all driven by
      # metadata.json#codegen.rust. No build.rs, no per-flake buildRustPackage.
      module = system:
        logos-module-builder.lib.mkLogosModule {
          src = ./.;
          configFile = ./metadata.json;
          flakeInputs = {
            delivery_module = logos-delivery-module;
          } // inputs;
        };
    in
    {
      packages = forAllSystems (system:
        let m = (module system).packages.${system};
        in m // {
          # CI builds `.#chat_module`; alias it to the plugin package. The full
          # set `m` (default, install, lidl, …) is exposed too, so the UI module
          # can consume chat_module's published .lidl contract.
          chat_module = m.default;

          # The matching delivery_module .lgx, re-exported from this flake's
          # locked delivery input, so the exact delivery_module rev chat_module is
          # built against can be installed alongside it.
          "delivery_module-lgx" = logos-delivery-module.packages.${system}.lgx;
        });

      # `nix run .#generate` materialises the two gitignored inputs `rust-lib/`
      # references into the working tree, both from the rev the builder pins: the
      # provider scaffold (logos-lidl-gen over chat_module.lidl) at
      # rust-lib/generated/, and the SDK source the crate path-deps as
      # `../logos-rust-sdk-src`. After it, bare `cargo build/test/clippy` works in
      # rust-lib/ directly, with no staged copy.
      apps = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          lidlGen = logos-module-builder.inputs.logos-rust-sdk.packages.${system}.lidl-gen;
          sdkSrc = logos-module-builder.packages.${system}.rust-sdk-src;
          generate = pkgs.writeShellApplication {
            name = "chat-module-generate";
            runtimeInputs = [ lidlGen pkgs.git ];
            text = ''
              root="$(git rev-parse --show-toplevel)"
              echo "generating rust-lib/generated/provider_gen.rs ..."
              mkdir -p "$root/rust-lib/generated"
              logos-lidl-gen "$root/rust-lib/chat_module.lidl" --provider \
                --dep delivery_module="$root/rust-lib/deps/delivery_module.lidl" \
                -o "$root/rust-lib/generated/provider_gen.rs"
              echo "staging the SDK source at logos-rust-sdk-src/ ..."
              rm -rf "''${root:?}/logos-rust-sdk-src"
              cp -RL "${sdkSrc}" "$root/logos-rust-sdk-src"
              chmod -R u+w "$root/logos-rust-sdk-src"
              echo "done. bare 'cargo build' now works in rust-lib/"
            '';
          };
        in {
          generate = {
            type = "app";
            program = "${generate}/bin/chat-module-generate";
          };
        });

      # Build tools for bare `cargo` (clippy/test) that the module build needs but
      # the CI runner image lacks: `protobuf` (protoc) for hashgraph-like-consensus's
      # prost-build build script. Sourced from the same pinned nixpkgs as the nix
      # build (metadata.json#nix.rust.packages.build), so `nix develop --command
      # cargo …` uses the repo's own pin, not a separate toolchain.
      devShells = forAllSystems (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            packages = [ pkgs.protobuf ];
          };
        });
    };
}
