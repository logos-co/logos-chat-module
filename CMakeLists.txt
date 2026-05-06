cmake_minimum_required(VERSION 3.14)
project(ChatModulePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LogosModule.cmake not found. LOGOS_MODULE_BUILDER_ROOT is not set.")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/metadata.json
               ${CMAKE_CURRENT_BINARY_DIR}/metadata.json COPYONLY)

logos_module(
    NAME chat_module
    INCLUDE_DIRS
        # Canonical header location (also reachable from non-Nix builds).
        ${CMAKE_CURRENT_SOURCE_DIR}/include
        # Where the flake's preConfigure stages the .a; keeps Nix-built
        # binaries finding any auxiliary generated headers in the same dir.
        ${CMAKE_CURRENT_SOURCE_DIR}/lib
)

# ── Rust static library ───────────────────────────────────────────────────────
# Search both `lib/` (Nix preConfigure stage) and the cargo target dir for
# convenience when developing outside Nix.
find_library(LIBCHAT_MODULE
    NAMES libchat_module.a chat_module
    PATHS
        ${CMAKE_CURRENT_SOURCE_DIR}/lib
        ${CMAKE_CURRENT_SOURCE_DIR}/target/release
        ${CMAKE_CURRENT_SOURCE_DIR}/target/debug
    NO_DEFAULT_PATH
)

if(NOT LIBCHAT_MODULE)
    message(FATAL_ERROR
        "Rust static library (libchat_module.a) not found. "
        "Inside Nix the flake's preConfigure stages it into ${CMAKE_CURRENT_SOURCE_DIR}/lib. "
        "Outside Nix run `cargo build --release` first."
    )
endif()

message(STATUS "Found chat_module Rust static library: ${LIBCHAT_MODULE}")
target_link_libraries(chat_module_module_plugin PRIVATE ${LIBCHAT_MODULE})

# ── logos-module-client ───────────────────────────────────────────────────────
if(NOT DEFINED LOGOS_MODULE_CLIENT_ROOT OR LOGOS_MODULE_CLIENT_ROOT STREQUAL "")
    if(DEFINED ENV{LOGOS_MODULE_CLIENT_ROOT})
        set(LOGOS_MODULE_CLIENT_ROOT "$ENV{LOGOS_MODULE_CLIENT_ROOT}")
    else()
        message(FATAL_ERROR
            "LOGOS_MODULE_CLIENT_ROOT is not set. "
            "Pass -DLOGOS_MODULE_CLIENT_ROOT=<path> or set LOGOS_MODULE_CLIENT_ROOT env var."
        )
    endif()
endif()

get_filename_component(LOGOS_MODULE_CLIENT_ROOT "${LOGOS_MODULE_CLIENT_ROOT}" ABSOLUTE)
message(STATUS "Using logos-module-client at ${LOGOS_MODULE_CLIENT_ROOT}")

find_library(LOGOS_MODULE_CLIENT_LIBRARY
    NAMES logos_module_client
    PATHS "${LOGOS_MODULE_CLIENT_ROOT}/lib"
    NO_DEFAULT_PATH
)

if(NOT LOGOS_MODULE_CLIENT_LIBRARY)
    message(FATAL_ERROR "liblogos_module_client not found in ${LOGOS_MODULE_CLIENT_ROOT}/lib")
endif()

add_library(logos_module_client SHARED IMPORTED)
set_target_properties(logos_module_client PROPERTIES
    IMPORTED_LOCATION "${LOGOS_MODULE_CLIENT_LIBRARY}"
    INTERFACE_INCLUDE_DIRECTORIES "${LOGOS_MODULE_CLIENT_ROOT}/include"
)

target_link_libraries(chat_module_module_plugin PRIVATE logos_module_client)

# ── Platform-specific Rust runtime deps ──────────────────────────────────────
if(APPLE)
    target_link_libraries(chat_module_module_plugin PRIVATE
        "-framework CoreFoundation"
        "-framework Security"
    )
else()
    target_link_libraries(chat_module_module_plugin PRIVATE pthread dl)
endif()
