#pragma once

#include <cstdint>

// Rust implements these functions and catches panics before crossing this ABI.
extern "C" {
bool craft_initialize(const char* files, const char* cache, float density);
// Safe before initialization; later changes are serialized on the render worker.
bool craft_set_system_language(const char* languageTag);
void craft_surface_created(void* window, uint32_t width, uint32_t height);
void craft_surface_changed(uint32_t width, uint32_t height);
void craft_surface_destroyed();
void craft_frame(uint64_t timestamp, uint64_t targetTimestamp);
// Coordinates and scroll deltas are logical points, surface dimensions are pixels.
// action: 0 down, 1 up, 2 move, 3 cancel, 4 leave.
// button: 1 primary, 2 secondary, 4 middle, 8 back, 16 forward.
// pressure: -1 for a mouse/finger; only a pen carries a pressure sample.
void craft_pointer(float x, float y, int32_t action, int32_t button,
                   float pressure, float tiltX, float tiltY);
void craft_key(int32_t code, bool pressed, bool ctrl, bool shift, bool alt, const char* text);
void craft_scroll(float dx, float dy);
// Multiplicative zoom delta for one pinch update, not the cumulative scale.
void craft_zoom(float factor);
bool craft_open_document(const char* path);
bool craft_save_document(const char* path);
// Pull one asynchronous original-menu file request as JSON, or an empty string.
// Copy the returned string before calling another Rust bridge function.
const char* craft_take_file_request();
// Re-encode the pending Save snapshot to the picker-selected filename format.
// Returns the actual sandbox stage path or an empty string on failure.
const char* craft_prepare_file_save(uint64_t id, const char* destinationName);
// result is success, cancel, or error.
bool craft_complete_file_request(uint64_t id, const char* result,
                                 const char* resolvedPath, const char* displayName, const char* error);
// Sends a file/drag error without a pending picker request to the original UI.
void craft_report_file_error(const char* message);
// Independent host output snapshot/events and bounded nonblocking input queue.
const char* craft_take_platform_output();
bool craft_platform_input(const char* json);
const char* craft_last_error();
#ifdef PHOTOCRAFT_DEVICE_TESTS
uint32_t craft_device_tests_abi_v1();
// Nonblocking, isolated-session-only control. Copy returned strings immediately.
const char* craft_test_submit(const char* runId, const char* requestJson);
const char* craft_test_poll(const char* runId, const char* ticket);
const char* craft_test_snapshot(const char* runId);
#endif
// C++ implements this sink. Rust can call it from its render worker without
// re-entering the bridge or taking the bridge lifecycle mutex.
void craft_log(int32_t level, const char* message) noexcept;
}
