// Stub implementations for logos_events: methods.
// In the real build, the codegen generates chat_module_events.cpp with
// bodies that route through LogosModuleContext::emitEventImpl_. For unit tests,
// the codegen doesn't run so we provide no-op stubs.

#include "chat_module_plugin.h"

void ChatModuleImpl::chatInitResult(bool, int64_t, const std::string&, const std::string&) {}
void ChatModuleImpl::chatStartResult(bool, int64_t, const std::string&, const std::string&) {}
void ChatModuleImpl::chatStopResult(bool, int64_t, const std::string&, const std::string&) {}
void ChatModuleImpl::chatDestroyResult(const std::string&, const std::string&) {}
void ChatModuleImpl::chatGetIdResult(const std::string&, const std::string&) {}
void ChatModuleImpl::chatListConversationsResult(const std::string&, const std::string&) {}
void ChatModuleImpl::chatGetConversationResult(const std::string&, const std::string&) {}
void ChatModuleImpl::chatNewPrivateConversationResult(bool, int64_t, const std::string&, const std::string&) {}
void ChatModuleImpl::chatSendMessageResult(bool, int64_t, const std::string&, const std::string&) {}
void ChatModuleImpl::chatGetIdentityResult(const std::string&, const std::string&) {}
void ChatModuleImpl::chatCreateIntroBundleResult(bool, int64_t, const std::string&, const std::string&) {}
void ChatModuleImpl::chatNewMessage(const std::string&, const std::string&) {}
void ChatModuleImpl::chatNewConversation(const std::string&, const std::string&) {}
void ChatModuleImpl::chatDeliveryAck(const std::string&, const std::string&) {}
void ChatModuleImpl::chatEvent(const std::string&, const std::string&) {}
