#pragma once

#include <QtCore/QObject>
#include "chat_module_interface.h"
#include "logos_api.h"
#include "logos_api_client.h"
#include "liblogoschat.h"

/**
 * @class ChatModulePlugin
 * @brief Qt plugin that exposes Logos Chat.
 *
 * Most operations are asynchronous. For these methods, the call returns
 * immediately — @c true meaning the request was accepted, @c false meaning it
 * was rejected before being sent (e.g. the client has not been initialised yet).
 * The actual result then arrives via the @ref eventResponse signal using a
 * method-specific event name.
 *
 * Some helper operations are synchronous (e.g. @ref initLogos and
 * @ref setEventCallback) and do not emit an @ref eventResponse for completion.
 *
 * **Typical startup sequence:**
 * -# @ref initLogos — provide the LogosAPI instance.
 * -# @ref initChat — initialise the client with your configuration.
 * -# @ref setEventCallback — subscribe to push events before starting.
 * -# @ref startChat — connect and begin receiving messages.
 */
class ChatModulePlugin : public QObject, public ChatModuleInterface
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID ChatModuleInterface_iid FILE "metadata.json")
    Q_INTERFACES(ChatModuleInterface PluginInterface)

public:
    ChatModulePlugin();
    ~ChatModulePlugin();

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
     *         no result signal is emitted and the caller must rely on the return value.
     *
     * @note If this function returns @c true, the result is returned asynchronously as
    *       @c eventResponse("chatInitResult", data)
     *   - @c data[0] @c bool — @c true on success.
     *   - @c data[1] @c int — status code.
     *   - @c data[2] @c QString — optional message from the SDK.
     *   - @c data[3] @c QString — ISO-8601 timestamp.
     */
    Q_INVOKABLE bool initChat(const QString &configJson) override; // TODO: should not be async

    /**
     * @brief Starts the chat client and connects to the network.
     *
     * @ref initChat must be called first.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not yet initialised.
     *
    * @note Asynchronously returns result: @c eventResponse("chatStartResult", data)
     *   - @c data[0] @c bool — @c true on success.
     *   - @c data[1] @c int — status code.
     *   - @c data[2] @c QString — optional message from the SDK.
     *   - @c data[3] @c QString — ISO-8601 timestamp.
     */
    Q_INVOKABLE bool startChat() override;

    /**
     * @brief Stops the chat client and disconnects from the network.
     *
     * This is only called when deinitializing the chat client. 
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
    * @note Asynchronously returns result: @c eventResponse("chatStopResult", data)
     *   - @c data[0] @c bool — @c true on success.
     *   - @c data[1] @c int — status code.
     *   - @c data[2] @c QString — optional message from the SDK.
     *   - @c data[3] @c QString — ISO-8601 timestamp.
     */
    Q_INVOKABLE bool stopChat() override; // TODO: should not be async

    /**
     * @brief Deallocates the chat client.
     *
     * After this call all memory is freed, and the chat client cannot be used anymore.
     * Accessing the chat client results in undefined behavior.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
    * @note  Asynchronously returns result: @c eventResponse("chatDestroyResult", data) — only emitted
     *       when the SDK provides a response message:
     *   - @c data[0] @c QString — message from the SDK.
     *   - @c data[1] @c QString — ISO-8601 timestamp.
     */
    Q_INVOKABLE bool destroyChat() override; // TODO: should not be async

    /**
     * @brief Subscribes to push events from the SDK.
     *
     * This is a synchronous call that registers a handler for incoming
     * events (new messages, new conversations, delivery acknowledgements) which are
     * delivered via @ref eventResponse as they arrive. Call this after @ref initChat
      and before @ref startChat to ensure that no messages are missed.
     *
     * Push events will arrive as:
    * - @c eventResponse("chatNewMessage", data)
    * - @c eventResponse("chatNewConversation", data)
    * - @c eventResponse("chatDeliveryAck", data)
     *
     * For all push events @c data is:
     *   - @c data[0] @c QString — JSON payload describing the event.
     *   - @c data[1] @c QString — ISO-8601 timestamp.
     *
     * @return @c true if the subscription was registered; @c false if the
     *         client is not initialised.
     */
    Q_INVOKABLE bool setEventCallback() override;

    // -------------------------------------------------------------------------
    // Client Info
    // -------------------------------------------------------------------------

    /**
     * @brief Retrieves the local client's unique identifier.
     * 
     * Ids can be used to uniquely distinguish between client installtions.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note When the SDK provides a non-empty identifier, this call
    *       asynchronously returns a result via @c eventResponse("chatGetIdResult", data)
     *   - @c data[0] @c QString — the client identifier.
     *   - @c data[1] @c QString — ISO-8601 timestamp.
     *
     *       On some failures the SDK may not provide an identifier or message,
    *       and in those cases no @c chatGetIdResult event is emitted. Callers
     *       must not assume that a result signal is always delivered.
     */
    Q_INVOKABLE bool getId() override; // TODO: should not be async

    // -------------------------------------------------------------------------
    // Conversation Operations
    // -------------------------------------------------------------------------

    /**
     * @brief Retrieves all conversations the local client participates in.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
    * @note Asynchronously returns result (when available): @c eventResponse("chatListConversationsResult", data)
     *   - @c data[0] @c QString — Conversation Ids.
     *   - @c data[1] @c QString — ISO-8601 timestamp.
     *
     * @warning Due to current SDK callback semantics, this event is only emitted
     *          when the underlying SDK provides a non-empty list of conversations
     *          (i.e. when @c msg is non-null and @c len > 0). On certain failures
    *          or when there are no conversations, no @c chatListConversationsResult
     *          event may be emitted. Callers SHOULD NOT rely on this event always
     *          firing; instead, use the synchronous return value from this method
     *          together with appropriate timeout or fallback handling.
     */
    Q_INVOKABLE bool listConversations() override;  // TODO: should not be async

    /**
     * @brief Retrieves a single conversation by its identifier.
     *
     * This conversation can be used to send messages.
     * @param convoId The conversation identifier to look up.
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
     * @note  When the underlying SDK returns a result message, it is delivered
    *        asynchronously as: @c eventResponse("chatGetConversationResult", data)
     *   - @c data[0] @c QString — JSON object describing the conversation.
     *   - @c data[1] @c QString — ISO-8601 timestamp.
     *
     * @attention On certain internal failures (for example, if no result message
    *            is produced by the SDK), no @c chatGetConversationResult
     *            event will be emitted. Callers MUST NOT rely on this signal
     *            being emitted in all failure cases and should additionally use
     *            the synchronous return value or their own timeout / error
     *            handling strategy.
     */
    Q_INVOKABLE bool getConversation(const QString &convoId) override;  // TODO: should not be async

    /**
     * @brief Starts a new private (1-to-1) conversation with a remote contact.
     *
     * The remote contact must share their introduction bundle (see
     * @ref createIntroBundle) with you out-of-band. You must include an
     * intial message.
     *
     * @param introBundleStr Introduction bundle of the remote contact.
     * @param contentHex     Hex-encoded content of the opening message
     *                     
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
    * @note  Asynchronously returns result: @c eventResponse("chatNewPrivateConversationResult", data)
     *   - @c data[0] @c bool — @c true on success.
     *   - @c data[1] @c int — status code.
     *   - @c data[2] @c QString — JSON object of the newly created conversation.
     *   - @c data[3] @c QString — ISO-8601 timestamp.
     */
    Q_INVOKABLE bool newPrivateConversation(const QString &introBundleStr, const QString &contentHex) override;  // TODO: should not be async
                                                                                                                 // TODO: content should accept bytes not hex     
    /**
     * @brief Sends a message to an existing conversation.
     *
     * @param convoId    Identifier of the target conversation.
     * @param contentHex Hex-encoded message content.
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
    * @note  Asynchronously returns result: @c eventResponse("chatSendMessageResult", data)
     *   - @c data[0] @c bool — @c true on success.
     *   - @c data[1] @c int — status code.
     *   - @c data[2] @c QString — JSON result, may include the assigned message ID.
     *   - @c data[3] @c QString — ISO-8601 timestamp.
     */
    Q_INVOKABLE bool sendMessage(const QString &convoId, const QString &contentHex) override;   // TODO: content should accept bytes not hex       
    // -------------------------------------------------------------------------
                                                                                              
    // Identity Operations
    // -------------------------------------------------------------------------

    /**
     * @brief Retrieves the local client's identity information.
     *
     * @return @c true if the request was accepted; @c false if the client is
     *         not initialised.
     *
    * @note  On success, asynchronously emits: @c eventResponse("chatGetIdentityResult", data)
     *   - @c data[0] @c QString — JSON object containing identity fields.
     *   - @c data[1] @c QString — ISO-8601 timestamp.
     *
     * @warning On some failure paths (for example, when no identity data is available
     *          or an internal error occurs in the underlying SDK), no
    *          @c chatGetIdentityResult event may be emitted even if this method
     *          returned @c true. Callers MUST NOT rely on this event always being
     *          delivered and should implement appropriate timeouts or alternative
     *          error handling.
     */
    Q_INVOKABLE bool getIdentity() override;  // TODO: Deprecate; This should not be used.

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
    * @note  Asynchronously returns result: @c eventResponse("chatCreateIntroBundleResult", data)
     *   - @c data[0] @c bool — @c true on success.
     *   - @c data[1] @c int — status code.
     *   - @c data[2] @c QString — the introduction bundle string to share.
     *   - @c data[3] @c QString — ISO-8601 timestamp.
     */
    Q_INVOKABLE bool createIntroBundle() override;  // TODO: should not be async

    /** @brief Returns the plugin name. */
    QString name() const override { return "chat_module"; }

    /** @brief Returns the plugin version string. */
    QString version() const override { return "1.0.0"; }

    /**
     * @brief Provides the LogosAPI instance used to route events to the host.
     *
     * @param logosAPIInstance The host application's LogosAPI object.
     */
    Q_INVOKABLE void initLogos(LogosAPI* logosAPIInstance);

    /**
     * @brief Forwards an SDK event to the host application.
     *
     * Intended for internal use by callback functions. Consumer code should
     * connect to @ref eventResponse instead.
     *
     * @param eventName Name of the event.
     * @param data      Ordered list of event arguments.
     */
    void emitEvent(const QString& eventName, const QVariantList& data);

signals:
    /**
     * @brief Emitted when the SDK completes an operation or delivers a push event.
     *
     * Connect to this signal to handle all chat SDK responses. Use @p eventName
     * to identify which operation completed. See each method's @c @note for the
     * exact @p data layout.
     *
     * **Event reference:**
     *
     * *Lifecycle*
     * | Event | data[0] | data[1] | data[2] | data[3] |
     * |---|---|---|---|---|
    * | @c chatInitResult       | `bool` success | `int` status code | `QString` SDK message | `QString` ISO-8601 timestamp |
    * | @c chatStartResult      | `bool` success | `int` status code | `QString` SDK message | `QString` ISO-8601 timestamp |
    * | @c chatStopResult       | `bool` success | `int` status code | `QString` SDK message | `QString` ISO-8601 timestamp |
    * | @c chatDestroyResult    | `QString` SDK message | `QString` ISO-8601 timestamp | — | — |
     *
     * *Client info*
     * | Event | data[0] | data[1] |
     * |---|---|---|
    * | @c chatGetIdResult | `QString` client identifier | `QString` ISO-8601 timestamp |
     *
     * *Conversations*
     * | Event | data[0] | data[1] | data[2] | data[3] |
     * |---|---|---|---|---|
    * | @c chatListConversationsResult        | `QString` conversation IDs | `QString` ISO-8601 timestamp | — | — |
    * | @c chatGetConversationResult          | `QString` JSON conversation object | `QString` ISO-8601 timestamp | — | — |
    * | @c chatNewPrivateConversationResult   | `bool` success | `int` status code | `QString` JSON conversation object | `QString` ISO-8601 timestamp |
    * | @c chatSendMessageResult              | `bool` success | `int` status code | `QString` JSON result (may include message ID) | `QString` ISO-8601 timestamp |
     *
     * *Identity*
     * | Event | data[0] | data[1] | data[2] | data[3] |
     * |---|---|---|---|---|
    * | @c chatGetIdentityResult        | `QString` JSON identity object | `QString` ISO-8601 timestamp | — | — |
    * | @c chatCreateIntroBundleResult  | `bool` success | `int` status code | `QString` introduction bundle string | `QString` ISO-8601 timestamp |
     *
     * *Push events (via @ref setEventCallback)*
     * | Event | data[0] | data[1] |
     * |---|---|---|
    * | @c chatNewMessage      | `QString` JSON payload | `QString` ISO-8601 timestamp |
    * | @c chatNewConversation | `QString` JSON payload | `QString` ISO-8601 timestamp |
    * | @c chatDeliveryAck     | `QString` JSON payload | `QString` ISO-8601 timestamp |
     *
     * @param eventName Name identifying the event type.
     * @param data      Ordered list of event-specific arguments.
     */
    void eventResponse(const QString& eventName, const QVariantList& data);  // TODO: should split into dedicated signals per event.     

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
