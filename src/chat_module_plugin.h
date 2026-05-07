#pragma once

#include <functional>
#include <string>

extern "C" {
#include "lib/liblogoschat.h"
}

/**
 * @class ChatModuleImpl
 * @brief Pure C++ implementation of the Logos Chat module.
 *
 * Most operations are asynchronous. For these methods, the call returns
 * immediately — @c true meaning the request was accepted, @c false meaning it
 * was rejected before being sent (e.g. the client has not been initialised yet).
 * The actual result then arrives via @ref emitEvent using a method-specific
 * event name and a JSON-encoded data string.
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
 * *Lifecycle*
 * | Event | JSON fields |
 * |---|---|
 * | @c chatInitResult       | `success` (bool), `statusCode` (int), `message` (string), `timestamp` (ISO-8601) |
 * | @c chatStartResult      | `success` (bool), `statusCode` (int), `message` (string), `timestamp` (ISO-8601) |
 * | @c chatStopResult       | `success` (bool), `statusCode` (int), `message` (string), `timestamp` (ISO-8601) |
 * | @c chatDestroyResult    | `message` (string), `timestamp` (ISO-8601) |
 *
 * *Client info*
 * | Event | JSON fields |
 * |---|---|
 * | @c chatGetIdResult | `id` (string), `timestamp` (ISO-8601) |
 *
 * *Conversations*
 * | Event | JSON fields |
 * |---|---|
 * | @c chatListConversationsResult        | `conversations` (string), `timestamp` (ISO-8601) |
 * | @c chatGetConversationResult          | `conversation` (string — JSON object), `timestamp` (ISO-8601) |
 * | @c chatNewPrivateConversationResult   | `success` (bool), `statusCode` (int), `conversation` (string — JSON object), `timestamp` (ISO-8601) |
 * | @c chatSendMessageResult              | `success` (bool), `statusCode` (int), `result` (string — may include message ID), `timestamp` (ISO-8601) |
 *
 * *Identity*
 * | Event | JSON fields |
 * |---|---|
 * | @c chatGetIdentityResult        | `identity` (string — JSON object), `timestamp` (ISO-8601) |
 * | @c chatCreateIntroBundleResult  | `success` (bool), `statusCode` (int), `introBundle` (string), `timestamp` (ISO-8601) |
 *
 * *Push events (via @ref setEventCallback)*
 * | Event | JSON fields |
 * |---|---|
 * | @c chatNewMessage      | `payload` (string — JSON), `timestamp` (ISO-8601) |
 * | @c chatNewConversation | `payload` (string — JSON), `timestamp` (ISO-8601) |
 * | @c chatDeliveryAck     | `payload` (string — JSON), `timestamp` (ISO-8601) |
 */
class ChatModuleImpl {
public:
    ChatModuleImpl();
    ~ChatModuleImpl();

    /// Wired automatically by the generated glue layer.
    /// Call this to emit named events to other modules / the host application.
    /// Data is a JSON-encoded string (object or array).
    std::function<void(const std::string& eventName, const std::string& data)> emitEvent;

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
     *       asynchronously as @c emitEvent("chatInitResult", data) where @c data
     *       is a JSON object with fields: @c success (bool), @c statusCode (int),
     *       @c message (string), @c timestamp (ISO-8601).
     */
    bool initChat(const std::string& configJson); // TODO: should not be async

    /**
     * @brief Starts the chat client and connects to the network.
     *
     * @ref initChat must be called first.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not yet initialised.
     *
     * @note Asynchronously returns result via @c emitEvent("chatStartResult", data)
     *       with fields: @c success (bool), @c statusCode (int), @c message (string),
     *       @c timestamp (ISO-8601).
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
     * @note Asynchronously returns result via @c emitEvent("chatStopResult", data)
     *       with fields: @c success (bool), @c statusCode (int), @c message (string),
     *       @c timestamp (ISO-8601).
     */
    bool stopChat(); // TODO: should not be async

    /**
     * @brief Deallocates the chat client.
     *
     * After this call all memory is freed, and the chat client cannot be used
     * anymore. Accessing the chat client results in undefined behavior.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note Asynchronously returns result via @c emitEvent("chatDestroyResult", data)
     *       — only emitted when the SDK provides a response message — with fields:
     *       @c message (string), @c timestamp (ISO-8601).
     */
    bool destroyChat(); // TODO: should not be async

    /**
     * @brief Subscribes to push events from the SDK.
     *
     * This is a synchronous call that registers a handler for incoming
     * events (new messages, new conversations, delivery acknowledgements) which
     * are delivered via @ref emitEvent as they arrive. Call this after
     * @ref initChat and before @ref startChat to ensure that no messages are
     * missed.
     *
     * Push events will arrive as:
     * - @c emitEvent("chatNewMessage", data)
     * - @c emitEvent("chatNewConversation", data)
     * - @c emitEvent("chatDeliveryAck", data)
     *
     * For all push events @c data is a JSON object with fields:
     * @c payload (string — JSON describing the event), @c timestamp (ISO-8601).
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
     *       asynchronously returns a result via @c emitEvent("chatGetIdResult", data)
     *       with fields: @c id (string), @c timestamp (ISO-8601).
     *
     *       On some failures the SDK may not provide an identifier or message,
     *       and in those cases no @c chatGetIdResult event is emitted. Callers
     *       must not assume that a result is always delivered.
     */
    bool getId(); // TODO: should not be async

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
     *       @c emitEvent("chatListConversationsResult", data) with fields:
     *       @c conversations (string), @c timestamp (ISO-8601).
     *
     * @warning Due to current SDK callback semantics, this event is only emitted
     *          when the underlying SDK provides a non-empty list of conversations
     *          (i.e. when @c msg is non-null and @c len > 0). On certain failures
     *          or when there are no conversations, no @c chatListConversationsResult
     *          event may be emitted. Callers SHOULD NOT rely on this event always
     *          firing; instead, use the synchronous return value from this method
     *          together with appropriate timeout or fallback handling.
     */
    bool listConversations(); // TODO: should not be async

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
     *       asynchronously as @c emitEvent("chatGetConversationResult", data) with
     *       fields: @c conversation (string — JSON object), @c timestamp (ISO-8601).
     *
     * @attention On certain internal failures (for example, if no result message
     *            is produced by the SDK), no @c chatGetConversationResult event
     *            will be emitted. Callers MUST NOT rely on this event being
     *            emitted in all failure cases and should additionally use the
     *            synchronous return value or their own timeout / error handling
     *            strategy.
     */
    bool getConversation(const std::string& convoId); // TODO: should not be async

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
     *       @c emitEvent("chatNewPrivateConversationResult", data) with fields:
     *       @c success (bool), @c statusCode (int), @c conversation (string —
     *       JSON object), @c timestamp (ISO-8601).
     */
    bool newPrivateConversation(const std::string& introBundleStr, const std::string& contentHex); // TODO: should not be async
                                                                                                   // TODO: content should accept bytes not hex

    /**
     * @brief Sends a message to an existing conversation.
     *
     * @param convoId    Identifier of the target conversation.
     * @param contentHex Hex-encoded message content.
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note Asynchronously returns result via
     *       @c emitEvent("chatSendMessageResult", data) with fields:
     *       @c success (bool), @c statusCode (int), @c result (string — may
     *       include the assigned message ID), @c timestamp (ISO-8601).
     */
    bool sendMessage(const std::string& convoId, const std::string& contentHex); // TODO: content should accept bytes not hex

    // -------------------------------------------------------------------------
    // Identity Operations
    // -------------------------------------------------------------------------

    /**
     * @brief Retrieves the local client's identity information.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note On success, asynchronously emits
     *       @c emitEvent("chatGetIdentityResult", data) with fields:
     *       @c identity (string — JSON object), @c timestamp (ISO-8601).
     *
     * @warning On some failure paths (for example, when no identity data is
     *          available or an internal error occurs in the underlying SDK), no
     *          @c chatGetIdentityResult event may be emitted even if this method
     *          returned @c true. Callers MUST NOT rely on this event always being
     *          delivered and should implement appropriate timeouts or alternative
     *          error handling.
     */
    bool getIdentity(); // TODO: Deprecate; This should not be used.

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
     * @note Asynchronously returns result via
     *       @c emitEvent("chatCreateIntroBundleResult", data) with fields:
     *       @c success (bool), @c statusCode (int), @c introBundle (string),
     *       @c timestamp (ISO-8601).
     */
    bool createIntroBundle(); // TODO: should not be async

private:
    void* chatCtx;

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
