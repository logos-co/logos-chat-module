#pragma once

#include <cstdint>
#include <string>

#include <QObject>

#include <logos_module_context.h>

extern "C" {
#include "lib/liblogoschat.h"
}

/**
 * @brief Pure C++ implementation of the Logos Chat module.
 *
 * Most operations are asynchronous. For these methods, the call returns
 * immediately — @c true meaning the request was accepted, @c false meaning it
 * was rejected before being sent (e.g. the client has not been initialised yet).
 * The actual result then arrives as a typed event declared in the
 * @c logos_events: section below, using a method-specific event name and
 * strongly-typed arguments.
 *
 * Some helper operations are synchronous (e.g. @ref setEventCallback) and do
 * not emit an event for completion.
 *
 * **Typical startup sequence:**
 * -# @ref initChat — initialise the client with your configuration.
 * -# @ref setEventCallback — subscribe to push events before starting.
 * -# @ref startChat — connect and begin receiving messages.
 *
 * **Event reference:**
 *
 * Each event is a typed `logos_events:` method; the universal codegen marshals
 * its arguments into one QVariant slot each (consumers read them positionally).
 *
 * *Lifecycle*
 * | Event | Arguments |
 * |---|---|
 * | @c chatInitResult       | `success` (bool), `statusCode` (int64), `message` (string), `timestamp` (ISO-8601) |
 * | @c chatStartResult      | `success` (bool), `statusCode` (int64), `message` (string), `timestamp` (ISO-8601) |
 * | @c chatStopResult       | `success` (bool), `statusCode` (int64), `message` (string), `timestamp` (ISO-8601) |
 * | @c chatDestroyResult    | `message` (string), `timestamp` (ISO-8601) |
 *
 * *Client info*
 * | Event | Arguments |
 * |---|---|
 * | @c chatGetIdResult | `clientId` (string), `timestamp` (ISO-8601) |
 *
 * *Conversations*
 * | Event | Arguments |
 * |---|---|
 * | @c chatListConversationsResult        | `conversations` (string), `timestamp` (ISO-8601) |
 * | @c chatGetConversationResult          | `conversation` (string — JSON object), `timestamp` (ISO-8601) |
 * | @c chatNewPrivateConversationResult   | `success` (bool), `statusCode` (int64), `conversation` (string — JSON object), `timestamp` (ISO-8601) |
 * | @c chatSendMessageResult              | `success` (bool), `statusCode` (int64), `result` (string — may include message ID), `timestamp` (ISO-8601) |
 *
 * *Identity*
 * | Event | Arguments |
 * |---|---|
 * | @c chatGetIdentityResult        | `identity` (string — JSON object), `timestamp` (ISO-8601) |
 * | @c chatCreateIntroBundleResult  | `success` (bool), `statusCode` (int64), `introBundle` (string), `timestamp` (ISO-8601) |
 *
 * *Push events (via @ref setEventCallback)*
 * | Event | Arguments |
 * |---|---|
 * | @c chatNewMessage      | `payload` (string — JSON), `timestamp` (ISO-8601) |
 * | @c chatNewConversation | `payload` (string — JSON), `timestamp` (ISO-8601) |
 * | @c chatDeliveryAck     | `payload` (string — JSON), `timestamp` (ISO-8601) |
 * | @c chatEvent           | `payload` (string — JSON), `timestamp` (ISO-8601) — fallback for unrecognised SDK event types |
 */
class ChatModuleImpl : public LogosModuleContext {
public:
    ChatModuleImpl();
    ~ChatModuleImpl();

    /// QObject anchor used as the receiver for deferred-emit posts inside
    /// libchat callbacks (see chat_module_plugin.cpp:deferredEmit). When
    /// this ChatModuleImpl is destroyed the anchor is destroyed too, so
    /// any pending `QMetaObject::invokeMethod(..., Qt::QueuedConnection)`
    /// targeting it is dropped by Qt — preventing use-after-free of a
    /// captured `impl` pointer if a callback fires shortly before teardown.
    QObject* emitRouter() { return &m_emitRouter; }

    // -------------------------------------------------------------------------
    // Client Lifecycle
    // -------------------------------------------------------------------------

    /**
     * @brief Initialises the chat client with the provided delivery configuration.
     *
     * @param configJson JSON configuration for the delivery service.
     * @return @c true if the request was accepted and initialisation was started;
     *         @c false if initialisation could not start (e.g. invalid config
     *         preventing context creation). When this function returns @c false,
     *         no event is emitted and the caller must rely on the return value.
     *
     * @note If this function returns @c true, the result is delivered
     *       asynchronously via @ref chatInitResult with arguments: @c success
     *       (bool), @c statusCode (int64), @c message (string), @c timestamp
     *       (ISO-8601).
     */
    // TODO: should not be async
    bool initChat(const std::string& configJson);

    /**
     * @brief Starts the chat client and connects to the network.
     *
     * @ref initChat must be called first.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not yet initialised.
     *
     * @note Asynchronously returns result via @ref chatStartResult with
     *       arguments: @c success (bool), @c statusCode (int64), @c message
     *       (string), @c timestamp (ISO-8601).
     */
    bool startChat();

    /**
     * @brief Stops the chat client and disconnects from the network.
     *
     * This is only called when deinitializing the chat client.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note Asynchronously returns result via @ref chatStopResult with
     *       arguments: @c success (bool), @c statusCode (int64), @c message
     *       (string), @c timestamp (ISO-8601).
     */
    // TODO: should not be async
    bool stopChat();

    /**
     * @brief Deallocates the chat client.
     *
     * After this call all memory is freed, and the chat client cannot be used
     * anymore. Accessing the chat client results in undefined behavior.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note Asynchronously returns result via @ref chatDestroyResult — only
     *       emitted when the SDK provides a response message — with arguments:
     *       @c message (string), @c timestamp (ISO-8601).
     */
    // TODO: should not be async
    bool destroyChat();

    /**
     * @brief Subscribes to push events from the SDK.
     *
     * This is a synchronous call that registers a handler for incoming
     * events (new messages, new conversations, delivery acknowledgements) which
     * are delivered as typed push events as they arrive. Call this after
     * @ref initChat and before @ref startChat to ensure that no messages are
     * missed.
     *
     * Push events will arrive as:
     * - @ref chatNewMessage
     * - @ref chatNewConversation
     * - @ref chatDeliveryAck
     *
     * For all push events the arguments are @c payload (string — JSON describing
     * the event) and @c timestamp (ISO-8601).
     *
     * @return @c true if the subscription was registered; @c false if the
     *         client is not initialised.
     */
    bool setEventCallback();

    // -------------------------------------------------------------------------
    // Client Info
    // -------------------------------------------------------------------------

    /**
     * @brief Retrieves the local client's unique identifier.
     *
     * Ids can be used to uniquely distinguish between client installations.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note When the SDK provides a non-empty identifier, this call
     *       asynchronously returns a result via @ref chatGetIdResult with
     *       arguments: @c clientId (string), @c timestamp (ISO-8601).
     *
     *       On some failures the SDK may not provide an identifier or message,
     *       and in those cases no @c chatGetIdResult event is emitted. Callers
     *       must not assume that a result is always delivered.
     */
    // TODO: should not be async
    bool getId();

    // -------------------------------------------------------------------------
    // Conversation Operations
    // -------------------------------------------------------------------------

    /**
     * @brief Retrieves all conversations the local client participates in.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note Asynchronously returns result (when available) via
     *       @ref chatListConversationsResult with arguments: @c conversations
     *       (string), @c timestamp (ISO-8601).
     *
     * @warning Due to current SDK callback semantics, this event is only emitted
     *          when the underlying SDK provides a non-empty list of conversations
     *          (i.e. when @c msg is non-null and @c len > 0). On certain failures
     *          or when there are no conversations, no @c chatListConversationsResult
     *          event may be emitted. Callers SHOULD NOT rely on this event always
     *          firing; instead, use the synchronous return value from this method
     *          together with appropriate timeout or fallback handling.
     */
    // TODO: should not be async
    bool listConversations();

    /**
     * @brief Retrieves a single conversation by its identifier.
     *
     * This conversation can be used to send messages.
     *
     * @param convoId The conversation identifier to look up.
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note When the underlying SDK returns a result message, it is delivered
     *       asynchronously via @ref chatGetConversationResult with arguments:
     *       @c conversation (string — JSON object), @c timestamp (ISO-8601).
     *
     * @attention On certain internal failures (for example, if no result message
     *            is produced by the SDK), no @c chatGetConversationResult event
     *            will be emitted. Callers MUST NOT rely on this event being
     *            emitted in all failure cases and should additionally use the
     *            synchronous return value or their own timeout / error handling
     *            strategy.
     */
    // TODO: should not be async
    bool getConversation(const std::string& convoId);

    /**
     * @brief Starts a new private (1-to-1) conversation with a remote contact.
     *
     * The remote contact must share their introduction bundle (see
     * @ref createIntroBundle) with you out-of-band. You must include an
     * initial message.
     *
     * @param introBundleStr Introduction bundle of the remote contact.
     * @param contentHex     Hex-encoded content of the opening message.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note Asynchronously returns result via
     *       @ref chatNewPrivateConversationResult with arguments: @c success
     *       (bool), @c statusCode (int64), @c conversation (string — JSON
     *       object), @c timestamp (ISO-8601).
     */
    // TODO: should not be async
    // TODO: content should accept bytes not hex
    bool newPrivateConversation(const std::string& introBundleStr, const std::string& contentHex);

    /**
     * @brief Sends a message to an existing conversation.
     *
     * @param convoId    Identifier of the target conversation.
     * @param contentHex Hex-encoded message content.
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note Asynchronously returns result via @ref chatSendMessageResult with
     *       arguments: @c success (bool), @c statusCode (int64), @c result
     *       (string — may include the assigned message ID), @c timestamp
     *       (ISO-8601).
     */
    // TODO: content should accept bytes not hex
    bool sendMessage(const std::string& convoId, const std::string& contentHex);

    // -------------------------------------------------------------------------
    // Identity Operations
    // -------------------------------------------------------------------------

    /**
     * @brief Retrieves the local client's identity information.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note On success, asynchronously emits @ref chatGetIdentityResult with
     *       arguments: @c identity (string — JSON object), @c timestamp
     *       (ISO-8601).
     *
     * @warning On some failure paths (for example, when no identity data is
     *          available or an internal error occurs in the underlying SDK), no
     *          @c chatGetIdentityResult event may be emitted even if this method
     *          returned @c true. Callers MUST NOT rely on this event always being
     *          delivered and should implement appropriate timeouts or alternative
     *          error handling.
     */
    // TODO: Deprecate; This should not be used.
    bool getIdentity();

    /**
     * @brief Creates a new introduction bundle to share with other users.
     *
     * The bundle encodes the public key material that a remote party needs to
     * initiate a private conversation with you via @ref newPrivateConversation.
     * Share it out-of-band (e.g. via a QR code or copy-paste).
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note Asynchronously returns result via @ref chatCreateIntroBundleResult
     *       with arguments: @c success (bool), @c statusCode (int64),
     *       @c introBundle (string), @c timestamp (ISO-8601).
     */
    // TODO: should not be async
    bool createIntroBundle();

    // -------------------------------------------------------------------------
    // Events
    // -------------------------------------------------------------------------
    //
    // Typed asynchronous events. The universal codegen (logos-cpp-generator)
    // emits the method bodies in a sidecar `chat_module_events.cpp`, marshalling
    // each argument into a QVariant slot and routing it through
    // LogosModuleContext::emitEventImpl_. Module code emits an event simply by
    // calling the corresponding method.
logos_events:
    void chatInitResult(bool success, int64_t statusCode, const std::string& message, const std::string& timestamp);
    void chatStartResult(bool success, int64_t statusCode, const std::string& message, const std::string& timestamp);
    void chatStopResult(bool success, int64_t statusCode, const std::string& message, const std::string& timestamp);
    void chatDestroyResult(const std::string& message, const std::string& timestamp);
    void chatGetIdResult(const std::string& clientId, const std::string& timestamp);
    void chatListConversationsResult(const std::string& conversations, const std::string& timestamp);
    void chatGetConversationResult(const std::string& conversation, const std::string& timestamp);
    void chatNewPrivateConversationResult(bool success, int64_t statusCode, const std::string& conversation, const std::string& timestamp);
    void chatSendMessageResult(bool success, int64_t statusCode, const std::string& result, const std::string& timestamp);
    void chatGetIdentityResult(const std::string& identity, const std::string& timestamp);
    void chatCreateIntroBundleResult(bool success, int64_t statusCode, const std::string& introBundle, const std::string& timestamp);
    void chatNewMessage(const std::string& payload, const std::string& timestamp);
    void chatNewConversation(const std::string& payload, const std::string& timestamp);
    void chatDeliveryAck(const std::string& payload, const std::string& timestamp);
    void chatEvent(const std::string& payload, const std::string& timestamp);

private:
    void* chatCtx;

    /// Receiver for `QMetaObject::invokeMethod(..., Qt::QueuedConnection)`
    /// in `deferredEmit`. See the accessor `emitRouter()` above.
    QObject m_emitRouter;

    static void init_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void start_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void stop_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void destroy_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void event_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void get_id_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void list_conversations_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void get_conversation_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void new_private_conversation_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void send_message_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void get_identity_callback(int callerRet, const char* msg, size_t len, void* userData);
    static void create_intro_bundle_callback(int callerRet, const char* msg, size_t len, void* userData);
};
