# Common build configuration shared across all packages
{ pkgs, logosSdk, logosLiblogos, logosDeliveryModule, logosChat }:

{
  pname = "logos-chat-module";
  version = "1.0.0";

  nativeBuildInputs = [
    pkgs.cmake
    pkgs.ninja
    pkgs.pkg-config
    pkgs.qt6.wrapQtAppsNoGuiHook
  ];

  buildInputs = [
    pkgs.qt6.qtbase
    pkgs.qt6.qtremoteobjects
  ];

  cmakeFlags = [
    "-GNinja"
    "-DLOGOS_CPP_SDK_ROOT=${logosSdk}"
    "-DLOGOS_LIBLOGOS_ROOT=${logosLiblogos}"
    "-DLOGOS_CHAT_ROOT=${logosChat}"
    "-DLOGOS_DELIVERY_ROOT=${logosDeliveryModule}"
    "-DLOGOS_CHAT_MODULE_USE_VENDOR=OFF"
  ];

  meta = with pkgs.lib; {
    description = "Logos Chat Module - Provides encrypted chat communication via delivery module";
    platforms = platforms.unix;
  };
}
