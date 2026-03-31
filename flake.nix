{
  description = "Logos Chat Module";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";
    nix-bundle-lgx.url = "github:logos-co/nix-bundle-lgx";
    logos-chat.url = "git+https://github.com/logos-messaging/logos-chat?submodules=1&rev=53302e4373755b72391727de3d5d2b30e1239dbb";
  };

  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
      externalLibInputs = {
        chat = inputs.logos-chat;
      };
      # TODO: The module builder copies the wrong header from the flake output.
      # liblogoschat.h lives in the source tree, not the build output.
      # Should be fixed in logos-module-builder (e.g. header_path in metadata.json).
      preConfigure = ''
        mkdir -p lib
        for f in $(find /nix/store -maxdepth 5 -name "liblogoschat.h" 2>/dev/null); do
          cp "$f" lib/ 2>/dev/null || true
        done
      '';
    };
}
