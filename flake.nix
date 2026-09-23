{
  description = "Logos Chat Module";

  # Pull pre-built artifacts (delivery module, liblogosdelivery, …) from the
  # self-hosted Logos Attic cache, for local builds too — CI configures its
  # substituters itself. Read-only and public; see infra-ci#263. Only the
  # public (master-built) cache belongs here; ci is CI-only by design.
  nixConfig = {
    extra-substituters = [ "https://cache.nix.logos.co/public" ];
    extra-trusted-public-keys = [ "public:l4HrXgL4nw246+LBh2SOJyhz64BoGegOYLheT/iIAPU=" ];
  };

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/0.3.1";

    # Delivery's Windows target is on master after PR #127. The lockfile pins
    # the merged revision until a release includes it.
    logos-delivery-module.url = "github:logos-co/logos-delivery-module";
  };

  outputs = inputs@{ self, logos-module-builder, logos-delivery-module, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = fn: nixpkgs.lib.genAttrs systems fn;

      # x86_64-windows is a cross PSEUDO-SYSTEM the builder already understands
      # (logos-module-builder lib/common.nix routes it to
      # logos-nix.lib.mkWindowsPkgs, and picks the build platform separately).
      #
      # `packages` ONLY. `apps` and `devShells` below both do
      # `import nixpkgs { inherit system; }`, which for this key is a NATIVE
      # Windows instantiation and dies in cc-wrapper — and neither a dev shell
      # nor the codegen runner means anything on a cross target anyway.
      #
      # chat_ui needs this: a consumer resolves a dependency's headers through
      # `packages.<system>`, so without a Windows entry here chat_ui's own cross
      # build has nothing to read (logos-module-builder#199 turns that into a
      # named error rather than a silent fallback to this source tree).
      targets = systems ++ [ "x86_64-windows" ];
      forAllTargets = fn: nixpkgs.lib.genAttrs targets fn;

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
      packages = forAllTargets (system:
        let m = (module system).packages.${system};
        in m // {
          # CI builds `.#chat_module`; alias it to the plugin package. The full
          # set `m` (default, install, lidl, …) is exposed too, so the UI module
          # can consume chat_module's published .lidl contract.
          chat_module = m.default;

          # Re-export the matching delivery package for every target, including
          # Windows, so the two modules can be installed together.
          "delivery_module-lgx" = logos-delivery-module.packages.${system}.lgx;
        } // nixpkgs.lib.optionalAttrs (system == "x86_64-windows") {
          # The Windows smoke job stages the exact delivery build this module
          # uses, along with its installable layout.
          "delivery_module-default" = logos-delivery-module.packages.${system}.default;
          "delivery_module-install-portable" = logos-delivery-module.packages.${system}.install-portable;
        });

      # `nix run .#generate` materialises the two gitignored inputs `rust-lib/`
      # references into the working tree: the provider scaffold (logos-lidl-gen
      # over chat_module.lidl and delivery's published contract) at
      # rust-lib/generated/, and the SDK source the crate path-deps as
      # `../logos-rust-sdk-src`. After it, bare `cargo build/test/clippy` works in
      # rust-lib/ directly, with no staged copy.
      apps = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          lidlGen = logos-module-builder.inputs.logos-rust-sdk.packages.${system}.lidl-gen;
          sdkSrc = logos-module-builder.packages.${system}.rust-sdk-src;
          deliveryLidl = logos-delivery-module.packages.${system}.lidl;
          generate = pkgs.writeShellApplication {
            name = "chat-module-generate";
            runtimeInputs = [ lidlGen pkgs.git ];
            text = ''
              root="$(git rev-parse --show-toplevel)"
              echo "generating rust-lib/generated/provider_gen.rs ..."
              mkdir -p "$root/rust-lib/generated"
              logos-lidl-gen "$root/rust-lib/chat_module.lidl" --provider \
                --dep delivery_module="${deliveryLidl}/delivery_module.lidl" \
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
