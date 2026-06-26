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
// LOGOS_METHOD on ChatModuleMixImpl. The plugin host's Q_INVOKABLE slot
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
void deferredEmit(ChatModuleMixImpl* impl, EmitFn&& emitFn)
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

ChatModuleMixImpl::ChatModuleMixImpl() : chatCtx(nullptr)
{
    fprintf(stderr, "ChatModuleMixImpl: Initializing...\n");
    fprintf(stderr, "ChatModuleMixImpl: Initialized successfully\n");
}

ChatModuleMixImpl::~ChatModuleMixImpl()
{
    if (chatCtx) {
        chat_destroy(chatCtx, destroy_callback, this);
        chatCtx = nullptr;
    }
}

// ============================================================================
// Static Callback Functions
// ============================================================================

void ChatModuleMixImpl::init_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::init_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::init_callback: Invalid userData\n");
        return;
    }

    std::string message = (msg && len > 0) ? std::string(msg, len) : "";
    bool success = (callerRet == RET_OK);

    deferredEmit(impl, [impl, success, callerRet, message, ts = isoTimestamp()]() {
        impl->chatInitResult(success, callerRet, message, ts);
    });
}

void ChatModuleMixImpl::start_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::start_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::start_callback: Invalid userData\n");
        return;
    }

    std::string message = (msg && len > 0) ? std::string(msg, len) : "";
    bool success = (callerRet == RET_OK);

    deferredEmit(impl, [impl, success, callerRet, message, ts = isoTimestamp()]() {
        impl->chatStartResult(success, callerRet, message, ts);
    });
}

void ChatModuleMixImpl::stop_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::stop_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::stop_callback: Invalid userData\n");
        return;
    }

    std::string message = (msg && len > 0) ? std::string(msg, len) : "";
    bool success = (callerRet == RET_OK);

    deferredEmit(impl, [impl, success, callerRet, message, ts = isoTimestamp()]() {
        impl->chatStopResult(success, callerRet, message, ts);
    });
}

void ChatModuleMixImpl::destroy_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::destroy_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::destroy_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);
        fprintf(stderr, "ChatModuleMixImpl::destroy_callback message: %s\n", message.c_str());

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatDestroyResult(message, ts);
        });
    }
}

void ChatModuleMixImpl::event_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::event_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::event_callback: Invalid userData\n");
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

void ChatModuleMixImpl::get_id_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::get_id_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::get_id_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatGetIdResult(message, ts);
        });
    }
}

void ChatModuleMixImpl::get_mix_status_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::get_mix_status_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::get_mix_status_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatGetMixStatusResult(message, ts);
        });
    }
}

void ChatModuleMixImpl::list_conversations_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::list_conversations_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::list_conversations_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatListConversationsResult(message, ts);
        });
    }
}

void ChatModuleMixImpl::get_conversation_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::get_conversation_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::get_conversation_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatGetConversationResult(message, ts);
        });
    }
}

void ChatModuleMixImpl::new_private_conversation_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::new_private_conversation_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::new_private_conversation_callback: Invalid userData\n");
        return;
    }

    std::string conversationJson = (msg && len > 0) ? std::string(msg, len) : "";
    bool success = (callerRet == RET_OK && !conversationJson.empty());

    deferredEmit(impl, [impl, success, callerRet, conversationJson, ts = isoTimestamp()]() {
        impl->chatNewPrivateConversationResult(success, callerRet, conversationJson, ts);
    });
}

void ChatModuleMixImpl::send_message_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::send_message_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::send_message_callback: Invalid userData\n");
        return;
    }

    std::string resultJson = (msg && len > 0) ? std::string(msg, len) : "";
    fprintf(stderr, "ChatModuleMixImpl::send_message_callback result: %s\n", resultJson.c_str());
    bool success = (callerRet == RET_OK);

    deferredEmit(impl, [impl, success, callerRet, resultJson, ts = isoTimestamp()]() {
        impl->chatSendMessageResult(success, callerRet, resultJson, ts);
    });
}

void ChatModuleMixImpl::get_identity_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::get_identity_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::get_identity_callback: Invalid userData\n");
        return;
    }

    if (msg && len > 0) {
        std::string message(msg, len);

        deferredEmit(impl, [impl, message, ts = isoTimestamp()]() {
            impl->chatGetIdentityResult(message, ts);
        });
    }
}

void ChatModuleMixImpl::create_intro_bundle_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    fprintf(stderr, "ChatModuleMixImpl::create_intro_bundle_callback called with ret: %d\n", callerRet);

    auto* impl = static_cast<ChatModuleMixImpl*>(userData);
    if (!impl) {
        fprintf(stderr, "ChatModuleMixImpl::create_intro_bundle_callback: Invalid userData\n");
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

bool ChatModuleMixImpl::initChat(const std::string& configJson)
{
    fprintf(stderr, "ChatModuleMixImpl::initChat called with config: %s\n", configJson.c_str());

    chatCtx = chat_new(configJson.c_str(), init_callback, this);

    if (chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Chat context created successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to create Chat context\n");
        return false;
    }
}

bool ChatModuleMixImpl::startChat()
{
    fprintf(stderr, "ChatModuleMixImpl::startChat called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot start Chat - context not initialized. Call initChat first.\n");
        return false;
    }

    int result = chat_start(chatCtx, start_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: Chat start initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to start Chat, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleMixImpl::stopChat()
{
    fprintf(stderr, "ChatModuleMixImpl::stopChat called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot stop Chat - context not initialized.\n");
        return false;
    }

    int result = chat_stop(chatCtx, stop_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: Chat stop initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to stop Chat, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleMixImpl::destroyChat()
{
    fprintf(stderr, "ChatModuleMixImpl::destroyChat called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot destroy Chat - context not initialized.\n");
        return false;
    }

    int result = chat_destroy(chatCtx, destroy_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: Chat destroy initiated successfully\n");
        chatCtx = nullptr;
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to destroy Chat, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleMixImpl::setEventCallback()
{
    fprintf(stderr, "ChatModuleMixImpl::setEventCallback called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot set event callback - context not initialized. Call initChat first.\n");
        return false;
    }

    set_event_callback(chatCtx, event_callback, this);

    fprintf(stderr, "ChatModuleMixImpl: Event callback set successfully\n");
    return true;
}

// ============================================================================
// Client Info Methods
// ============================================================================

bool ChatModuleMixImpl::getId()
{
    fprintf(stderr, "ChatModuleMixImpl::getId called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot get ID - context not initialized\n");
        return false;
    }

    int result = chat_get_id(chatCtx, get_id_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: Get ID initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to get ID, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleMixImpl::getMixStatus()
{
    fprintf(stderr, "ChatModuleMixImpl::getMixStatus called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot get mix status - context not initialized\n");
        return false;
    }

    int result = chat_get_mix_status(chatCtx, get_mix_status_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: Get mix status initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to get mix status, error code: %d\n", result);
        return false;
    }
}

// ============================================================================
// Conversation Operations
// ============================================================================

bool ChatModuleMixImpl::listConversations()
{
    fprintf(stderr, "ChatModuleMixImpl::listConversations called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot list conversations - context not initialized\n");
        return false;
    }

    int result = chat_list_conversations(chatCtx, list_conversations_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: List conversations initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to list conversations, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleMixImpl::getConversation(const std::string& convoId)
{
    fprintf(stderr, "ChatModuleMixImpl::getConversation called with convoId: %s\n", convoId.c_str());

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot get conversation - context not initialized\n");
        return false;
    }

    int result = chat_get_conversation(chatCtx, get_conversation_callback, this, convoId.c_str());

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: Get conversation initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to get conversation, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleMixImpl::newPrivateConversation(const std::string& introBundleStr, const std::string& contentHex)
{
    fprintf(stderr, "ChatModuleMixImpl::newPrivateConversation called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot create new private conversation - context not initialized\n");
        return false;
    }

    int result = chat_new_private_conversation(chatCtx, new_private_conversation_callback, this,
                                                introBundleStr.c_str(), contentHex.c_str());

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: New private conversation initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to create new private conversation, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleMixImpl::sendMessage(const std::string& convoId, const std::string& contentHex)
{
    fprintf(stderr, "ChatModuleMixImpl::sendMessage called with convoId: %s\n", convoId.c_str());

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot send message - context not initialized\n");
        return false;
    }

    int result = chat_send_message(chatCtx, send_message_callback, this,
                                    convoId.c_str(), contentHex.c_str());

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: Send message initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to send message, error code: %d\n", result);
        return false;
    }
}

// ============================================================================
// Identity Operations
// ============================================================================

bool ChatModuleMixImpl::getIdentity()
{
    fprintf(stderr, "ChatModuleMixImpl::getIdentity called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot get identity - context not initialized\n");
        return false;
    }

    int result = chat_get_identity(chatCtx, get_identity_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: Get identity initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to get identity, error code: %d\n", result);
        return false;
    }
}

bool ChatModuleMixImpl::createIntroBundle()
{
    fprintf(stderr, "ChatModuleMixImpl::createIntroBundle called\n");

    if (!chatCtx) {
        fprintf(stderr, "ChatModuleMixImpl: Cannot create intro bundle - context not initialized\n");
        return false;
    }

    int result = chat_create_intro_bundle(chatCtx, create_intro_bundle_callback, this);

    if (result == RET_OK) {
        fprintf(stderr, "ChatModuleMixImpl: Create intro bundle initiated successfully\n");
        return true;
    } else {
        fprintf(stderr, "ChatModuleMixImpl: Failed to create intro bundle, error code: %d\n", result);
        return false;
    }
}
