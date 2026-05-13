#include "chat_module_plugin.h"
#include <cstdio>
#include <cstring>
#include <chrono>
#include <string>
#include <utility>
#include <nlohmann/json.hpp>
#include <QMetaObject>

namespace {
// Post an emitEvent call through the plugin host's event loop so it
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
// pending queued metacall — so the captured raw `impl` pointer below
// cannot be dereferenced after free.
void deferredEmit(ChatModuleImpl* impl, const char* eventName, std::string payload)
{
    QMetaObject::invokeMethod(
        impl->emitRouter(),
        [impl, name = std::string(eventName), payload = std::move(payload)]() {
            impl->emitEvent(name, payload);
        },
        Qt::QueuedConnection
    );
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

    nlohmann::json ev;
    ev["success"] = (callerRet == RET_OK);
    ev["statusCode"] = callerRet;
    ev["message"] = message;
    ev["timestamp"] = isoTimestamp();

    deferredEmit(impl, "chatInitResult", ev.dump());
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

    nlohmann::json ev;
    ev["success"] = (callerRet == RET_OK);
    ev["statusCode"] = callerRet;
    ev["message"] = message;
    ev["timestamp"] = isoTimestamp();

    deferredEmit(impl, "chatStartResult", ev.dump());
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

    nlohmann::json ev;
    ev["success"] = (callerRet == RET_OK);
    ev["statusCode"] = callerRet;
    ev["message"] = message;
    ev["timestamp"] = isoTimestamp();

    deferredEmit(impl, "chatStopResult", ev.dump());
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

        nlohmann::json ev;
        ev["message"] = message;
        ev["timestamp"] = isoTimestamp();

        deferredEmit(impl, "chatDestroyResult", ev.dump());
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

        std::string eventName = "chatEvent";
        try {
            auto doc = nlohmann::json::parse(message);
            if (doc.contains("eventType") && doc["eventType"].is_string()) {
                std::string eventType = doc["eventType"].get<std::string>();
                if (eventType == "new_message")
                    eventName = "chatNewMessage";
                else if (eventType == "new_conversation")
                    eventName = "chatNewConversation";
                else if (eventType == "delivery_ack")
                    eventName = "chatDeliveryAck";
            }
        } catch (...) {
            // parse failed, keep default eventName
        }

        nlohmann::json ev;
        ev["payload"] = message;
        ev["timestamp"] = isoTimestamp();

        deferredEmit(impl, eventName.c_str(), ev.dump());
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

        nlohmann::json ev;
        ev["clientId"] = message;
        ev["timestamp"] = isoTimestamp();

        deferredEmit(impl, "chatGetIdResult", ev.dump());
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

        nlohmann::json ev;
        ev["conversations"] = message;
        ev["timestamp"] = isoTimestamp();

        deferredEmit(impl, "chatListConversationsResult", ev.dump());
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

        nlohmann::json ev;
        ev["conversation"] = message;
        ev["timestamp"] = isoTimestamp();

        deferredEmit(impl, "chatGetConversationResult", ev.dump());
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

    nlohmann::json ev;
    ev["success"] = (callerRet == RET_OK && !conversationJson.empty());
    ev["statusCode"] = callerRet;
    ev["conversation"] = conversationJson;
    ev["timestamp"] = isoTimestamp();

    deferredEmit(impl, "chatNewPrivateConversationResult", ev.dump());
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

    nlohmann::json ev;
    ev["success"] = (callerRet == RET_OK);
    ev["statusCode"] = callerRet;
    ev["result"] = resultJson;
    ev["timestamp"] = isoTimestamp();

    deferredEmit(impl, "chatSendMessageResult", ev.dump());
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

        nlohmann::json ev;
        ev["identity"] = message;
        ev["timestamp"] = isoTimestamp();

        deferredEmit(impl, "chatGetIdentityResult", ev.dump());
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

    nlohmann::json ev;
    ev["success"] = (callerRet == RET_OK && !bundleStr.empty());
    ev["statusCode"] = callerRet;
    ev["introBundle"] = bundleStr;
    ev["timestamp"] = isoTimestamp();

    deferredEmit(impl, "chatCreateIntroBundleResult", ev.dump());
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
