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
)

# ── Rust static library ───────────────────────────────────────────────────────
# The flake's preConfigure stages libchat_module.a into lib/; the cargo target
# dir is also searched for convenience when developing outside Nix.
find_library(LIBCHAT_MODULE
    NAMES libchat_module.a chat_module
    PATHS
        ${CMAKE_CURRENT_SOURCE_DIR}/lib
        ${CMAKE_CURRENT_SOURCE_DIR}/rust-lib/target/release
        ${CMAKE_CURRENT_SOURCE_DIR}/rust-lib/target/debug
    NO_DEFAULT_PATH
)

if(NOT LIBCHAT_MODULE)
    message(FATAL_ERROR
        "Rust static library (libchat_module.a) not found in ${CMAKE_CURRENT_SOURCE_DIR}/lib. "
        "Ensure the preConfigure hook staged the buildRustPackage output."
    )
endif()

message(STATUS "Found chat_module Rust static library: ${LIBCHAT_MODULE}")

# The Rust archive provides the logos_module_* exports the generated glue
# calls; its own lp_* undefineds resolve against the logos-protocol archive
# already in the plugin link (via logos-qt-sdk). Re-mention the protocol
# target after the Rust archive so single-pass linkers (GNU ld) see it later
# on the line — one protocol stack shared by the glue and the Rust code.
target_link_libraries(chat_module_module_plugin PRIVATE
    ${LIBCHAT_MODULE}
    logos-protocol::logos_protocol
)

if(APPLE)
    target_link_libraries(chat_module_module_plugin PRIVATE
        "-framework CoreFoundation"
        "-framework Security"
    )
else()
    target_link_libraries(chat_module_module_plugin PRIVATE pthread dl)
endif()
