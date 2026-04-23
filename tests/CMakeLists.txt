cmake_minimum_required(VERSION 3.14)
project(ChatModuleTests LANGUAGES CXX)

include(LogosTest)

logos_test(
    NAME chat_module_tests
    MODULE_SOURCES
        ../src/chat_module_plugin.cpp
    TEST_SOURCES
        main.cpp
        test_chat_module.cpp
    MOCK_C_SOURCES
        mocks/mock_liblogoschat.cpp
    EXTRA_INCLUDES
        stubs/lib
)
