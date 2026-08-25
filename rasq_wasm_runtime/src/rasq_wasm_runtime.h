#include <stdint.h>

#if _WIN32
#define FFI_PLUGIN_EXPORT __declspec(dllexport)
#else
#define FFI_PLUGIN_EXPORT
#endif

// Opaque handle to a loaded wasm plugin instance (wasmi Store + Instance),
// implemented in ../../rust/rasq_wasm_runtime_core.
typedef struct RasqWasmInstance RasqWasmInstance;

// Instantiates a wasm module from raw bytes. Returns NULL on failure
// (invalid module, missing `memory`/`alloc`/`dealloc` exports, ...).
// The `rasq.host_fn` import answers E00 (no handler) for every call.
FFI_PLUGIN_EXPORT RasqWasmInstance *rasq_wasm_instantiate(const uint8_t *bytes_ptr, intptr_t bytes_len);

// Host-function handler serving the plugin's `rasq.host_fn` import
// (PROTOCOL.md §4.1, synchronous v1). Receives a `rasq.hostfn/1` request
// JSON; must return a response JSON allocated with rasq_wasm_string_dup
// (the runtime frees it), or NULL for "unhandled" (becomes an E00 error).
typedef char *(*RasqHostFnCallback)(const char *request, void *user_data);

// Instantiates with a host-function handler. `user_data` is passed through
// to every callback invocation untouched.
FFI_PLUGIN_EXPORT RasqWasmInstance *rasq_wasm_instantiate_with_host(const uint8_t *bytes_ptr, intptr_t bytes_len, RasqHostFnCallback callback, void *user_data);

// Copies a NUL-terminated string into the runtime's allocator. Host-function
// callbacks MUST return strings allocated here.
FFI_PLUGIN_EXPORT char *rasq_wasm_string_dup(const char *s);

// Frees an instance created by rasq_wasm_instantiate.
FFI_PLUGIN_EXPORT void rasq_wasm_destroy(RasqWasmInstance *instance);

// Calls the plugin's `snapshot` export (PROTOCOL.md §3.1). Returns a
// `rasq.boundary/1` JSON string owned by the caller; free it with
// rasq_wasm_free_string. On failure, returns a JSON `{"ok":false,"error":...}`
// string instead (never NULL).
FFI_PLUGIN_EXPORT char *rasq_wasm_snapshot(RasqWasmInstance *instance);

// Calls the plugin's `patch` export. See rasq_wasm_snapshot.
FFI_PLUGIN_EXPORT char *rasq_wasm_patch(RasqWasmInstance *instance);

// Delivers a host `call { fn, args, callId }` (PROTOCOL.md §3.2) to the
// plugin and returns its `result` message. See rasq_wasm_snapshot.
FFI_PLUGIN_EXPORT char *rasq_wasm_call(RasqWasmInstance *instance, const char *fn_name, const char *args_json, const char *call_id);

// Frees a string returned by rasq_wasm_snapshot/rasq_wasm_patch/rasq_wasm_call.
FFI_PLUGIN_EXPORT void rasq_wasm_free_string(char *s);
