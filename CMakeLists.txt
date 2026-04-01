cmake_minimum_required(VERSION 3.14)
project(LogosChatModulePlugin LANGUAGES CXX)

# Include the Logos Module CMake helper
if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LogosModule.cmake not found. Set LOGOS_MODULE_BUILDER_ROOT.")
endif()

# Define the module
logos_module(
    NAME chat_module
    SOURCES
        src/chat_module_interface.h
        src/chat_module_plugin.h
        src/chat_module_plugin.cpp
    EXTERNAL_LIBS
        logoschat
    INCLUDE_DIRS
        lib
)
