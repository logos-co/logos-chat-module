cmake_minimum_required(VERSION 3.14)
project(ChatModulePlugin LANGUAGES CXX)

include(GNUInstallDirs)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

option(LOGOS_CHAT_MODULE_USE_VENDOR "Force use of vendored Logos dependencies" OFF)

########### DEPENDENCIES SECTION ###########

if(NOT DEFINED LOGOS_LIBLOGOS_ROOT)
    set(_parent_liblogos "${CMAKE_CURRENT_SOURCE_DIR}/../logos-liblogos")
    set(_use_vendor ${LOGOS_CHAT_MODULE_USE_VENDOR})
    if(NOT _use_vendor)
        if(NOT EXISTS "${_parent_liblogos}/interface.h")
            set(_use_vendor ON)
        endif()
    endif()
    if(_use_vendor)
        set(LOGOS_LIBLOGOS_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/vendor/logos-liblogos")
    else()
        set(LOGOS_LIBLOGOS_ROOT "${_parent_liblogos}")
    endif()
endif()

if(NOT DEFINED LOGOS_CPP_SDK_ROOT)
    set(_parent_cpp_sdk "${CMAKE_CURRENT_SOURCE_DIR}/../logos-cpp-sdk")
    set(_use_vendor ${LOGOS_CHAT_MODULE_USE_VENDOR})
    if(NOT _use_vendor)
        if(NOT EXISTS "${_parent_cpp_sdk}/cpp/logos_api.h")
            set(_use_vendor ON)
        endif()
    endif()
    if(_use_vendor)
        set(LOGOS_CPP_SDK_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/vendor/logos-cpp-sdk")
    else()
        set(LOGOS_CPP_SDK_ROOT "${_parent_cpp_sdk}")
    endif()
endif()

if(NOT DEFINED LOGOS_DELIVERY_ROOT)
    set(_parent_delivery "${CMAKE_CURRENT_SOURCE_DIR}/../logos-delivery-module")
    set(_use_vendor ${LOGOS_CHAT_MODULE_USE_VENDOR})
    if(NOT _use_vendor)
        if(NOT EXISTS "${_parent_delivery}/delivery_module_plugin.h")
            set(_use_vendor ON)
        endif()
    endif()
    if(_use_vendor)
        set(LOGOS_DELIVERY_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/vendor/logos-delivery-module")
    else()
        set(LOGOS_DELIVERY_ROOT "${_parent_delivery}")
    endif()
endif()

# Check if dependencies are available (support both source and installed layouts)
set(_liblogos_found FALSE)
if(EXISTS "${LOGOS_LIBLOGOS_ROOT}/interface.h")
    set(_liblogos_found TRUE)
    set(_liblogos_is_source TRUE)
elseif(EXISTS "${LOGOS_LIBLOGOS_ROOT}/include/interface.h")
    set(_liblogos_found TRUE)
    set(_liblogos_is_source FALSE)
endif()

set(_cpp_sdk_found FALSE)
if(EXISTS "${LOGOS_CPP_SDK_ROOT}/cpp/logos_api.h")
    set(_cpp_sdk_found TRUE)
    set(_cpp_sdk_is_source TRUE)
elseif(EXISTS "${LOGOS_CPP_SDK_ROOT}/include/cpp/logos_api.h")
    set(_cpp_sdk_found TRUE)
    set(_cpp_sdk_is_source FALSE)
endif()

set(_delivery_module_found FALSE)
if(EXISTS "${LOGOS_DELIVERY_ROOT}/delivery_module_plugin.h")
    set(_delivery_module_found TRUE)
    set(_delivery_module_is_source TRUE)
elseif(EXISTS "${LOGOS_DELIVERY_ROOT}/include/delivery_module_api.h")
    set(_delivery_module_found TRUE)
    set(_delivery_module_is_source FALSE)
endif()

if(NOT _liblogos_found)
    message(FATAL_ERROR "logos-liblogos not found at ${LOGOS_LIBLOGOS_ROOT}. "
                        "Set LOGOS_LIBLOGOS_ROOT or run git submodule update --init --recursive.")
endif()

if(NOT _cpp_sdk_found)
    message(FATAL_ERROR "logos-cpp-sdk not found at ${LOGOS_CPP_SDK_ROOT}. "
                        "Set LOGOS_CPP_SDK_ROOT or run git submodule update --init --recursive.")
endif()

if(NOT _delivery_module_found)
    message(FATAL_ERROR "logos-delivery-module not found at ${LOGOS_DELIVERY_ROOT}. "
                        "Set LOGOS_DELIVERY_ROOT or check that the module is available.")
endif()

# Root that contains the vendored dependencies
get_filename_component(LOGOS_DEPS_ROOT "${LOGOS_CPP_SDK_ROOT}" DIRECTORY)

########### QT SECTION ###########

if(NOT DEFINED QT_VERSION_MAJOR)
    find_package(QT NAMES Qt6 Qt5 REQUIRED COMPONENTS Core RemoteObjects)
    if(Qt6_FOUND)
        set(QT_VERSION_MAJOR 6)
    else()
        set(QT_VERSION_MAJOR 5)
    endif()
endif()
find_package(Qt${QT_VERSION_MAJOR} REQUIRED COMPONENTS Core RemoteObjects)

########### CPP GENERATOR ###########

set(METADATA_JSON "${CMAKE_CURRENT_SOURCE_DIR}/metadata.json")
set(PLUGINS_OUTPUT_DIR "${CMAKE_BINARY_DIR}/generated_code")

# If the delivery module is from source, run the cpp generator to produce delivery_module proxy
if(_delivery_module_is_source)
    set(CPP_GENERATOR_BUILD_DIR "${LOGOS_DEPS_ROOT}/build/cpp-generator")
    set(CPP_GENERATOR "${CPP_GENERATOR_BUILD_DIR}/bin/logos-cpp-generator")

    if(NOT TARGET cpp_generator_build)
        add_custom_target(cpp_generator_build
            COMMAND bash "${LOGOS_CPP_SDK_ROOT}/cpp-generator/compile.sh"
            WORKING_DIRECTORY "${LOGOS_DEPS_ROOT}"
            COMMENT "Building logos-cpp-generator"
            VERBATIM
        )
    endif()

    # Build the delivery module first to get the plugin .dylib/.so
    add_custom_target(run_build_delivery_module
        COMMAND "${CMAKE_COMMAND}" -S "${LOGOS_DELIVERY_ROOT}" -B "${LOGOS_DELIVERY_ROOT}/build"
        COMMAND "${CMAKE_COMMAND}" --build "${LOGOS_DELIVERY_ROOT}/build" --target all
        WORKING_DIRECTORY "${LOGOS_DELIVERY_ROOT}/.."
        COMMENT "Building delivery module in ${LOGOS_DELIVERY_ROOT}/build"
        VERBATIM
    )

    if(APPLE)
        set(DELIVERY_PLUGIN_FILE "${LOGOS_DELIVERY_ROOT}/build/modules/delivery_module_plugin.dylib")
    elseif(UNIX)
        set(DELIVERY_PLUGIN_FILE "${LOGOS_DELIVERY_ROOT}/build/modules/delivery_module_plugin.so")
    else()
        message(FATAL_ERROR "delivery_module_plugin not found.")
    endif()

    # Generate delivery module proxy (--module-only)
    add_custom_target(run_cpp_generator_delivery_module
        COMMAND "${CPP_GENERATOR}" "${DELIVERY_PLUGIN_FILE}"
            --interface "${LOGOS_DELIVERY_ROOT}/delivery_module_interface.h"
            --module-only
            --output-dir "${PLUGINS_OUTPUT_DIR}"
        WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
        COMMENT "Running logos-cpp-generator on delivery module"
        VERBATIM
    )
    add_dependencies(run_cpp_generator_delivery_module cpp_generator_build)
    add_dependencies(run_cpp_generator_delivery_module run_build_delivery_module)
endif()

# Generate logos_sdk.cpp (--general-only) from our metadata
if(_cpp_sdk_is_source)
    if(NOT DEFINED CPP_GENERATOR_BUILD_DIR)
        set(CPP_GENERATOR_BUILD_DIR "${LOGOS_DEPS_ROOT}/build/cpp-generator")
    endif()
    if(NOT DEFINED CPP_GENERATOR)
        set(CPP_GENERATOR "${CPP_GENERATOR_BUILD_DIR}/bin/logos-cpp-generator")
    endif()

    if(NOT TARGET cpp_generator_build)
        add_custom_target(cpp_generator_build
            COMMAND bash "${LOGOS_CPP_SDK_ROOT}/cpp-generator/compile.sh"
            WORKING_DIRECTORY "${LOGOS_DEPS_ROOT}"
            COMMENT "Building logos-cpp-generator"
            VERBATIM
        )
    endif()

    add_custom_command(
        OUTPUT "${PLUGINS_OUTPUT_DIR}/logos_sdk.cpp"
        COMMAND ${CMAKE_COMMAND} -E make_directory "${PLUGINS_OUTPUT_DIR}"
        COMMAND "${CPP_GENERATOR}" --metadata "${METADATA_JSON}" --general-only --output-dir "${PLUGINS_OUTPUT_DIR}"
        WORKING_DIRECTORY "${LOGOS_DEPS_ROOT}"
        DEPENDS "${METADATA_JSON}" cpp_generator_build
        VERBATIM
    )

    add_custom_target(run_cpp_generator_chat DEPENDS "${PLUGINS_OUTPUT_DIR}/logos_sdk.cpp")
else()
    set(PLUGINS_OUTPUT_DIR "${CMAKE_CURRENT_SOURCE_DIR}/generated_code")
endif()

########### LIBCHAT ###########

# libchat root: set via -DLOGOS_CHAT_ROOT=<path>
if(NOT DEFINED LOGOS_CHAT_ROOT)
    message(FATAL_ERROR "LOGOS_CHAT_ROOT is not set. "
                        "Pass -DLOGOS_CHAT_ROOT=<path> pointing to the libchat build output.")
endif()

if(NOT EXISTS "${LOGOS_CHAT_ROOT}/include/libchat.h")
    message(FATAL_ERROR "libchat.h not found at ${LOGOS_CHAT_ROOT}/include/. "
                        "Ensure LOGOS_CHAT_ROOT points to a valid libchat build output.")
endif()

set(LIBCHAT_DIR "${LOGOS_CHAT_ROOT}/lib")
if(APPLE)
    set(LIBCHAT_NAMES liblibchat.dylib liblibchat.so)
elseif(WIN32)
    set(LIBCHAT_NAMES liblibchat.dll liblibchat.dylib liblibchat.so)
else()
    set(LIBCHAT_NAMES liblibchat.so)
endif()
find_library(LIBCHAT_PATH NAMES ${LIBCHAT_NAMES} PATHS ${LIBCHAT_DIR} NO_DEFAULT_PATH)

########### SOURCES ###########

set(PLUGIN_SOURCES
    chat_module_plugin.cpp
    chat_module_plugin.h
    chat_module_interface.h
)

# Add liblogos interface header
if(_liblogos_is_source)
    list(APPEND PLUGIN_SOURCES ${LOGOS_LIBLOGOS_ROOT}/interface.h)
else()
    list(APPEND PLUGIN_SOURCES ${LOGOS_LIBLOGOS_ROOT}/include/interface.h)
endif()

# Add SDK sources (only if source layout)
if(_cpp_sdk_is_source)
    list(APPEND PLUGIN_SOURCES
        ${LOGOS_CPP_SDK_ROOT}/cpp/logos_api.cpp
        ${LOGOS_CPP_SDK_ROOT}/cpp/logos_api.h
        ${LOGOS_CPP_SDK_ROOT}/cpp/logos_api_client.cpp
        ${LOGOS_CPP_SDK_ROOT}/cpp/logos_api_client.h
        ${LOGOS_CPP_SDK_ROOT}/cpp/logos_api_consumer.cpp
        ${LOGOS_CPP_SDK_ROOT}/cpp/logos_api_consumer.h
        ${LOGOS_CPP_SDK_ROOT}/cpp/logos_api_provider.cpp
        ${LOGOS_CPP_SDK_ROOT}/cpp/logos_api_provider.h
        ${LOGOS_CPP_SDK_ROOT}/cpp/token_manager.cpp
        ${LOGOS_CPP_SDK_ROOT}/cpp/token_manager.h
        ${LOGOS_CPP_SDK_ROOT}/cpp/module_proxy.cpp
        ${LOGOS_CPP_SDK_ROOT}/cpp/module_proxy.h
    )
endif()

# Add generated logos_sdk.cpp
list(APPEND PLUGIN_SOURCES ${PLUGINS_OUTPUT_DIR}/logos_sdk.cpp)
if(_cpp_sdk_is_source)
    set_source_files_properties(
        ${PLUGINS_OUTPUT_DIR}/logos_sdk.cpp
        PROPERTIES GENERATED TRUE
    )
endif()

########### LIBRARY ###########

add_library(chat_module_plugin SHARED ${PLUGIN_SOURCES})

set_target_properties(chat_module_plugin PROPERTIES PREFIX "")

# Ensure generators run before building
if(_cpp_sdk_is_source)
    add_dependencies(chat_module_plugin run_cpp_generator_chat)
endif()
if(_delivery_module_is_source)
    add_dependencies(chat_module_plugin run_cpp_generator_delivery_module)
endif()

# Link Qt
target_link_libraries(chat_module_plugin PRIVATE
    Qt${QT_VERSION_MAJOR}::Core
    Qt${QT_VERSION_MAJOR}::RemoteObjects
)

# Link SDK library if using installed layout
if(NOT _cpp_sdk_is_source)
    find_library(LOGOS_SDK_LIB logos_sdk PATHS ${LOGOS_CPP_SDK_ROOT}/lib NO_DEFAULT_PATH REQUIRED)
    target_link_libraries(chat_module_plugin PRIVATE ${LOGOS_SDK_LIB})
endif()

# Link to libchat
if(LIBCHAT_PATH)
    target_link_libraries(chat_module_plugin PRIVATE ${LIBCHAT_PATH})
else()
    message(WARNING "libchat not found in ${LOGOS_CHAT_ROOT}/lib/. Check LOGOS_CHAT_ROOT.")
endif()

# Include directories
target_include_directories(chat_module_plugin PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
    ${LOGOS_CHAT_ROOT}/include
    ${PLUGINS_OUTPUT_DIR}
)

if(_liblogos_is_source)
    target_include_directories(chat_module_plugin PRIVATE ${LOGOS_LIBLOGOS_ROOT})
else()
    target_include_directories(chat_module_plugin PRIVATE ${LOGOS_LIBLOGOS_ROOT}/include)
endif()

if(_cpp_sdk_is_source)
    target_include_directories(chat_module_plugin PRIVATE
        ${LOGOS_CPP_SDK_ROOT}/cpp
        ${LOGOS_CPP_SDK_ROOT}/cpp/generated
    )
else()
    target_include_directories(chat_module_plugin PRIVATE
        ${LOGOS_CPP_SDK_ROOT}/include
        ${LOGOS_CPP_SDK_ROOT}/include/cpp
        ${LOGOS_CPP_SDK_ROOT}/include/core
        ${PLUGINS_OUTPUT_DIR}/include
    )
endif()

########### OUTPUT ###########

set_target_properties(chat_module_plugin PROPERTIES
    LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/modules"
    RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/modules"
    BUILD_WITH_INSTALL_RPATH TRUE
    SKIP_BUILD_RPATH FALSE)

if(APPLE)
    target_link_options(chat_module_plugin PRIVATE -undefined dynamic_lookup)
    set_target_properties(chat_module_plugin PROPERTIES
        INSTALL_RPATH "@loader_path"
        INSTALL_NAME_DIR "@rpath"
        BUILD_WITH_INSTALL_NAME_DIR TRUE)

    if(LIBCHAT_PATH)
        get_filename_component(LIBCHAT_FILENAME "${LIBCHAT_PATH}" NAME)
        add_custom_command(TARGET chat_module_plugin PRE_LINK
            COMMAND ${CMAKE_COMMAND} -E copy_if_different
                ${LIBCHAT_PATH}
                ${CMAKE_BINARY_DIR}/modules/${LIBCHAT_FILENAME}
            COMMENT "Copying ${LIBCHAT_FILENAME} to modules directory"
        )
        add_custom_command(TARGET chat_module_plugin POST_BUILD
            COMMAND install_name_tool -id "@rpath/chat_module_plugin.dylib" $<TARGET_FILE:chat_module_plugin>
            COMMAND install_name_tool -id "@rpath/${LIBCHAT_FILENAME}" "${CMAKE_BINARY_DIR}/modules/${LIBCHAT_FILENAME}"
            COMMAND install_name_tool -change "${LIBCHAT_PATH}" "@rpath/${LIBCHAT_FILENAME}" $<TARGET_FILE:chat_module_plugin>
            COMMENT "Updating library paths for macOS"
        )
    else()
        add_custom_command(TARGET chat_module_plugin POST_BUILD
            COMMAND install_name_tool -id "@rpath/chat_module_plugin.dylib" $<TARGET_FILE:chat_module_plugin>
            COMMENT "Updating library paths for macOS (libchat not found)"
        )
    endif()
else()
    set_target_properties(chat_module_plugin PROPERTIES
        INSTALL_RPATH "$ORIGIN"
        INSTALL_RPATH_USE_LINK_PATH FALSE)

    if(LIBCHAT_PATH)
        get_filename_component(LIBCHAT_FILENAME "${LIBCHAT_PATH}" NAME)
        add_custom_command(TARGET chat_module_plugin PRE_LINK
            COMMAND ${CMAKE_COMMAND} -E copy_if_different
                ${LIBCHAT_PATH}
                ${CMAKE_BINARY_DIR}/modules/${LIBCHAT_FILENAME}
            COMMENT "Copying ${LIBCHAT_FILENAME} to modules directory"
        )
    endif()
endif()

########### INSTALL ###########

install(TARGETS chat_module_plugin
    LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}/logos/modules
    RUNTIME DESTINATION ${CMAKE_INSTALL_LIBDIR}/logos/modules
    ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}/logos/modules
)

install(FILES ${METADATA_JSON}
    DESTINATION ${CMAKE_INSTALL_DATADIR}/logos-chat-module
)

install(DIRECTORY "${PLUGINS_OUTPUT_DIR}/"
    DESTINATION ${CMAKE_INSTALL_DATADIR}/logos-chat-module/generated
    OPTIONAL
)
