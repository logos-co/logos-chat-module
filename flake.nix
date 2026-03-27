{
  description = "Logos Chat Module";

  inputs = {
    nixpkgs.follows = "logos-liblogos/nixpkgs";
    logos-cpp-sdk.url = "github:logos-co/logos-cpp-sdk";
    logos-liblogos.url = "github:logos-co/logos-liblogos";
    logos-delivery-module.url = "github:logos-co/logos-delivery-module";
    libchat.url = "github:logos-messaging/libchat";

    # Align SDK version across transitive dependencies
    logos-delivery-module.inputs.logos-cpp-sdk.follows = "logos-cpp-sdk";
  };

  outputs = { self, nixpkgs, logos-cpp-sdk, logos-liblogos, logos-delivery-module, libchat }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        pkgs = import nixpkgs { inherit system; };
        logosSdk = logos-cpp-sdk.packages.${system}.default;
        logosLiblogos = logos-liblogos.packages.${system}.default;
        logosDeliveryModule = logos-delivery-module.packages.${system}.default;
        logosChat = libchat.packages.${system}.default;
      });
    in
    {
      packages = forAllSystems ({ pkgs, logosSdk, logosLiblogos, logosDeliveryModule, logosChat }:
        let
          common = import ./nix/default.nix { inherit pkgs logosSdk logosLiblogos logosDeliveryModule logosChat; };
          src = ./.;
          lib = import ./nix/lib.nix { inherit pkgs common src logosChat logosSdk; logosDeliveryModule = logosDeliveryModule; };
          include = import ./nix/include.nix { inherit pkgs common src lib logosSdk; };
          combined = pkgs.symlinkJoin {
            name = "logos-chat-module";
            paths = [ lib include ];
          };
        in
        {
          lib = lib;
          default = combined;
        }
      );

      devShells = forAllSystems ({ pkgs, logosSdk, logosLiblogos, logosDeliveryModule, logosChat }: {
        default = pkgs.mkShell {
          nativeBuildInputs = [
            pkgs.cmake
            pkgs.ninja
            pkgs.pkg-config
          ];
          buildInputs = [
            pkgs.qt6.qtbase
            pkgs.qt6.qtremoteobjects
          ];

          shellHook = ''
            export LOGOS_CPP_SDK_ROOT="${logosSdk}"
            export LOGOS_LIBLOGOS_ROOT="${logosLiblogos}"
            export LOGOS_CHAT_ROOT="${logosChat}"
            export LOGOS_DELIVERY_ROOT="${logosDeliveryModule}"
            echo "Logos Chat Module development environment"
            echo "LOGOS_CPP_SDK_ROOT: $LOGOS_CPP_SDK_ROOT"
            echo "LOGOS_LIBLOGOS_ROOT: $LOGOS_LIBLOGOS_ROOT"
            echo "LOGOS_CHAT_ROOT: $LOGOS_CHAT_ROOT"
            echo "LOGOS_DELIVERY_ROOT: $LOGOS_DELIVERY_ROOT"
          '';
        };
      });
    };
}
