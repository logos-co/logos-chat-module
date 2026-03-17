#include "chat_module_plugin.h"
#include <QDebug>
#include <QCoreApplication>
#include <QVariantList>
#include <QDateTime>
#include <QJsonDocument>
#include <QJsonObject>

ChatModulePlugin::ChatModulePlugin() : chatCtx(nullptr)
{
    qDebug() << "ChatModulePlugin: Initializing...";
    qDebug() << "ChatModulePlugin: Initialized successfully";
}

ChatModulePlugin::~ChatModulePlugin() 
{
    // Clean up Chat context if it exists
    if (chatCtx) {
        chat_destroy(chatCtx, destroy_callback, this);
        chatCtx = nullptr;
    }
    
    // Clean up resources
    if (logosAPI) {
        delete logosAPI;
        logosAPI = nullptr;
    }
}

void ChatModulePlugin::initLogos(LogosAPI* logosAPIInstance) {
    if (logosAPI) {
        delete logosAPI;
    }
    logosAPI = logosAPIInstance;
}

void ChatModulePlugin::emitEvent(const QString& eventName, const QVariantList& data) {
    if (!logosAPI) {
        qWarning() << "ChatModulePlugin: LogosAPI not available, cannot emit" << eventName;
        return;
    }

    LogosAPIClient* client = logosAPI->getClient("chat_module");
    if (!client) {
        qWarning() << "ChatModulePlugin: Failed to get chat_module client for event" << eventName;
        return;
    }

    client->onEventResponse(this, eventName, data);
}

// ============================================================================
// Static Callback Functions
// ============================================================================

void ChatModulePlugin::init_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::init_callback called with ret:" << callerRet;
    
    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::init_callback: Invalid userData";
        return;
    }

    QString message = (msg && len > 0) ? QString::fromUtf8(msg, len) : "";

    QVariantList eventData;
    eventData << (callerRet == RET_OK);  // success boolean
    eventData << callerRet;               // return code
    eventData << message;                 // message (may be empty)
    eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

    plugin->emitEvent("chatsdkInitResult", eventData);
}

void ChatModulePlugin::start_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::start_callback called with ret:" << callerRet;
    
    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::start_callback: Invalid userData";
        return;
    }

    QString message = (msg && len > 0) ? QString::fromUtf8(msg, len) : "";
    
    QVariantList eventData;
    eventData << (callerRet == RET_OK);  // success boolean
    eventData << callerRet;               // return code
    eventData << message;
    eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

    plugin->emitEvent("chatsdkStartResult", eventData);
}

void ChatModulePlugin::stop_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::stop_callback called with ret:" << callerRet;
    
    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::stop_callback: Invalid userData";
        return;
    }

    QString message = (msg && len > 0) ? QString::fromUtf8(msg, len) : "";

    QVariantList eventData;
    eventData << (callerRet == RET_OK);  // success boolean
    eventData << callerRet;               // return code
    eventData << message;
    eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

    plugin->emitEvent("chatsdkStopResult", eventData);
}

void ChatModulePlugin::destroy_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::destroy_callback called with ret:" << callerRet;
    
    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::destroy_callback: Invalid userData";
        return;
    }

    if (msg && len > 0) {
        QString message = QString::fromUtf8(msg, len);
        qDebug() << "ChatModulePlugin::destroy_callback message:" << message;

        QVariantList eventData;
        eventData << message;
        eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

        plugin->emitEvent("chatsdkDestroyResult", eventData);
    }
}

void ChatModulePlugin::event_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::event_callback called with ret:" << callerRet;

    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::event_callback: Invalid userData";
        return;
    }

    if (msg && len > 0) {
        QString message = QString::fromUtf8(msg, len);
        
        // Parse the JSON to determine the event type
        QJsonDocument doc = QJsonDocument::fromJson(message.toUtf8());
        QString eventName = "chatsdkEvent"; // Default event name
        
        if (doc.isObject()) {
            QJsonObject obj = doc.object();
            QString eventType = obj["eventType"].toString();
            
            // Map event types to Qt event names
            if (eventType == "new_message") {
                eventName = "chatsdkNewMessage";
            } else if (eventType == "new_conversation") {
                eventName = "chatsdkNewConversation";
            } else if (eventType == "delivery_ack") {
                eventName = "chatsdkDeliveryAck";
            }
        }

        QVariantList eventData;
        eventData << message;
        eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

        plugin->emitEvent(eventName, eventData);
    }
}

void ChatModulePlugin::get_id_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::get_id_callback called with ret:" << callerRet;

    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::get_id_callback: Invalid userData";
        return;
    }

    if (msg && len > 0) {
        QString message = QString::fromUtf8(msg, len);
        
        QVariantList eventData;
        eventData << message;
        eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

        plugin->emitEvent("chatsdkGetIdResult", eventData);
    }
}

void ChatModulePlugin::list_conversations_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::list_conversations_callback called with ret:" << callerRet;

    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::list_conversations_callback: Invalid userData";
        return;
    }

    if (msg && len > 0) {
        QString message = QString::fromUtf8(msg, len);
        
        QVariantList eventData;
        eventData << message;
        eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

        plugin->emitEvent("chatsdkListConversationsResult", eventData);
    }
}

void ChatModulePlugin::get_conversation_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::get_conversation_callback called with ret:" << callerRet;

    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::get_conversation_callback: Invalid userData";
        return;
    }

    if (msg && len > 0) {
        QString message = QString::fromUtf8(msg, len);
        
        QVariantList eventData;
        eventData << message;
        eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

        plugin->emitEvent("chatsdkGetConversationResult", eventData);
    }
}

void ChatModulePlugin::new_private_conversation_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::new_private_conversation_callback called with ret:" << callerRet;

    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::new_private_conversation_callback: Invalid userData";
        return;
    }

    QString conversationJson = (msg && len > 0) ? QString::fromUtf8(msg, len) : "";
    
    QVariantList eventData;
    eventData << (callerRet == RET_OK && !conversationJson.isEmpty());  // success
    eventData << callerRet;                                               // return code
    eventData << conversationJson;                                        // conversation JSON
    eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

    plugin->emitEvent("chatsdkNewPrivateConversationResult", eventData);
}

void ChatModulePlugin::send_message_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::send_message_callback called with ret:" << callerRet;

    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::send_message_callback: Invalid userData";
        return;
    }

    QString resultJson = (msg && len > 0) ? QString::fromUtf8(msg, len) : "";
    qDebug() << "ChatModulePlugin::send_message_callback result:" << resultJson;
    
    QVariantList eventData;
    eventData << (callerRet == RET_OK);  // success
    eventData << callerRet;               // return code
    eventData << resultJson;              // result JSON (may contain message ID)
    eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

    plugin->emitEvent("chatsdkSendMessageResult", eventData);
}

void ChatModulePlugin::get_identity_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::get_identity_callback called with ret:" << callerRet;

    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::get_identity_callback: Invalid userData";
        return;
    }

    if (msg && len > 0) {
        QString message = QString::fromUtf8(msg, len);
        
        QVariantList eventData;
        eventData << message;
        eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

        plugin->emitEvent("chatsdkGetIdentityResult", eventData);
    }
}

void ChatModulePlugin::create_intro_bundle_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatModulePlugin::create_intro_bundle_callback called with ret:" << callerRet;

    ChatModulePlugin* plugin = static_cast<ChatModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatModulePlugin::create_intro_bundle_callback: Invalid userData";
        return;
    }

    QString bundleStr = (msg && len > 0) ? QString::fromUtf8(msg, len) : "";

    QVariantList eventData;
    eventData << (callerRet == RET_OK && !bundleStr.isEmpty());  // success
    eventData << callerRet;                                        // return code
    eventData << bundleStr;                                        // intro bundle string
    eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

    plugin->emitEvent("chatsdkCreateIntroBundleResult", eventData);
}

// ============================================================================
// Client Lifecycle Methods
// ============================================================================

bool ChatModulePlugin::initChat(const QString &configJson)
{
    qDebug() << "ChatModulePlugin::initChat called with config:" << configJson;
    
    // Convert QString to UTF-8 byte array
    QByteArray cfgUtf8 = configJson.toUtf8();
    
    // Call chat_new with the configuration
    chatCtx = chat_new(cfgUtf8.constData(), init_callback, this);
    
    if (chatCtx) {
        qDebug() << "ChatModulePlugin: Chat context created successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to create Chat context";
        return false;
    }
}

bool ChatModulePlugin::startChat()
{
    qDebug() << "ChatModulePlugin::startChat called";
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot start Chat - context not initialized. Call initChat first.";
        return false;
    }
    
    int result = chat_start(chatCtx, start_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: Chat start initiated successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to start Chat, error code:" << result;
        return false;
    }
}

bool ChatModulePlugin::stopChat()
{
    qDebug() << "ChatModulePlugin::stopChat called";
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot stop Chat - context not initialized.";
        return false;
    }
    
    int result = chat_stop(chatCtx, stop_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: Chat stop initiated successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to stop Chat, error code:" << result;
        return false;
    }
}

bool ChatModulePlugin::destroyChat()
{
    qDebug() << "ChatModulePlugin::destroyChat called";
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot destroy Chat - context not initialized.";
        return false;
    }
    
    int result = chat_destroy(chatCtx, destroy_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: Chat destroy initiated successfully";
        chatCtx = nullptr;
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to destroy Chat, error code:" << result;
        return false;
    }
}

bool ChatModulePlugin::setEventCallback()
{
    qDebug() << "ChatModulePlugin::setEventCallback called";
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot set event callback - context not initialized. Call initChat first.";
        return false;
    }
    
    set_event_callback(chatCtx, event_callback, this);
    
    qDebug() << "ChatModulePlugin: Event callback set successfully";
    return true;
}

// ============================================================================
// Client Info Methods
// ============================================================================

bool ChatModulePlugin::getId()
{
    qDebug() << "ChatModulePlugin::getId called";
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot get ID - context not initialized";
        return false;
    }
    
    int result = chat_get_id(chatCtx, get_id_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: Get ID initiated successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to get ID, error code:" << result;
        return false;
    }
}

// ============================================================================
// Conversation Operations
// ============================================================================

bool ChatModulePlugin::listConversations()
{
    qDebug() << "ChatModulePlugin::listConversations called";
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot list conversations - context not initialized";
        return false;
    }
    
    int result = chat_list_conversations(chatCtx, list_conversations_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: List conversations initiated successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to list conversations, error code:" << result;
        return false;
    }
}

bool ChatModulePlugin::getConversation(const QString &convoId)
{
    qDebug() << "ChatModulePlugin::getConversation called with convoId:" << convoId;
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot get conversation - context not initialized";
        return false;
    }
    
    QByteArray convoIdUtf8 = convoId.toUtf8();
    
    int result = chat_get_conversation(chatCtx, get_conversation_callback, this, convoIdUtf8.constData());
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: Get conversation initiated successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to get conversation, error code:" << result;
        return false;
    }
}

bool ChatModulePlugin::newPrivateConversation(const QString &introBundleStr, const QString &contentHex)
{
    qDebug() << "ChatModulePlugin::newPrivateConversation called";

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot create new private conversation - context not initialized";
        return false;
    }

    QByteArray introBundleUtf8 = introBundleStr.toUtf8();
    QByteArray contentUtf8 = contentHex.toUtf8();
    
    int result = chat_new_private_conversation(chatCtx, new_private_conversation_callback, this, introBundleUtf8.constData(), contentUtf8.constData());
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: New private conversation initiated successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to create new private conversation, error code:" << result;
        return false;
    }
}

bool ChatModulePlugin::sendMessage(const QString &convoId, const QString &contentHex)
{
    qDebug() << "ChatModulePlugin::sendMessage called with convoId:" << convoId;
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot send message - context not initialized";
        return false;
    }
    
    QByteArray convoIdUtf8 = convoId.toUtf8();
    QByteArray contentUtf8 = contentHex.toUtf8();
    
    int result = chat_send_message(chatCtx, send_message_callback, this, convoIdUtf8.constData(), contentUtf8.constData());
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: Send message initiated successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to send message, error code:" << result;
        return false;
    }
}

// ============================================================================
// Identity Operations
// ============================================================================

bool ChatModulePlugin::getIdentity()
{
    qDebug() << "ChatModulePlugin::getIdentity called";
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot get identity - context not initialized";
        return false;
    }
    
    int result = chat_get_identity(chatCtx, get_identity_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: Get identity initiated successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to get identity, error code:" << result;
        return false;
    }
}

bool ChatModulePlugin::createIntroBundle()
{
    qDebug() << "ChatModulePlugin::createIntroBundle called";
    
    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot create intro bundle - context not initialized";
        return false;
    }
    
    int result = chat_create_intro_bundle(chatCtx, create_intro_bundle_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatModulePlugin: Create intro bundle initiated successfully";
        return true;
    } else {
        qWarning() << "ChatModulePlugin: Failed to create intro bundle, error code:" << result;
        return false;
    }
}
