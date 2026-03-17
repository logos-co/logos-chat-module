#include "chatsdk_module_plugin.h"
#include <QDebug>
#include <QCoreApplication>
#include <QVariantList>
#include <QDateTime>
#include <QJsonDocument>
#include <QJsonObject>

ChatSDKModulePlugin::ChatSDKModulePlugin() : chatCtx(nullptr)
{
    qDebug() << "ChatSDKModulePlugin: Initializing...";
    qDebug() << "ChatSDKModulePlugin: Initialized successfully";
}

ChatSDKModulePlugin::~ChatSDKModulePlugin() 
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

void ChatSDKModulePlugin::initLogos(LogosAPI* logosAPIInstance) {
    if (logosAPI) {
        delete logosAPI;
    }
    logosAPI = logosAPIInstance;
}

void ChatSDKModulePlugin::emitEvent(const QString& eventName, const QVariantList& data) {
    emit eventResponse(eventName, data);
}

// ============================================================================
// Static Callback Functions
// ============================================================================

void ChatSDKModulePlugin::init_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::init_callback called with ret:" << callerRet;
    
    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::init_callback: Invalid userData";
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

void ChatSDKModulePlugin::start_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::start_callback called with ret:" << callerRet;
    
    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::start_callback: Invalid userData";
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

void ChatSDKModulePlugin::stop_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::stop_callback called with ret:" << callerRet;
    
    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::stop_callback: Invalid userData";
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

void ChatSDKModulePlugin::destroy_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::destroy_callback called with ret:" << callerRet;
    
    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::destroy_callback: Invalid userData";
        return;
    }

    if (msg && len > 0) {
        QString message = QString::fromUtf8(msg, len);
        qDebug() << "ChatSDKModulePlugin::destroy_callback message:" << message;

        QVariantList eventData;
        eventData << message;
        eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

        plugin->emitEvent("chatsdkDestroyResult", eventData);
    }
}

void ChatSDKModulePlugin::event_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::event_callback called with ret:" << callerRet;

    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::event_callback: Invalid userData";
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

void ChatSDKModulePlugin::get_id_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::get_id_callback called with ret:" << callerRet;

    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::get_id_callback: Invalid userData";
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

void ChatSDKModulePlugin::list_conversations_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::list_conversations_callback called with ret:" << callerRet;

    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::list_conversations_callback: Invalid userData";
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

void ChatSDKModulePlugin::get_conversation_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::get_conversation_callback called with ret:" << callerRet;

    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::get_conversation_callback: Invalid userData";
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

void ChatSDKModulePlugin::new_private_conversation_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::new_private_conversation_callback called with ret:" << callerRet;

    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::new_private_conversation_callback: Invalid userData";
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

void ChatSDKModulePlugin::send_message_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::send_message_callback called with ret:" << callerRet;

    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::send_message_callback: Invalid userData";
        return;
    }

    QString resultJson = (msg && len > 0) ? QString::fromUtf8(msg, len) : "";
    qDebug() << "ChatSDKModulePlugin::send_message_callback result:" << resultJson;
    
    QVariantList eventData;
    eventData << (callerRet == RET_OK);  // success
    eventData << callerRet;               // return code
    eventData << resultJson;              // result JSON (may contain message ID)
    eventData << QDateTime::currentDateTime().toString(Qt::ISODate);

    plugin->emitEvent("chatsdkSendMessageResult", eventData);
}

void ChatSDKModulePlugin::get_identity_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::get_identity_callback called with ret:" << callerRet;

    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::get_identity_callback: Invalid userData";
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

void ChatSDKModulePlugin::create_intro_bundle_callback(int callerRet, const char* msg, size_t len, void* userData)
{
    qDebug() << "ChatSDKModulePlugin::create_intro_bundle_callback called with ret:" << callerRet;

    ChatSDKModulePlugin* plugin = static_cast<ChatSDKModulePlugin*>(userData);
    if (!plugin) {
        qWarning() << "ChatSDKModulePlugin::create_intro_bundle_callback: Invalid userData";
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

bool ChatSDKModulePlugin::initChat(const QString &configJson)
{
    qDebug() << "ChatSDKModulePlugin::initChat called with config:" << configJson;
    
    // Convert QString to UTF-8 byte array
    QByteArray cfgUtf8 = configJson.toUtf8();
    
    // Call chat_new with the configuration
    chatCtx = chat_new(cfgUtf8.constData(), init_callback, this);
    
    if (chatCtx) {
        qDebug() << "ChatSDKModulePlugin: Chat context created successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to create Chat context";
        return false;
    }
}

bool ChatSDKModulePlugin::startChat()
{
    qDebug() << "ChatSDKModulePlugin::startChat called";
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot start Chat - context not initialized. Call initChat first.";
        return false;
    }
    
    int result = chat_start(chatCtx, start_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: Chat start initiated successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to start Chat, error code:" << result;
        return false;
    }
}

bool ChatSDKModulePlugin::stopChat()
{
    qDebug() << "ChatSDKModulePlugin::stopChat called";
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot stop Chat - context not initialized.";
        return false;
    }
    
    int result = chat_stop(chatCtx, stop_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: Chat stop initiated successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to stop Chat, error code:" << result;
        return false;
    }
}

bool ChatSDKModulePlugin::destroyChat()
{
    qDebug() << "ChatSDKModulePlugin::destroyChat called";
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot destroy Chat - context not initialized.";
        return false;
    }
    
    int result = chat_destroy(chatCtx, destroy_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: Chat destroy initiated successfully";
        chatCtx = nullptr;
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to destroy Chat, error code:" << result;
        return false;
    }
}

bool ChatSDKModulePlugin::setEventCallback()
{
    qDebug() << "ChatSDKModulePlugin::setEventCallback called";
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot set event callback - context not initialized. Call initChat first.";
        return false;
    }
    
    set_event_callback(chatCtx, event_callback, this);
    
    qDebug() << "ChatSDKModulePlugin: Event callback set successfully";
    return true;
}

// ============================================================================
// Client Info Methods
// ============================================================================

bool ChatSDKModulePlugin::getId()
{
    qDebug() << "ChatSDKModulePlugin::getId called";
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot get ID - context not initialized";
        return false;
    }
    
    int result = chat_get_id(chatCtx, get_id_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: Get ID initiated successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to get ID, error code:" << result;
        return false;
    }
}

// ============================================================================
// Conversation Operations
// ============================================================================

bool ChatSDKModulePlugin::listConversations()
{
    qDebug() << "ChatSDKModulePlugin::listConversations called";
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot list conversations - context not initialized";
        return false;
    }
    
    int result = chat_list_conversations(chatCtx, list_conversations_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: List conversations initiated successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to list conversations, error code:" << result;
        return false;
    }
}

bool ChatSDKModulePlugin::getConversation(const QString &convoId)
{
    qDebug() << "ChatSDKModulePlugin::getConversation called with convoId:" << convoId;
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot get conversation - context not initialized";
        return false;
    }
    
    QByteArray convoIdUtf8 = convoId.toUtf8();
    
    int result = chat_get_conversation(chatCtx, get_conversation_callback, this, convoIdUtf8.constData());
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: Get conversation initiated successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to get conversation, error code:" << result;
        return false;
    }
}

bool ChatSDKModulePlugin::newPrivateConversation(const QString &introBundleStr, const QString &contentHex)
{
    qDebug() << "ChatSDKModulePlugin::newPrivateConversation called";

    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot create new private conversation - context not initialized";
        return false;
    }

    QByteArray introBundleUtf8 = introBundleStr.toUtf8();
    QByteArray contentUtf8 = contentHex.toUtf8();
    
    int result = chat_new_private_conversation(chatCtx, new_private_conversation_callback, this, introBundleUtf8.constData(), contentUtf8.constData());
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: New private conversation initiated successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to create new private conversation, error code:" << result;
        return false;
    }
}

bool ChatSDKModulePlugin::sendMessage(const QString &convoId, const QString &contentHex)
{
    qDebug() << "ChatSDKModulePlugin::sendMessage called with convoId:" << convoId;
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot send message - context not initialized";
        return false;
    }
    
    QByteArray convoIdUtf8 = convoId.toUtf8();
    QByteArray contentUtf8 = contentHex.toUtf8();
    
    int result = chat_send_message(chatCtx, send_message_callback, this, convoIdUtf8.constData(), contentUtf8.constData());
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: Send message initiated successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to send message, error code:" << result;
        return false;
    }
}

// ============================================================================
// Identity Operations
// ============================================================================

bool ChatSDKModulePlugin::getIdentity()
{
    qDebug() << "ChatSDKModulePlugin::getIdentity called";
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot get identity - context not initialized";
        return false;
    }
    
    int result = chat_get_identity(chatCtx, get_identity_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: Get identity initiated successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to get identity, error code:" << result;
        return false;
    }
}

bool ChatSDKModulePlugin::createIntroBundle()
{
    qDebug() << "ChatSDKModulePlugin::createIntroBundle called";
    
    if (!chatCtx) {
        qWarning() << "ChatSDKModulePlugin: Cannot create intro bundle - context not initialized";
        return false;
    }
    
    int result = chat_create_intro_bundle(chatCtx, create_intro_bundle_callback, this);
    
    if (result == RET_OK) {
        qDebug() << "ChatSDKModulePlugin: Create intro bundle initiated successfully";
        return true;
    } else {
        qWarning() << "ChatSDKModulePlugin: Failed to create intro bundle, error code:" << result;
        return false;
    }
}
