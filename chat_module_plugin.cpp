#include "chat_module_plugin.h"
#include <QDebug>
#include <QCoreApplication>
#include <QVariantList>
#include <QDateTime>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonArray>

static QString vecToQString(const Vec_uint8_t& v) {
    return QString::fromUtf8(reinterpret_cast<const char*>(v.ptr), static_cast<int>(v.len));
}

static QByteArray vecToByteArray(const Vec_uint8_t& v) {
    return QByteArray(reinterpret_cast<const char*>(v.ptr), static_cast<int>(v.len));
}

// Returned slice is only valid while the QByteArray lives
static slice_ref_uint8_t borrowSlice(const QByteArray& ba) {
    slice_ref_uint8_t s;
    s.ptr = reinterpret_cast<const uint8_t*>(ba.constData());
    s.len = static_cast<size_t>(ba.size());
    return s;
}

static QString timestamp() {
    return QDateTime::currentDateTime().toString(Qt::ISODate);
}

// ============================================================================
// Construction / Destruction
// ============================================================================

ChatModulePlugin::ChatModulePlugin()
    : chatCtx(nullptr)
    , m_logos(nullptr)
    , m_deliveryListenerActive(false)
{
    qDebug() << "ChatModulePlugin: Initialized";
}

ChatModulePlugin::~ChatModulePlugin()
{
    if (chatCtx) {
        destroyChat();
    }
    // m_logos references logosAPI, so destroy the dependent first
    delete m_logos;
    m_logos = nullptr;
    delete logosAPI;
    logosAPI = nullptr;
}

void ChatModulePlugin::initLogos(LogosAPI* logosAPIInstance) {
    if (logosAPI) {
        delete logosAPI;
    }
    logosAPI = logosAPIInstance;

    delete m_logos;
    m_logos = new LogosModules(logosAPI);
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
// Content Topic Helper
// ============================================================================

QString ChatModulePlugin::buildContentTopic(const char* addressBytes, size_t addressLen) {
    QString address = QString::fromUtf8(addressBytes, static_cast<int>(addressLen));
    return QString("/logos-chat/1/%1/proto").arg(address);
}

// ============================================================================
// Delivery Helpers
// ============================================================================

void ChatModulePlugin::deliverPayloads(const Payload_t* payloads, size_t count) {
    if (!m_logos) {
        qWarning() << "ChatModulePlugin: LogosModules not available";
        return;
    }

    for (size_t i = 0; i < count; i++) {
        const Payload_t& p = payloads[i];

        QString contentTopic = buildContentTopic(
            reinterpret_cast<const char*>(p.address.ptr), p.address.len);

        QByteArray payloadData(reinterpret_cast<const char*>(p.data.ptr),
                               static_cast<int>(p.data.len));
        QString payloadB64 = QString::fromLatin1(payloadData.toBase64());

        qDebug() << "ChatModulePlugin: Sending payload to" << contentTopic
                 << "size:" << p.data.len;

        LogosResult result = m_logos->delivery_module.send(contentTopic, payloadB64);

        if (!result.success) {
            qWarning() << "ChatModulePlugin: delivery.send() failed:" << result.getError();
        }
    }
}

void ChatModulePlugin::onDeliveryMessageReceived(const QVariantList& data) {
    if (!chatCtx) return;

    // data[0]=hash, data[1]=contentTopic, data[2]=payload(base64), data[3]=timestamp
    if (data.size() < 3) {
        qWarning() << "ChatModulePlugin: messageReceived event has insufficient data";
        return;
    }

    if (data[1].toString() != localContentTopic) return;

    QString payloadB64 = data[2].toString();

    // Two base64 layers to unwrap:
    //   1. Delivery module encodes all payloads as base64 for its JSON transport
    //   2. We base64-encode the raw binary in deliverPayloads() because delivery_module.send() takes QString
    // So: data[2] = deliveryBase64( ourBase64( rawProtobuf ) )
    QByteArray ourB64Bytes = QByteArray::fromBase64(payloadB64.toLatin1());
    QByteArray rawPayload = QByteArray::fromBase64(ourB64Bytes);

    // Pass to libchat
    slice_ref_uint8_t payloadSlice = borrowSlice(rawPayload);

    HandlePayloadResult_t result = {};
    handle_payload(chatCtx, payloadSlice, &result);

    if (result.error_code != 0) {
        qWarning() << "ChatModulePlugin: handle_payload error:" << result.error_code;
        destroy_handle_payload_result(&result);
        return;
    }

    if (result.content.len > 0) {
        QString convoId = vecToQString(result.convo_id);
        QByteArray content = vecToByteArray(result.content);
        QString contentStr = QString::fromUtf8(content);

        if (result.is_new_convo) {
            QJsonObject convoObj;
            convoObj["conversationId"] = convoId;
            convoObj["conversationType"] = "private";
            QString convoJson = QJsonDocument(convoObj).toJson(QJsonDocument::Compact);

            QVariantList eventData;
            eventData << convoJson;
            eventData << timestamp();
            emitEvent("chatNewConversation", eventData);
        }

        QJsonObject msgObj;
        msgObj["conversationId"] = convoId;
        msgObj["content"] = contentStr;
        msgObj["sender"] = "Peer";
        QString msgJson = QJsonDocument(msgObj).toJson(QJsonDocument::Compact);

        QVariantList eventData;
        eventData << msgJson;
        eventData << timestamp();
        emitEvent("chatNewMessage", eventData);
    }

    destroy_handle_payload_result(&result);
}

// ============================================================================
// Client Lifecycle
// ============================================================================

bool ChatModulePlugin::initChat(const QString &configJson)
{
    qDebug() << "ChatModulePlugin::initChat called";

    QString name = "default";
    QJsonDocument doc = QJsonDocument::fromJson(configJson.toUtf8());
    if (doc.isObject()) {
        QJsonObject obj = doc.object();
        if (obj.contains("name")) {
            name = obj["name"].toString();
        }
    }

    if (chatCtx) {
        destroy_context(chatCtx);
        chatCtx = nullptr;
    }

    QByteArray nameUtf8 = name.toUtf8();
    slice_ref_uint8_t nameSlice = borrowSlice(nameUtf8);
    chatCtx = create_context(nameSlice);

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Failed to create context";
        QVariantList eventData;
        eventData << false << -1 << "Failed to create context" << timestamp();
        emitEvent("chatInitResult", eventData);
        return false;
    }

    Vec_uint8_t addrVec = local_delivery_address(chatCtx);
    localContentTopic = buildContentTopic(
        reinterpret_cast<const char*>(addrVec.ptr), addrVec.len);
    destroy_string(addrVec);

    qDebug() << "ChatModulePlugin: Local content topic:" << localContentTopic;

    // Build delivery module config from chat config fields
    if (m_logos) {
        QJsonObject deliveryCfg;
        deliveryCfg["logLevel"] = "INFO";

        if (doc.isObject()) {
            QJsonObject chatCfg = doc.object();
            int port = chatCfg["port"].toInt(0);
            if (port > 0) {
                deliveryCfg["tcpPort"] = port;
                deliveryCfg["discv5UdpPort"] = port + 1000;
            }
            if (chatCfg.contains("clusterId"))
                deliveryCfg["clusterId"] = chatCfg["clusterId"].toInt();
            if (chatCfg.contains("preset"))
                deliveryCfg["preset"] = chatCfg["preset"].toString();
        }

        QString deliveryConfigJson = QJsonDocument(deliveryCfg).toJson(QJsonDocument::Compact);
        qDebug() << "ChatModulePlugin: Delivery config:" << deliveryConfigJson;

        if (!m_logos->delivery_module.createNode(deliveryConfigJson)) {
            qWarning() << "ChatModulePlugin: Failed to create delivery node";
            QVariantList eventData;
            eventData << false << -1 << "Failed to create delivery node" << timestamp();
            emitEvent("chatInitResult", eventData);
            return false;
        }
        qDebug() << "ChatModulePlugin: Delivery node created";
    }

    QVariantList eventData;
    eventData << true << 0 << "Context created" << timestamp();
    emitEvent("chatInitResult", eventData);
    return true;
}

bool ChatModulePlugin::startChat()
{
    qDebug() << "ChatModulePlugin::startChat called";

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot start - not initialized";
        return false;
    }

    if (!m_logos) {
        qWarning() << "ChatModulePlugin: Cannot start - LogosModules not available";
        return false;
    }

    // Start the delivery node
    if (!m_logos->delivery_module.start()) {
        qWarning() << "ChatModulePlugin: Failed to start delivery node";
        return false;
    }
    qDebug() << "ChatModulePlugin: Delivery node started";

    bool subResult = m_logos->delivery_module.subscribe(localContentTopic);
    qDebug() << "ChatModulePlugin: subscribe result:" << subResult;

    if (!m_deliveryListenerActive) {
        if (!m_logos->delivery_module.on("messageReceived", [this](const QVariantList& data) {
                onDeliveryMessageReceived(data);
            })) {
            qWarning() << "ChatModulePlugin: Failed to subscribe to messageReceived events";
        } else {
            m_deliveryListenerActive = true;
        }
    }

    QVariantList eventData;
    eventData << true << 0 << "Chat started" << timestamp();
    emitEvent("chatStartResult", eventData);
    return true;
}

bool ChatModulePlugin::stopChat()
{
    qDebug() << "ChatModulePlugin::stopChat called";

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot stop - not initialized";
        return false;
    }

    if (m_logos && !localContentTopic.isEmpty()) {
        m_logos->delivery_module.unsubscribe(localContentTopic);
        m_logos->delivery_module.stop();
    }

    QVariantList eventData;
    eventData << true << 0 << "Chat stopped" << timestamp();
    emitEvent("chatStopResult", eventData);
    return true;
}

bool ChatModulePlugin::destroyChat()
{
    qDebug() << "ChatModulePlugin::destroyChat called";

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot destroy - not initialized";
        return false;
    }

    stopChat();

    destroy_context(chatCtx);
    chatCtx = nullptr;

    QVariantList eventData;
    eventData << "Context destroyed" << timestamp();
    emitEvent("chatDestroyResult", eventData);
    return true;
}

bool ChatModulePlugin::setEventCallback()
{
    qDebug() << "ChatModulePlugin::setEventCallback called (no-op, handled in startChat)";
    return chatCtx != nullptr;
}

// ============================================================================
// Client Info
// ============================================================================

bool ChatModulePlugin::getId()
{
    qDebug() << "ChatModulePlugin::getId called";

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot get ID - not initialized";
        return false;
    }

    Vec_uint8_t nameVec = installation_name(chatCtx);
    QString name = vecToQString(nameVec);
    destroy_string(nameVec);

    QVariantList eventData;
    eventData << name << timestamp();
    emitEvent("chatGetIdResult", eventData);
    return true;
}

// ============================================================================
// Conversation Operations
// ============================================================================

bool ChatModulePlugin::listConversations()
{
    qDebug() << "ChatModulePlugin::listConversations called";

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot list conversations - not initialized";
        return false;
    }

    ListConvoResult_t result = list_conversations(chatCtx);

    if (result.error_code != 0) {
        qWarning() << "ChatModulePlugin: list_conversations error:" << result.error_code;
        destroy_list_result(result);
        return false;
    }

    QJsonArray ids;
    for (size_t i = 0; i < result.convo_ids.len; i++) {
        ids.append(vecToQString(result.convo_ids.ptr[i]));
    }
    QString idsJson = QJsonDocument(ids).toJson(QJsonDocument::Compact);

    destroy_list_result(result);

    QVariantList eventData;
    eventData << idsJson << timestamp();
    emitEvent("chatListConversationsResult", eventData);
    return true;
}

bool ChatModulePlugin::getConversation(const QString &convoId)
{
    qDebug() << "ChatModulePlugin::getConversation called:" << convoId;

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot get conversation - not initialized";
        return false;
    }

    // TODO: libchat doesn't expose a single-conversation query yet — echoes back the ID
    QJsonObject obj;
    obj["conversationId"] = convoId;
    QString json = QJsonDocument(obj).toJson(QJsonDocument::Compact);

    QVariantList eventData;
    eventData << json << timestamp();
    emitEvent("chatGetConversationResult", eventData);
    return true;
}

bool ChatModulePlugin::newPrivateConversation(const QString &introBundleStr, const QString &contentHex)
{
    qDebug() << "ChatModulePlugin::newPrivateConversation called";

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot create conversation - not initialized";
        return false;
    }

    // introBundleStr is the text format directly: "logos_chatintro_1_..."
    QByteArray bundleBytes = introBundleStr.toUtf8();
    slice_ref_uint8_t bundleSlice = borrowSlice(bundleBytes);

    QByteArray contentBytes = QByteArray::fromHex(contentHex.toLatin1());
    slice_ref_uint8_t contentSlice = borrowSlice(contentBytes);

    NewConvoResult_t result = {};
    create_new_private_convo(chatCtx, bundleSlice, contentSlice, &result);

    if (result.error_code != 0) {
        qWarning() << "ChatModulePlugin: create_new_private_convo error:" << result.error_code;
        QVariantList eventData;
        eventData << false << result.error_code << "" << timestamp();
        emitEvent("chatNewPrivateConversationResult", eventData);
        destroy_convo_result(&result);
        return true; // Request was accepted, error is in the event
    }

    QString convoId = vecToQString(result.convo_id);

    deliverPayloads(result.payloads.ptr, result.payloads.len);

    // Emit chatNewConversation for the initiator so the UI adds it to the list
    QJsonObject convoObj;
    convoObj["conversationId"] = convoId;
    convoObj["conversationType"] = "private";
    QString convoJson = QJsonDocument(convoObj).toJson(QJsonDocument::Compact);
    QVariantList convoEventData;
    convoEventData << convoJson << timestamp();
    emitEvent("chatNewConversation", convoEventData);

    QVariantList eventData;
    eventData << true << 0 << convoJson << timestamp();
    emitEvent("chatNewPrivateConversationResult", eventData);

    destroy_convo_result(&result);
    return true;
}

bool ChatModulePlugin::sendMessage(const QString &convoId, const QString &contentHex)
{
    qDebug() << "ChatModulePlugin::sendMessage called, convoId:" << convoId;

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot send message - not initialized";
        return false;
    }

    QByteArray convoIdUtf8 = convoId.toUtf8();

    QByteArray contentBytes = QByteArray::fromHex(contentHex.toLatin1());
    slice_ref_uint8_t contentSlice = borrowSlice(contentBytes);

    slice_ref_uint8_t convoIdSlice = borrowSlice(convoIdUtf8);

    SendContentResult_t result = {};
    send_content(chatCtx, convoIdSlice, contentSlice, &result);

    if (result.error_code != 0) {
        qWarning() << "ChatModulePlugin: send_content error:" << result.error_code;
        QVariantList eventData;
        eventData << false << result.error_code << "" << timestamp();
        emitEvent("chatSendMessageResult", eventData);
        destroy_send_content_result(&result);
        return true;
    }

    deliverPayloads(result.payloads.ptr, result.payloads.len);

    QVariantList eventData;
    eventData << true << 0 << "{}" << timestamp();
    emitEvent("chatSendMessageResult", eventData);

    destroy_send_content_result(&result);
    return true;
}

// ============================================================================
// Identity Operations
// ============================================================================

bool ChatModulePlugin::getIdentity()
{
    qDebug() << "ChatModulePlugin::getIdentity called";

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot get identity - not initialized";
        return false;
    }

    Vec_uint8_t nameVec = installation_name(chatCtx);
    Vec_uint8_t addrVec = local_delivery_address(chatCtx);

    QJsonObject obj;
    obj["installationName"] = vecToQString(nameVec);
    obj["deliveryAddress"] = vecToQString(addrVec);
    QString json = QJsonDocument(obj).toJson(QJsonDocument::Compact);

    destroy_string(nameVec);
    destroy_string(addrVec);

    QVariantList eventData;
    eventData << json << timestamp();
    emitEvent("chatGetIdentityResult", eventData);
    return true;
}

bool ChatModulePlugin::createIntroBundle()
{
    qDebug() << "ChatModulePlugin::createIntroBundle called";

    if (!chatCtx) {
        qWarning() << "ChatModulePlugin: Cannot create intro bundle - not initialized";
        return false;
    }

    CreateIntroResult_t result = {};
    create_intro_bundle(chatCtx, &result);

    if (result.error_code != 0) {
        qWarning() << "ChatModulePlugin: create_intro_bundle error:" << result.error_code;
        QVariantList eventData;
        eventData << false << result.error_code << "" << timestamp();
        emitEvent("chatCreateIntroBundleResult", eventData);
        destroy_intro_result(&result);
        return true;
    }

    // intro_bytes is already a text format: "logos_chatintro_1_" + base64(protobuf)
    QString bundleStr = QString::fromUtf8(
        reinterpret_cast<const char*>(result.intro_bytes.ptr),
        static_cast<int>(result.intro_bytes.len));

    QVariantList eventData;
    eventData << true << 0 << bundleStr << timestamp();
    emitEvent("chatCreateIntroBundleResult", eventData);

    destroy_intro_result(&result);
    return true;
}
