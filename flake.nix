{
  description = "Logos ChatSDK Module";

  inputs = {
    # Follow the same nixpkgs as logos-liblogos to ensure compatibility
    nixpkgs.follows = "logos-liblogos/nixpkgs";
    logos-cpp-sdk.url = "github:logos-co/logos-cpp-sdk";
    logos-liblogos.url = "github:logos-co/logos-liblogos";
    logos-chat.url = "git+https://github.com/logos-messaging/logos-chat?submodules=1&rev=53302e4373755b72391727de3d5d2b30e1239dbb";
  };

  outputs = { self, nixpkgs, logos-cpp-sdk, logos-liblogos, logos-chat }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f {
        pkgs = import nixpkgs { inherit system; };
        logosSdk = logos-cpp-sdk.packages.${system}.default;
        logosLiblogos = logos-liblogos.packages.${system}.default;
        logosChat = logos-chat.packages.${system}.default;
      });
    in
    {
      packages = forAllSystems ({ pkgs, logosSdk, logosLiblogos, logosChat }: 
        let
          # Common configuration
          common = import ./nix/default.nix { inherit pkgs logosSdk logosLiblogos logosChat; };
          src = ./.;
          
          # Library package (plugin + liblogoschat)
          lib = import ./nix/lib.nix { inherit pkgs common src logosChat; };
          
          # Include package (generated headers from plugin)
          include = import ./nix/include.nix { inherit pkgs common src lib logosSdk; };
          
          # Combined package
          combined = pkgs.symlinkJoin {
            name = "logos-chatsdk-module";
            paths = [ lib include ];
          };
        in
        {
          # Individual outputs
          lib = lib;
          
          # Default package (combined)
          default = combined;
        }
      );

      devShells = forAllSystems ({ pkgs, logosSdk, logosLiblogos, logosChat }: {
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
            echo "Logos ChatSDK Module development environment"
            echo "LOGOS_CPP_SDK_ROOT: $LOGOS_CPP_SDK_ROOT"
            echo "LOGOS_LIBLOGOS_ROOT: $LOGOS_LIBLOGOS_ROOT"
            echo "LOGOS_CHAT_ROOT: $LOGOS_CHAT_ROOT"
          '';
        };
      });
    };
}
