// Mock implementation of liblogoschat C functions.
// Replaces the real C library at link time during unit tests.
//
// Callbacks are invoked synchronously so that the plugin's methods
// (which directly call the C function and return based on the result)
// get their callback fired before the method returns.
//
// Return values and callback messages are controlled via LogosCMockStore:
//   t.mockCFunction("chat_new").returns(1);    // make chat_new succeed
//   t.mockCFunction("chat_start").returns(0);  // force RET_OK

#include <logos_clib_mock.h>
#include <cstring>

#define RET_OK  0
#define RET_ERR 1

typedef void (*chat_callback)(int callerRet, const char* msg, size_t len, void* userData);

// Sentinel address used as a fake non-null chat context.
static char s_fakeCtx = 0;

// Helper: invoke callback with RET_OK and the string configured in the mock store.
static void invokeOk(const char* funcName, chat_callback cb, void* userData) {
    if (!cb) return;
    const char* msg = LogosCMockStore::instance().getReturnString(funcName);
    cb(RET_OK, msg ? msg : "", msg ? strlen(msg) : 0, userData);
}

extern "C" {

void* chat_new(const char* /*cfg*/, chat_callback cb, void* userData) {
    LOGOS_CMOCK_RECORD("chat_new");
    int ok = LOGOS_CMOCK_RETURN(int, "chat_new");
    if (ok && cb) {
        cb(RET_OK, "", 0, userData);
    } else if (!ok && cb) {
        cb(RET_ERR, "mock: chat_new fail", 18, userData);
    }
    return ok ? static_cast<void*>(&s_fakeCtx) : nullptr;
}

int chat_start(void* /*ctx*/, chat_callback cb, void* userData) {
    LOGOS_CMOCK_RECORD("chat_start");
    invokeOk("chat_start", cb, userData);
    return RET_OK;
}

int chat_stop(void* /*ctx*/, chat_callback cb, void* userData) {
    LOGOS_CMOCK_RECORD("chat_stop");
    invokeOk("chat_stop", cb, userData);
    return RET_OK;
}

int chat_destroy(void* /*ctx*/, chat_callback cb, void* userData) {
    LOGOS_CMOCK_RECORD("chat_destroy");
    invokeOk("chat_destroy", cb, userData);
    return RET_OK;
}

void set_event_callback(void* /*ctx*/, chat_callback /*cb*/, void* /*userData*/) {
    LOGOS_CMOCK_RECORD("set_event_callback");
}

int chat_get_id(void* /*ctx*/, chat_callback cb, void* userData) {
    LOGOS_CMOCK_RECORD("chat_get_id");
    invokeOk("chat_get_id", cb, userData);
    return RET_OK;
}

int chat_list_conversations(void* /*ctx*/, chat_callback cb, void* userData) {
    LOGOS_CMOCK_RECORD("chat_list_conversations");
    invokeOk("chat_list_conversations", cb, userData);
    return RET_OK;
}

int chat_get_conversation(void* /*ctx*/, chat_callback cb, void* userData, const char* /*convoId*/) {
    LOGOS_CMOCK_RECORD("chat_get_conversation");
    invokeOk("chat_get_conversation", cb, userData);
    return RET_OK;
}

int chat_new_private_conversation(void* /*ctx*/, chat_callback cb, void* userData, const char* /*introBundleStr*/, const char* /*contentHex*/) {
    LOGOS_CMOCK_RECORD("chat_new_private_conversation");
    invokeOk("chat_new_private_conversation", cb, userData);
    return RET_OK;
}

int chat_send_message(void* /*ctx*/, chat_callback cb, void* userData, const char* /*convoId*/, const char* /*contentHex*/) {
    LOGOS_CMOCK_RECORD("chat_send_message");
    invokeOk("chat_send_message", cb, userData);
    return RET_OK;
}

int chat_get_identity(void* /*ctx*/, chat_callback cb, void* userData) {
    LOGOS_CMOCK_RECORD("chat_get_identity");
    invokeOk("chat_get_identity", cb, userData);
    return RET_OK;
}

int chat_create_intro_bundle(void* /*ctx*/, chat_callback cb, void* userData) {
    LOGOS_CMOCK_RECORD("chat_create_intro_bundle");
    invokeOk("chat_create_intro_bundle", cb, userData);
    return RET_OK;
}

} // extern "C"
