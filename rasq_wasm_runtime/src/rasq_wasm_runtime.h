#include <stdbool.h>
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

// Enables the runtime-native `net.fetch` backend (blocking HTTP inside the
// runtime; works on mobile where the embedder has no synchronous HTTP).
// While enabled, net.fetch never reaches the host-function callback. The
// calling thread blocks for the duration of each request.
FFI_PLUGIN_EXPORT void rasq_wasm_set_native_fetch(RasqWasmInstance *instance, bool enabled);

// Overrides the per-invocation fuel budget (roughly "instructions executed";
// default = rasq_wasm_fuel_budget_media()). Every snapshot/patch/call is
// refuelled to this amount; a guest that burns through it fails with error
// code E02 ("plugin exceeded its execution budget") instead of wedging the
// calling thread forever. 0 = every call fails immediately (hard stop).
FFI_PLUGIN_EXPORT void rasq_wasm_set_fuel_per_call(RasqWasmInstance *instance, uint64_t fuel);

// Recommended budgets, measured against plugin-sample/dist. The default a
// fresh instance starts with is the *media* one, so an embedder that never
// calls the setter is already correct for the heavy case; call the setter
// with the standard budget for plugins that do not process bitmaps.
//
// The axis is whether the plugin touches bitmaps (the `composer.media` slot,
// or the replace_media/tag_media permissions) — NOT the manifest `kind`:
// the heaviest sample plugin (sample.media-process, 2.1G fuel for one
// media.edit) declares kind "action", while "mediaViewer" is the lightest
// thing measured (118K). `kind` says where a plugin plugs in, not what it
// costs.
FFI_PLUGIN_EXPORT uint64_t rasq_wasm_fuel_budget_media(void);
FFI_PLUGIN_EXPORT uint64_t rasq_wasm_fuel_budget_standard(void);

// Replaces the set of hosts the runtime-native net.fetch may reach: a
// comma-separated list ("misskey.io,*.example.com"), matched against the
// URL's host (lower-cased; userinfo and port stripped). "*.example.com"
// matches subdomains only; "*" matches anything. NULL or "" means NO host —
// a fresh instance starts closed, so an embedder that never calls this
// cannot hand a plugin the whole internet by omission. Redirects are not
// followed (the 3xx is returned), so a permitted host cannot bounce the
// request somewhere undeclared. Call again when the session's instance
// host changes.
FFI_PLUGIN_EXPORT void rasq_wasm_set_fetch_hosts(RasqWasmInstance *instance, const char *hosts);

// Grants or revokes gpu.compute (D-081) for this instance. Pass true only
// when the plugin's manifest declares the `gpu_ml` permission and the store
// review approved it. gpu.compute is served inside the runtime, so the
// host-function callback's own permission checks never see it — this flag
// is the gate. A fresh instance starts closed: every gpu.compute answers
// E01 (permission denied) until this is called with true. Requests are also
// bounded inside the runtime (input 64 MiB, outputSize a multiple of 4 up to
// 64 MiB, WGSL 256 KiB, workgroups <= the device limit per dimension and
// 2^20 in total, 10 s wall clock) — see docs/host-functions.md.
FFI_PLUGIN_EXPORT void rasq_wasm_set_gpu_compute(RasqWasmInstance *instance, bool enabled);

// Takes the net.fetch request *shapes* this instance has seen for the first
// time since the previous call, as a JSON array of {hash, shape} (or a single
// {overflow: true} once the per-instance cap is exceeded). A shape carries no
// values: host, method, a path template ("/api/users/{hex}/notes"), query
// keys, header names and the key skeleton of a JSON body. The embedder
// reports first-seen shapes to the store (with the user's consent) so the
// review can re-read a version whose traffic pattern changed. Never NULL
// for a live instance; free with rasq_wasm_free_string.
FFI_PLUGIN_EXPORT char *rasq_wasm_take_fetch_shapes(RasqWasmInstance *instance);

// Overrides the wall-clock budget of one runtime-native net.fetch, in
// milliseconds (default 15000). Only meaningful with
// rasq_wasm_set_native_fetch(.., true): when the embedder serves net.fetch
// itself, bounding that call is the embedder's job — fuel does not tick
// while a host function blocks.
FFI_PLUGIN_EXPORT void rasq_wasm_set_fetch_timeout_ms(RasqWasmInstance *instance, uint64_t timeout_ms);

// Overrides the wall-clock budget of one guest invocation (call / snapshot /
// patch), in milliseconds (default 30000). Every host call checks the
// remaining time first and a runtime-native net.fetch never waits longer than
// what is left, so repeated slow fetches cannot hold the calling thread past
// this budget (E02).
FFI_PLUGIN_EXPORT void rasq_wasm_set_call_timeout_ms(RasqWasmInstance *instance, uint64_t timeout_ms);

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

// Decompresses a zstd frame (.rasqpkg = zstd-compressed tar, D-034). Returns a
// buffer owned by the caller (free with rasq_bytes_free(ptr, *out_len)), or
// NULL on failure: corrupt input, or an output larger than `max_out` bytes
// (decompression-bomb guard). `*out_len` is set on success.
FFI_PLUGIN_EXPORT uint8_t *rasq_zstd_decompress(const uint8_t *bytes_ptr, intptr_t bytes_len, intptr_t max_out, intptr_t *out_len);

// Frees a buffer returned by rasq_zstd_decompress.
FFI_PLUGIN_EXPORT void rasq_bytes_free(uint8_t *ptr, intptr_t len);
