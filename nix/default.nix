# Common build configuration shared across all packages
{ pkgs, logosSdk, logosLiblogos, logosChat }:

{
  pname = "logos-chat-module";
  version = "1.0.0";
  
  # Common native build inputs
  nativeBuildInputs = [ 
    pkgs.cmake 
    pkgs.ninja 
    pkgs.pkg-config
    pkgs.qt6.wrapQtAppsNoGuiHook
  ];
  
  # Common runtime dependencies
  buildInputs = [ 
    pkgs.qt6.qtbase 
    pkgs.qt6.qtremoteobjects 
  ];
  
  # Common CMake flags
  cmakeFlags = [ 
    "-GNinja"
    "-DLOGOS_CPP_SDK_ROOT=${logosSdk}"
    "-DLOGOS_LIBLOGOS_ROOT=${logosLiblogos}"
    "-DLOGOS_CHAT_ROOT=${logosChat}"
    "-DLOGOS_CHAT_MODULE_USE_VENDOR=OFF"
  ];

  # Metadata
  meta = with pkgs.lib; {
    description = "Logos Chat Module - Provides chat communication capabilities";
    platforms = platforms.unix;
  };
}
