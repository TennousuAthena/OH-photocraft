#pragma once

#include <napi/native_api.h>

namespace photocraft::platform {
// UI-thread operations; IME callbacks use their own text snapshot and only
// enqueue bounded Rust inputs. They never enter the renderer lifecycle mutex.
bool ApplyOutput(napi_env env, napi_value uiContext, napi_value outputJson,
                 double physicalOriginX, double physicalOriginY, int32_t windowId);
void Release() noexcept;
void SetFocused(bool focused) noexcept;
void SetSurfaceAvailable(bool available) noexcept;
void SetPointerInside(bool inside) noexcept;
bool ImeAttached() noexcept;
// Content-free release probes share a process-wide bounded diagnostic budget.
void LogKeyDiagnostic(bool pressed, bool hasTextFallback) noexcept;
}
