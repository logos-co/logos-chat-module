# Builds the logos-chat-module library
{ pkgs, common, src, logosChat, logosDeliveryModule, logosSdk }:

pkgs.stdenv.mkDerivation {
  pname = "${common.pname}-lib";
  version = common.version;

  inherit src;
  inherit (common) buildInputs cmakeFlags meta;

  # Add logosSdk to nativeBuildInputs for logos-cpp-generator
  nativeBuildInputs = common.nativeBuildInputs ++ [ logosSdk ];

  # Determine platform-specific library extension
  libchatLib = if pkgs.stdenv.hostPlatform.isDarwin then "liblibchat.dylib" else "liblibchat.so";

  preConfigure = ''
    runHook prePreConfigure

    # Create generated_code directory for pre-generated files
    mkdir -p ./generated_code

    # Copy delivery module's generated proxy headers if available
    if [ -d "${logosDeliveryModule}/include" ]; then
      cp -r "${logosDeliveryModule}/include"/* ./generated_code/
    fi

    # Copy SDK headers needed by generated code (logos_types.h etc.)
    if [ -f "${logosSdk}/include/cpp/logos_types.h" ]; then
      cp "${logosSdk}/include/cpp/logos_types.h" ./generated_code/
    elif [ -f "${logosSdk}/include/logos_types.h" ]; then
      cp "${logosSdk}/include/logos_types.h" ./generated_code/
    else
      echo "WARNING: logos_types.h not found in SDK"
    fi

    # Run logos-cpp-generator to produce logos_sdk.cpp from our metadata
    logos-cpp-generator --metadata ${src}/metadata.json --general-only --output-dir ./generated_code

    # Create include subdirectory for installed layout headers
    if [ -f "./generated_code/logos_sdk.h" ]; then
      mkdir -p ./generated_code/include
      for file in ./generated_code/*.h; do
        [ -f "$file" ] && cp "$file" ./generated_code/include/
      done
      for file in ./generated_code/*.cpp; do
        [ -f "$file" ] && cp "$file" ./generated_code/include/
      done
    fi

    runHook postPreConfigure
  '';

  postInstall = ''
    mkdir -p $out/lib

    # Copy libchat library from the Rust build output
    srcLib="${logosChat}/lib/''${libchatLib}"
    if [ ! -f "$srcLib" ]; then
      echo "Expected ''${libchatLib} in ${logosChat}/lib/" >&2
      exit 1
    fi
    cp "$srcLib" "$out/lib/"

    # Fix the install name of libchat on macOS
    ${pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
      ${pkgs.darwin.cctools}/bin/install_name_tool -id "@rpath/''${libchatLib}" "$out/lib/''${libchatLib}"
    ''}

    # Copy the chat module plugin from the installed location
    if [ -f "$out/lib/logos/modules/chat_module_plugin.dylib" ]; then
      cp "$out/lib/logos/modules/chat_module_plugin.dylib" "$out/lib/"

      ${pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
        for dep in $(${pkgs.darwin.cctools}/bin/otool -L "$out/lib/chat_module_plugin.dylib" | grep liblibchat | awk '{print $1}'); do
          ${pkgs.darwin.cctools}/bin/install_name_tool -change "$dep" "@rpath/''${libchatLib}" "$out/lib/chat_module_plugin.dylib"
        done
      ''}
    elif [ -f "$out/lib/logos/modules/chat_module_plugin.so" ]; then
      cp "$out/lib/logos/modules/chat_module_plugin.so" "$out/lib/"
    else
      echo "Error: No chat_module_plugin library file found"
      exit 1
    fi

    # Remove the nested structure we don't want
    rm -rf "$out/lib/logos" 2>/dev/null || true
    rm -rf "$out/share" 2>/dev/null || true
  '';
}
