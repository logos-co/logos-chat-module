#pragma once

#include <QtCore/QObject>
#include "chat_module_interface.h"
#include "logos_api.h"
#include "logos_sdk.h"
#include "libchat.h"

/**
 * @class ChatModulePlugin
 * @brief Qt plugin that wraps libchat (Rust) and uses the delivery module for transport.
 *
 * This is a thin C++ wrapper — all business logic lives in the Rust library (libchat).
 * The wrapper orchestrates two components:
 *   1. **libchat** (C FFI) — encryption, key exchange, conversation state
 *   2. **delivery_module** (LogosModules proxy) — network transport via Waku
 *
 * All libchat calls are synchronous. The delivery module is called via the
 * generated LogosModules proxy (m_logos->delivery_module.*).
 *
 * Data flow:
 *   Outgoing: host → initChat/sendMessage → libchat encrypts → deliverPayloads → delivery_module.send
 *   Incoming: delivery_module "messageReceived" → onDeliveryMessageReceived → libchat decrypts → emit event
 *
 * Lifecycle:
 *   1. initLogos() — host provides LogosAPI, LogosModules proxy is created
 *   2. initChat(config) — creates libchat context + delivery node (createNode)
 *   3. startChat() — starts delivery node, subscribes to inbox topic, registers message listener
 *   4. sendMessage/newPrivateConversation — encrypt via libchat, send via delivery
 *   5. stopChat() — unsubscribes, stops delivery node
 *   6. destroyChat() — stops + destroys libchat context
 *
 * Note: This module currently manages the delivery module's lifecycle (createNode/start/stop).
 * If multiple modules need delivery in the future, lifecycle management should move to logos-core.
 *
 * Events emitted (via eventResponse signal):
 *
 * Lifecycle:
 *   chatInitResult        [bool success, int code, QString message, QString timestamp]
 *   chatStartResult       [bool success, int code, QString message, QString timestamp]
 *   chatStopResult        [bool success, int code, QString message, QString timestamp]
 *   chatDestroyResult     [QString message, QString timestamp]
 *
 * Client info:
 *   chatGetIdResult       [QString installationName, QString timestamp]
 *   chatGetIdentityResult [QString json{installationName, deliveryAddress}, QString timestamp]
 *
 * Conversations:
 *   chatListConversationsResult        [QString jsonArray, QString timestamp]
 *   chatGetConversationResult          [QString json{conversationId}, QString timestamp]
 *   chatNewPrivateConversationResult   [bool success, int code, QString json{conversationId}, QString timestamp]
 *   chatSendMessageResult              [bool success, int code, QString json, QString timestamp]
 *
 * Identity:
 *   chatCreateIntroBundleResult [bool success, int code, QString bundleStr, QString timestamp]
 *
 * Push events (from delivery module):
 *   chatNewConversation [QString json{conversationId, conversationType}, QString timestamp]
 *   chatNewMessage      [QString json{conversationId, content, sender}, QString timestamp]
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

    /// Creates libchat context and delivery node from config JSON.
    /// Config fields: "name" (string), "port" (int), "clusterId" (int), "preset" (string).
    /// Emits: chatInitResult
    Q_INVOKABLE bool initChat(const QString &configJson) override;

    /// Starts the delivery node, subscribes to inbox topic, registers message listener.
    /// Emits: chatStartResult
    Q_INVOKABLE bool startChat() override;

    /// Unsubscribes from inbox topic and stops the delivery node.
    /// Emits: chatStopResult
    Q_INVOKABLE bool stopChat() override;

    /// Stops chat and destroys the libchat context.
    /// Emits: chatDestroyResult
    Q_INVOKABLE bool destroyChat() override;

    /// No-op — event handling is set up in startChat().
    Q_INVOKABLE bool setEventCallback() override;

    // -------------------------------------------------------------------------
    // Client Info
    // -------------------------------------------------------------------------

    /// Returns the installation name. Emits: chatGetIdResult
    Q_INVOKABLE bool getId() override;

    // -------------------------------------------------------------------------
    // Conversation Operations
    // -------------------------------------------------------------------------

    /// Lists all conversation IDs. Emits: chatListConversationsResult
    Q_INVOKABLE bool listConversations() override;

    /// Returns info for a single conversation. Emits: chatGetConversationResult
    Q_INVOKABLE bool getConversation(const QString &convoId) override;

    /// Creates a new 1-to-1 conversation from a remote intro bundle.
    /// @param introBundleStr Intro bundle string from the remote peer (logos_chatintro_1_... format).
    /// @param contentHex Hex-encoded initial message content.
    /// Emits: chatNewPrivateConversationResult
    Q_INVOKABLE bool newPrivateConversation(const QString &introBundleStr, const QString &contentHex) override;

    /// Sends a message to an existing conversation.
    /// @param convoId Conversation identifier.
    /// @param contentHex Hex-encoded message content.
    /// Emits: chatSendMessageResult
    Q_INVOKABLE bool sendMessage(const QString &convoId, const QString &contentHex) override;

    // -------------------------------------------------------------------------
    // Identity Operations
    // -------------------------------------------------------------------------

    /// Returns installation name and delivery address. Emits: chatGetIdentityResult
    Q_INVOKABLE bool getIdentity() override;

    /// Creates a shareable intro bundle for this installation.
    /// Emits: chatCreateIntroBundleResult
    Q_INVOKABLE bool createIntroBundle() override;

    QString name() const override { return "chat_module"; }
    QString version() const override { return "1.0.0"; }

    /// Called by the host to provide the LogosAPI instance. Creates LogosModules proxy.
    Q_INVOKABLE void initLogos(LogosAPI* logosAPIInstance);

    /// Forwards an event to the host via LogosAPI client.
    void emitEvent(const QString& eventName, const QVariantList& data);

signals:
    void eventResponse(const QString& eventName, const QVariantList& data);

private:
    ContextHandle_t* chatCtx;       ///< Opaque handle to libchat context (owned)
    LogosModules* m_logos;           ///< Generated proxy for module-to-module calls (owned)
    QString localContentTopic;       ///< Content topic for our inbox (e.g. /logos-chat/1/{addr}/proto)
    bool m_deliveryListenerActive;   ///< Whether messageReceived listener is registered

    /// Sends all payloads from a libchat result via delivery_module.send().
    /// Each payload's address field is used to construct the content topic.
    /// Payload data is base64-encoded for transport.
    void deliverPayloads(const Payload_t* payloads, size_t count);

    /// Handles incoming "messageReceived" events from the delivery module.
    /// Decodes the payload (double base64: delivery layer + our encoding),
    /// passes raw bytes to libchat's handle_payload, and emits chat events.
    void onDeliveryMessageReceived(const QVariantList& data);

    /// Builds a content topic string from a delivery address.
    /// Format: /logos-chat/1/{hex_address}/proto
    static QString buildContentTopic(const char* addressBytes, size_t addressLen);
};
