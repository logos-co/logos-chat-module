#include "chat_module_plugin.h"
#include <cstdio>
#include <cstring>
#include <chrono>
#include <string>
#include <utility>
#include <nlohmann/json.hpp>
#include <QMetaObject>

namespace {
// Post a typed-event emit through the plugin host's event loop so it
// fires after the current synchronous stack unwinds.
//
// Every libchat callback (init/start/stop/destroy/get_id/...) is invoked
// synchronously from inside the corresponding libchat C API call (e.g.
// chat_new, chat_start), which is itself invoked synchronously from a
// LOGOS_METHOD on ChatModuleImpl. The plugin host's Q_INVOKABLE slot
// is still on the stack at that point; the host thread cannot reach
// the next event-loop iteration (which is when QLocalSocket flushes
// the QRO reply packet) until every synchronous emit has unwound.
// On slow runners (GHA ubuntu-latest) the resulting flush starvation
// exceeds the caller's 20s waitForFinished deadline → exit 4.
//
// Deferring via QueuedConnection lets the Q_INVOKABLE slot return
// promptly; the emit runs on the next event-loop iteration after the
// reply has been flushed, restoring normal ordering.
//
// Receiver is `impl->emitRouter()` (a QObject member of `impl`). When
// the impl is destroyed the router is destroyed too, and Qt drops any
// pending queued metacall — so the captured raw `impl` pointer in the
// emit closure cannot be dereferenced after free.
template <typename EmitFn>
void deferredEmit(ChatModuleImpl* impl, EmitFn&& emitFn)
{
    QMetaObject::invokeMethod(
        impl->emitRouter(),
        std::forward<EmitFn>(emitFn),
        Qt::QueuedConnection);
}
}  // namespace

static std::string isoTimestamp()
{
    auto now = std::chrono::system_clock::now();
    auto tt = std::chrono::system_clock::to_time_t(now);
    struct tm buf;
    gmtime_r(&tt, &buf);
    char out[32];
    strftime(out, sizeof(out), "%Y-%m-%dT%H:%M:%SZ", &buf);
    return out;
}

ChatModuleImpl::ChatModuleImpl() : chatCtx(nullptr)
{
    fprintf(stderr, "ChatModuleImpl: Initializing...\n");
    fprintf(stderr, "ChatModuleImpl: Initialized successfully\n");
}

ChatModuleImpl::~ChatModuleImpl()
{
    if (chatCtx) {
        chat_destroy(chatCtx, destroy_callback, this);
        chatCtx = nullptr;
    }
}

// ============================================================================
// Static Callback Functions
// ============================================================================

void ChatModuleImpl::init_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::init_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::init_callback: Invalid userData\n");
        return;
    }

    std::string message = (msg && len > 0) ? std::string(msg, len) : "";
    bool success = (callerRet == RET_OK);

    deferredEmit(impl, [impl, success, callerRet, message, ts = isoTimestamp()]() {
        impl->chatInitResult(success, callerRet, message, ts);
    });
}

void ChatModuleImpl::start_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::start_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::start_callback: Invalid userData\n");
        return;
    }

    std::string message = (msg && len > 0) ? std::string(msg, len) : "";
    bool success = (callerRet == RET_OK);

    deferredEmit(impl, [impl, success, callerRet, message, ts = isoTimestamp()]() {
        impl->chatStartResult(success, callerRet, message, ts);
    });
}

void ChatModuleImpl::stop_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::stop_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::stop_callback: Invalid userData\n");
        return;
    }

    std::string message = (msg && len > 0) ? std::string(msg, len) : "";
    bool success = (callerRet == RET_OK);

    deferredEmit(impl, [impl, success, callerRet, message, ts = isoTimestamp()]() {
        impl->chatStopResult(success, callerRet, message, ts);
    });
}

void ChatModuleImpl::destroy_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::destroy_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::destroy_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);
        fprintf(stderr, "ChatModuleImpl::destroy_callback message: %s\n", message.c_str());

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatDestroyResult(message, ts);
        });
    }
}

void ChatModuleImpl::event_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::event_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::event_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        std::string eventType;
        try {
            auto doc = nlohmann::json::parse(message);
            if (doc.contains("eventType") && doc["eventType"].is_string())
                eventType = doc["eventType"].get<std::string>();
        } catch (...) {
            // parse failed, fall through to the generic chatEvent
        }

        deferredEmit(impl, [impl, eventType, message, ts = isoTimestamp()]() {
            if (eventType == "new_message")
                impl->chatNewMessage(message, ts);
            else if (eventType == "new_conversation")
                impl->chatNewConversation(message, ts);
            else if (eventType == "delivery_ack")
                impl->chatDeliveryAck(message, ts);
            else
                impl->chatEvent(message, ts);
        });
    }
}

void ChatModuleImpl::get_id_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::get_id_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::get_id_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatGetIdResult(message, ts);
        });
    }
}

void ChatModuleImpl::get_mix_status_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::get_mix_status_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::get_mix_status_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatGetMixStatusResult(message, ts);
        });
    }
}

void ChatModuleImpl::list_conversations_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::list_conversations_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::list_conversations_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatListConversationsResult(message, ts);
        });
    }
}

void ChatModuleImpl::get_conversation_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::get_conversation_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::get_conversation_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatGetConversationResult(message, ts);
        });
    }
}

void ChatModuleImpl::new_private_conversation_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::new_private_conversation_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::new_private_conversation_callback: Invalid userData\n");
        return;
    }

    std::string conversationJson = (msg && len > 0) ? std::string(msg, len) : "";
    bool success = (callerRet == RET_OK && !conversationJson.empty());

    deferredEmit(impl, [impl, success, callerRet, conversationJson, ts = isoTimestamp()]() {
        impl->chatNewPrivateConversationResult(success, callerRet, conversationJson, ts);
    });
}

void ChatModuleImpl::send_message_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::send_message_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::send_message_callback: Invalid userData\n");
        return;
    }

    std::string resultJson = (msg && len > 0) ? std::string(msg, len) : "";
    fprintf(stderr, "ChatModuleImpl::send_message_callback result: %s\n", resultJson.c_str());
    bool success = (callerRet == RET_OK);

    deferredEmit(impl, [impl, success, callerRet, resultJson, ts = isoTimestamp()]() {
        impl->chatSendMessageResult(success, callerRet, resultJson, ts);
    });
}

void ChatModuleImpl::get_identity_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::get_identity_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::get_identity_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatGetIdentityResult(message, ts);
        });
    }
}

void ChatModuleImpl::create_intro_bundle_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleImpl::create_intro_bundle_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleImpl::create_intro_bundle_callback: Invalid userData\n");
        return;
    }

    std::string bundleStr = (msg && len > 0) ? std::string(msg, len) : "";
    bool success = (callerRet == RET_OK && !bundleStr.empty());

    deferredEmit(impl, [impl, success, callerRet, bundleStr, ts = isoTimestamp()]() {
        impl->chatCreateIntroBundleResult(success, callerRet, bundleStr, ts);
    });
}

// ============================================================================
// Client Lifecycle Methods
// ============================================================================

bool ChatModuleImpl::initChat(const std::string& configJson)
{
    fprintf(stderr, "ChatModuleImpl::initChat called with config: %s\n", configJson.c_str());

    chatCtx = chat_new(configJson.c_str(), init_callback, this);

    if (chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Chat context created successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to create Chat context\n");
        return false;
    }
}

bool ChatModuleImpl::startChat()
{
    fprintf(stderr, "ChatModuleImpl::startChat called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot start Chat - context not initialized. Call initChat first.\n");
        return false;
    }

    int result = chat_start(chatCtx, start_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: Chat start initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to start Chat, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleImpl::stopChat()
{
    fprintf(stderr, "ChatModuleImpl::stopChat called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot stop Chat - context not initialized.\n");
        return false;
    }

    int result = chat_stop(chatCtx, stop_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: Chat stop initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to stop Chat, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleImpl::destroyChat()
{
    fprintf(stderr, "ChatModuleImpl::destroyChat called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot destroy Chat - context not initialized.\n");
        return false;
    }

    int result = chat_destroy(chatCtx, destroy_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: Chat destroy initiated successfully\n");
        chatCtx = nullptr;
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to destroy Chat, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleImpl::setEventCallback()
{
    fprintf(stderr, "ChatModuleImpl::setEventCallback called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot set event callback - context not initialized. Call initChat first.\n");
        return false;
    }

    set_event_callback(chatCtx, event_callback, this);

    fprintf(stderr, "ChatModuleImpl: Event callback set successfully\n");
    return true;
}

// ============================================================================
// Client Info Methods
// ============================================================================

bool ChatModuleImpl::getId()
{
    fprintf(stderr, "ChatModuleImpl::getId called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot get ID - context not initialized\n");
        return false;
    }

    int result = chat_get_id(chatCtx, get_id_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: Get ID initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to get ID, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleImpl::getMixStatus()
{
    fprintf(stderr, "ChatModuleImpl::getMixStatus called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot get mix status - context not initialized\n");
        return false;
    }

    int result = chat_get_mix_status(chatCtx, get_mix_status_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: Get mix status initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to get mix status, error code: %d\n", result);
        return false;
    }
}

// ============================================================================
// Conversation Operations
// ============================================================================

bool ChatModuleImpl::listConversations()
{
    fprintf(stderr, "ChatModuleImpl::listConversations called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot list conversations - context not initialized\n");
        return false;
    }

    int result = chat_list_conversations(chatCtx, list_conversations_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: List conversations initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to list conversations, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleImpl::getConversation(const std::string& convoId)
{
    fprintf(stderr, "ChatModuleImpl::getConversation called with convoId: %s\n", convoId.c_str());

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot get conversation - context not initialized\n");
        return false;
    }

    int result = chat_get_conversation(chatCtx, get_conversation_callback, this, convoId.c_str());

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: Get conversation initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to get conversation, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleImpl::newPrivateConversation(const std::string& introBundleStr, const std::string& contentHex)
{
    fprintf(stderr, "ChatModuleImpl::newPrivateConversation called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot create new private conversation - context not initialized\n");
        return false;
    }

    int result = chat_new_private_conversation(chatCtx, new_private_conversation_callback, this,
                                                introBundleStr.c_str(), contentHex.c_str());

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: New private conversation initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to create new private conversation, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleImpl::sendMessage(const std::string& convoId, const std::string& contentHex)
{
    fprintf(stderr, "ChatModuleImpl::sendMessage called with convoId: %s\n", convoId.c_str());

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot send message - context not initialized\n");
        return false;
    }

    int result = chat_send_message(chatCtx, send_message_callback, this,
                                    convoId.c_str(), contentHex.c_str());

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: Send message initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to send message, error code: %d\n", result);
        return false;
    }
}

// ============================================================================
// Identity Operations
// ============================================================================

bool ChatModuleImpl::getIdentity()
{
    fprintf(stderr, "ChatModuleImpl::getIdentity called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot get identity - context not initialized\n");
        return false;
    }

    int result = chat_get_identity(chatCtx, get_identity_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: Get identity initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to get identity, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleImpl::createIntroBundle()
{
    fprintf(stderr, "ChatModuleImpl::createIntroBundle called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleImpl: Cannot create intro bundle - context not initialized\n");
        return false;
    }

    int result = chat_create_intro_bundle(chatCtx, create_intro_bundle_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleImpl: Create intro bundle initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleImpl: Failed to create intro bundle, error code: %d\n", result);
        return false;
    }
}
