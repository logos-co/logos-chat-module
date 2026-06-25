{
  description = "Logos Chat Module";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/tutorial-v3";
    nix-bundle-lgx.url = "github:logos-co/nix-bundle-lgx";
    # TODO(testnet-v02-mix): repin to a pushed logos-messaging/logos-chat rev once
    # the feat/logos-testnetv02-mix branch (mix sender-anonymity) is upstreamed.
    logos-chat.url = "git+file:///Users/prem/Code/logos-chat-canonical?submodules=1&ref=feat/logos-testnetv02-mix";
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
      tests = {
        dir = ./tests;
        mockCLibs = [ "logoschat" ];
      };
    };
}
