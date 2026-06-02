{
  description = "chat_module: a Rust Logos Module wrapping libchat, backed by delivery_module";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/c_ffi";
    logos-module-builder.inputs.logos-cpp-sdk.url = "github:logos-co/logos-cpp-sdk/c_ffi";
    logos-module-client.url = "github:logos-co/logos-module-client/binary-params";
    logos-rust-sdk.url = "github:logos-co/logos-rust-sdk/binary-params";
    logos-delivery-module.url = "github:logos-co/logos-delivery-module/v0.1.2";
    nixpkgs.follows = "logos-module-builder/nixpkgs";
  };

  outputs = inputs@{ self, logos-module-builder, logos-module-client,
                     logos-delivery-module, nixpkgs, ... }:
    let
      mkModule = logos-module-builder.lib.mkLogosModule;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = fn: nixpkgs.lib.genAttrs systems fn;

    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};

          moduleClient    = logos-module-client.packages.${system}.logos-module-client;
          moduleClientLib = logos-module-client.packages.${system}.logos-module-client-lib;

          # ── Build the Rust staticlib ──────────────────────────────────────
          #
          # Cargo dependencies (libchat, logos-rust-sdk) resolve from git
          # URLs declared in Cargo.toml. Nix needs `outputHashes` for each
          # git dependency in Cargo.lock for a reproducible offline build.
          #
          # First-build workflow:
          #   1. Run `cargo generate-lockfile` (network) to produce Cargo.lock.
          #   2. `nix build .#chat_module` — fails with messages like
          #      "missing hash for client-X.Y.Z".
          #   3. Re-run with the hash printed by Nix; paste it into the
          #      outputHashes attrset below.
          chatModuleLib = pkgs.rustPlatform.buildRustPackage {
            pname   = "chat_module";
            version = "1.0.0";
            src     = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
              # All git dependencies from Cargo.lock that need explicit
              # hashes. Fill each one in as Nix prints the expected hash on
              # the first build attempt.
              outputHashes = {
                "chat-proto-0.1.0"      = "sha256-aCl80VOIkd/GK3gnmRuFoSAvPBfeE/FKCaNlLt5AbUU=";
                "chat-sqlite-0.1.0"     = "sha256-dpT176lvJDnUrXdeHsv4UqwY67XD/Bu6pfnzoiLtL7I=";
                "client-0.1.0"          = "sha256-dpT176lvJDnUrXdeHsv4UqwY67XD/Bu6pfnzoiLtL7I=";
                "crypto-0.1.0"          = "sha256-dpT176lvJDnUrXdeHsv4UqwY67XD/Bu6pfnzoiLtL7I=";
                "double-ratchets-0.0.1" = "sha256-dpT176lvJDnUrXdeHsv4UqwY67XD/Bu6pfnzoiLtL7I=";
                "libchat-0.1.0"         = "sha256-dpT176lvJDnUrXdeHsv4UqwY67XD/Bu6pfnzoiLtL7I=";
                # binary-params rev — refresh with the hash `nix build` prints.
                "logos-rust-sdk-0.2.0"  = pkgs.lib.fakeHash;
                "storage-0.1.0"         = "sha256-dpT176lvJDnUrXdeHsv4UqwY67XD/Bu6pfnzoiLtL7I=";
              };
            };

            doCheck = false;

            nativeBuildInputs = [ pkgs.pkg-config pkgs.perl ];

            # Install the cbindgen-generated header alongside the .a so
            # `mkModule.preConfigure` below can stage both into the
            # Qt-plugin build sandbox.
            postInstall = ''
              mkdir -p $out/include
              cp include/chat_module.h $out/include/
            '';
          };

          # ── Logos Module (Qt plugin + codegen) ────────────────────────────
          chatModule = mkModule {
            src        = ./.;
            configFile = ./metadata.json;
            flakeInputs = {
              delivery_module = logos-delivery-module;
            } // inputs;

            extraBuildInputs = [ moduleClientLib ];

            preConfigure = ''
              echo "=== Staging pre-built Rust chat_module library and header ==="
              mkdir -p lib include
              cp ${chatModuleLib}/lib/libchat_module.a lib/
              cp ${chatModuleLib}/include/chat_module.h include/chat_module.h
              echo "=== Rust library + header staged ==="

              export LOGOS_MODULE_CLIENT_ROOT="${moduleClient}"
            '';
          };

        in
        {
          chat_module         = chatModule.packages.${system}.default;
          chat_module_install = chatModule.packages.${system}.install;
          default             = chatModule.packages.${system}.default;
        }
      );
    };
}
