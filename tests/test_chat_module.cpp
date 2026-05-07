// Unit tests for ChatModuleImpl.
// All liblogoschat C functions are mocked at link time via mock_liblogoschat.cpp.
// Mocks invoke callbacks synchronously so the impl methods get their result
// immediately upon calling the C function.

#include <logos_test.h>
#include "chat_module_plugin.h"

// ---------------------------------------------------------------------------
// Helper: create an impl that has a valid chat context (initChat called).
// ---------------------------------------------------------------------------
static ChatModuleImpl* createInitializedImpl(LogosTestContext& t) {
    t.mockCFunction("chat_new").returns(1);
    auto* impl = new ChatModuleImpl();
    impl->emitEvent = [](const std::string&, const std::string&) {};
    LOGOS_ASSERT_TRUE(impl->initChat(R"({"logLevel":"INFO"})"));
    return impl;
}

// ============================================================================
// initChat
// ============================================================================

LOGOS_TEST(initChat_succeeds_when_ffi_returns_non_null_context) {
    auto t = LogosTestContext("chat_module");
    t.mockCFunction("chat_new").returns(1);

    ChatModuleImpl plugin;
    plugin.emitEvent = [](const std::string&, const std::string&) {};
    LOGOS_ASSERT_TRUE(plugin.initChat(R"({"logLevel":"INFO"})"));
    LOGOS_ASSERT(t.cFunctionCalled("chat_new"));
}

LOGOS_TEST(initChat_fails_when_ffi_returns_null) {
    auto t = LogosTestContext("chat_module");
    t.mockCFunction("chat_new").returns(0);

    ChatModuleImpl plugin;
    plugin.emitEvent = [](const std::string&, const std::string&) {};
    LOGOS_ASSERT_FALSE(plugin.initChat(R"({"logLevel":"INFO"})"));
    LOGOS_ASSERT(t.cFunctionCalled("chat_new"));
}

LOGOS_TEST(initChat_tracks_call_count) {
    auto t = LogosTestContext("chat_module");
    t.mockCFunction("chat_new").returns(1);

    ChatModuleImpl plugin;
    plugin.emitEvent = [](const std::string&, const std::string&) {};
    plugin.initChat(R"({"logLevel":"INFO"})");
    LOGOS_ASSERT_EQ(t.cFunctionCallCount("chat_new"), 1);
}

// ============================================================================
// startChat
// ============================================================================

LOGOS_TEST(startChat_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.startChat());
}

LOGOS_TEST(startChat_succeeds_after_initChat) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->startChat());
    LOGOS_ASSERT(t.cFunctionCalled("chat_start"));

    delete plugin;
}

LOGOS_TEST(startChat_calls_ffi_start) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    plugin->startChat();
    LOGOS_ASSERT_EQ(t.cFunctionCallCount("chat_start"), 1);

    delete plugin;
}

// ============================================================================
// stopChat
// ============================================================================

LOGOS_TEST(stopChat_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.stopChat());
}

LOGOS_TEST(stopChat_succeeds_after_initChat) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->stopChat());
    LOGOS_ASSERT(t.cFunctionCalled("chat_stop"));

    delete plugin;
}

// ============================================================================
// destroyChat
// ============================================================================

LOGOS_TEST(destroyChat_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.destroyChat());
}

LOGOS_TEST(destroyChat_succeeds_after_initChat) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->destroyChat());
    LOGOS_ASSERT(t.cFunctionCalled("chat_destroy"));

    delete plugin;
}

// ============================================================================
// setEventCallback
// ============================================================================

LOGOS_TEST(setEventCallback_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.setEventCallback());
}

LOGOS_TEST(setEventCallback_succeeds_after_initChat) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->setEventCallback());
    LOGOS_ASSERT(t.cFunctionCalled("set_event_callback"));

    delete plugin;
}

// ============================================================================
// getId
// ============================================================================

LOGOS_TEST(getId_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.getId());
}

LOGOS_TEST(getId_succeeds_after_initChat) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->getId());
    LOGOS_ASSERT(t.cFunctionCalled("chat_get_id"));

    delete plugin;
}

// ============================================================================
// listConversations
// ============================================================================

LOGOS_TEST(listConversations_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.listConversations());
}

LOGOS_TEST(listConversations_succeeds_after_initChat) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->listConversations());
    LOGOS_ASSERT(t.cFunctionCalled("chat_list_conversations"));

    delete plugin;
}

// ============================================================================
// getConversation
// ============================================================================

LOGOS_TEST(getConversation_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.getConversation("conv-123"));
}

LOGOS_TEST(getConversation_succeeds_with_convo_id) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->getConversation("conv-123"));
    LOGOS_ASSERT(t.cFunctionCalled("chat_get_conversation"));

    delete plugin;
}

// ============================================================================
// newPrivateConversation
// ============================================================================

LOGOS_TEST(newPrivateConversation_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.newPrivateConversation("bundle-abc", "deadbeef"));
}

LOGOS_TEST(newPrivateConversation_succeeds_with_args) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->newPrivateConversation("bundle-abc", "deadbeef"));
    LOGOS_ASSERT(t.cFunctionCalled("chat_new_private_conversation"));

    delete plugin;
}

// ============================================================================
// sendMessage
// ============================================================================

LOGOS_TEST(sendMessage_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.sendMessage("conv-123", "deadbeef"));
}

LOGOS_TEST(sendMessage_succeeds_with_args) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->sendMessage("conv-123", "deadbeef"));
    LOGOS_ASSERT(t.cFunctionCalled("chat_send_message"));

    delete plugin;
}

LOGOS_TEST(sendMessage_calls_ffi_with_correct_count) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    plugin->sendMessage("conv-1", "aabbcc");
    plugin->sendMessage("conv-2", "ddeeff");
    LOGOS_ASSERT_EQ(t.cFunctionCallCount("chat_send_message"), 2);

    delete plugin;
}

// ============================================================================
// getIdentity
// ============================================================================

LOGOS_TEST(getIdentity_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.getIdentity());
}

LOGOS_TEST(getIdentity_succeeds_after_initChat) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->getIdentity());
    LOGOS_ASSERT(t.cFunctionCalled("chat_get_identity"));

    delete plugin;
}

// ============================================================================
// createIntroBundle
// ============================================================================

LOGOS_TEST(createIntroBundle_fails_without_initChat) {
    auto t = LogosTestContext("chat_module");
    ChatModuleImpl plugin;
    LOGOS_ASSERT_FALSE(plugin.createIntroBundle());
}

LOGOS_TEST(createIntroBundle_succeeds_after_initChat) {
    auto t = LogosTestContext("chat_module");
    auto* plugin = createInitializedImpl(t);

    LOGOS_ASSERT_TRUE(plugin->createIntroBundle());
    LOGOS_ASSERT(t.cFunctionCalled("chat_create_intro_bundle"));

    delete plugin;
}

