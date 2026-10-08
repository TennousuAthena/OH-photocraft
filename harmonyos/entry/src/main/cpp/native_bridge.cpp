#include "photocraft_ffi.h"
#include "platform_bridge.h"

#include <ace/xcomponent/native_interface_xcomponent.h>
#include <hilog/log.h>
#include <napi/native_api.h>
#include <native_window/external_window.h>

#include <algorithm>
#include <atomic>
#include <cmath>
#include <exception>
#include <limits>
#include <mutex>
#include <stdexcept>
#include <string>

namespace {
constexpr unsigned int CRAFT_LOG_DOMAIN = 0xD003F00;
constexpr const char* CRAFT_LOG_TAG = "PhotoCraft";
std::mutex bridgeMutex;
OH_NativeXComponent* activeComponent = nullptr;
std::atomic<OH_NativeXComponent*> inputComponent { nullptr };
OHNativeWindow* activeWindow = nullptr;
bool initialized = false;
bool surfaceAttached = false;
float density = 1.0f;
float lastPointerX = 0.0f;
float lastPointerY = 0.0f;
bool firstFrameReported = false;
bool primaryTouchActive = false;
int32_t primaryTouchId = 0;
bool modifiersKnown = false;
uint64_t lastModifiers = 0;
double lastPinchScale = 1.0;
std::string lastLoggedError;

void OnFrame(OH_NativeXComponent* component, uint64_t timestamp, uint64_t targetTimestamp) noexcept;

void LogError(const char* message) noexcept
{
    OH_LOG_Print(LOG_APP, LOG_ERROR, CRAFT_LOG_DOMAIN, CRAFT_LOG_TAG, "%{public}s", message);
}

void ReportRustError()
{
    const char* message = craft_last_error();
    const std::string current = message ? message : "";
    if (!current.empty() && current != lastLoggedError) {
        LogError(current.c_str());
    }
    lastLoggedError = current;
}

// No C++ exception may escape a callback into ArkUI or Node-API. The mutex also
// serializes rendering against surface destruction and document operations.
template <typename Function>
void NativeBoundary(Function&& function) noexcept
{
    try {
        std::lock_guard<std::mutex> lock(bridgeMutex);
        function();
    } catch (const std::exception& error) {
        LogError(error.what());
    } catch (...) {
        LogError("Unexpected exception at the native callback boundary");
    }
}

template <typename Function>
napi_value NapiBoundary(napi_env env, Function&& function) noexcept
{
    try {
        std::lock_guard<std::mutex> lock(bridgeMutex);
        return function();
    } catch (const std::exception& error) {
        napi_throw_error(env, nullptr, error.what());
    } catch (...) {
        napi_throw_error(env, nullptr, "Unexpected exception at the Node-API boundary");
    }
    return nullptr;
}

// Platform output/input use independent Rust mailboxes. No renderer lifecycle
// mutex may span an IME service call or one of its callbacks.
template <typename Function>
napi_value PlatformNapiBoundary(napi_env env, Function&& function) noexcept
{
    try {
        return function();
    } catch (const std::exception& error) {
        napi_throw_error(env, nullptr, error.what());
    } catch (...) {
        napi_throw_error(env, nullptr, "Unexpected exception at the platform Node-API boundary");
    }
    return nullptr;
}

void Check(napi_status status)
{
    if (status != napi_ok) {
        throw std::runtime_error("Invalid native bridge argument or Node-API operation");
    }
}

std::string ReadString(napi_env env, napi_value value)
{
    size_t length = 0;
    Check(napi_get_value_string_utf8(env, value, nullptr, 0, &length));
    if (length > 1024 * 1024) {
        throw std::runtime_error("Native bridge string exceeds 1 MiB");
    }
    std::string result(length + 1, '\0');
    Check(napi_get_value_string_utf8(env, value, result.data(), result.size(), &length));
    result.resize(length);
    if (result.find('\0') != std::string::npos) {
        throw std::runtime_error("Native bridge strings must not contain NUL characters");
    }
    return result;
}

uint64_t ReadRequestId(napi_env env, napi_value value)
{
    double requestedId = 0;
    Check(napi_get_value_double(env, value, &requestedId));
    constexpr double maxSafeInteger = 9007199254740991.0;
    if (!std::isfinite(requestedId) || requestedId < 0 || requestedId > maxSafeInteger ||
        std::trunc(requestedId) != requestedId) {
        throw std::runtime_error("Request id must be a nonnegative safe integer");
    }
    int64_t integerId = 0;
    Check(napi_get_value_int64(env, value, &integerId));
    return static_cast<uint64_t>(integerId);
}

napi_value Boolean(napi_env env, bool value)
{
    napi_value result = nullptr;
    Check(napi_get_boolean(env, value, &result));
    return result;
}

napi_value Undefined(napi_env env)
{
    napi_value result = nullptr;
    Check(napi_get_undefined(env, &result));
    return result;
}

template <size_t Count>
void Arguments(napi_env env, napi_callback_info info, napi_value (&args)[Count])
{
    size_t count = Count;
    Check(napi_get_cb_info(env, info, &count, args, nullptr, nullptr));
    if (count != Count) {
        throw std::runtime_error("Incorrect argument count for the PhotoCraft native bridge");
    }
}

bool SurfaceSize(OH_NativeXComponent* component, void* window, uint32_t& width, uint32_t& height)
{
    int32_t bufferWidth = 0;
    int32_t bufferHeight = 0;
    // GET_BUFFER_GEOMETRY has height first, unlike SET_BUFFER_GEOMETRY.
    if (OH_NativeWindow_NativeWindowHandleOpt(static_cast<OHNativeWindow*>(window),
            GET_BUFFER_GEOMETRY, &bufferHeight, &bufferWidth) == 0 && bufferWidth > 0 && bufferHeight > 0) {
        width = static_cast<uint32_t>(bufferWidth);
        height = static_cast<uint32_t>(bufferHeight);
        return true;
    }
    uint64_t componentWidth = 0;
    uint64_t componentHeight = 0;
    if (OH_NativeXComponent_GetXComponentSize(component, window, &componentWidth, &componentHeight) != 0 ||
        componentWidth == 0 || componentHeight == 0 ||
        componentWidth > std::numeric_limits<uint32_t>::max() ||
        componentHeight > std::numeric_limits<uint32_t>::max()) {
        return false;
    }
    width = static_cast<uint32_t>(componentWidth);
    height = static_cast<uint32_t>(componentHeight);
    return true;
}

void ReleaseInputs() noexcept
{
    primaryTouchActive = false;
    modifiersKnown = false;
    lastPinchScale = 1.0;
    craft_pointer(lastPointerX, lastPointerY, 3, 0, -1.0f, 0.0f, 0.0f);
    craft_key(KEY_UNKNOWN, false, false, false, false, "");
}

void AttachRenderer()
{
    uint32_t width = 0;
    uint32_t height = 0;
    if (initialized && activeWindow && SurfaceSize(activeComponent, activeWindow, width, height)) {
        craft_surface_created(activeWindow, width, height);
        ReportRustError();
        const char* error = craft_last_error();
        surfaceAttached = !error || *error == '\0';
        if (surfaceAttached) {
            OH_LOG_Print(LOG_APP, LOG_INFO, CRAFT_LOG_DOMAIN, CRAFT_LOG_TAG,
                "Surface attached: %{public}u x %{public}u pixels, density %{public}f", width, height, density);
        }
        firstFrameReported = false;
    }
}

void OnSurfaceCreated(OH_NativeXComponent* component, void* window) noexcept
{
    if (!component || component != inputComponent.load(std::memory_order_acquire)) {
        return;
    }
    photocraft::platform::SetSurfaceAvailable(false);
    bool created = false;
    NativeBoundary([&] {
        if (!component || !window) {
            return;
        }
        if (activeWindow) {
            OH_NativeXComponent_UnregisterOnFrameCallback(activeComponent);
            if (initialized) {
                ReleaseInputs();
                craft_surface_destroyed();
            }
            OH_NativeWindow_NativeObjectUnreference(activeWindow);
            activeWindow = nullptr;
        }
        if (OH_NativeWindow_NativeObjectReference(window) != 0) {
            LogError("Could not retain XComponent native window");
            return;
        }
        activeComponent = component;
        activeWindow = static_cast<OHNativeWindow*>(window);
        created = true;
        surfaceAttached = false;
        AttachRenderer();
        OH_NativeXComponent_ExpectedRateRange range { 30, 60, 60 };
        OH_NativeXComponent_SetExpectedFrameRateRange(component, &range);
        // Re-register on every surface creation: a minimized/reopened window
        // may recreate its surface without loading the native module again.
        if (OH_NativeXComponent_RegisterOnFrameCallback(component, OnFrame) != 0) {
            LogError("Could not register PhotoCraft XComponent frame callback");
        }
    });
    if (created) {
        photocraft::platform::SetSurfaceAvailable(true);
    }
}

void OnSurfaceChanged(OH_NativeXComponent* component, void* window) noexcept
{
    NativeBoundary([&] {
        if (!initialized || component != activeComponent || window != activeWindow) {
            return;
        }
        if (!surfaceAttached) {
            AttachRenderer();
            return;
        }
        uint32_t width = 0;
        uint32_t height = 0;
        if (SurfaceSize(component, window, width, height)) {
            craft_surface_changed(width, height);
            ReportRustError();
        }
    });
}

void OnSurfaceDestroyed(OH_NativeXComponent* component, void* window) noexcept
{
    bool destroyed = false;
    NativeBoundary([&] {
        if (component != activeComponent || window != activeWindow) {
            return;
        }
        OH_NativeXComponent_UnregisterOnFrameCallback(component);
        destroyed = true;
        if (initialized) {
            ReleaseInputs();
            craft_surface_destroyed();
        }
        OH_NativeWindow_NativeObjectUnreference(activeWindow);
        activeWindow = nullptr;
        activeComponent = nullptr;
        surfaceAttached = false;
    });
    if (destroyed) {
        photocraft::platform::SetSurfaceAvailable(false);
    }
}

void OnFrame(OH_NativeXComponent* component, uint64_t timestamp, uint64_t targetTimestamp) noexcept
{
    NativeBoundary([&] {
        if (initialized && surfaceAttached && activeWindow && component == activeComponent) {
            craft_frame(timestamp, targetTimestamp);
            ReportRustError();
            if (!firstFrameReported) {
                // This is receipt of the frame callback, not proof of GPU output.
                OH_LOG_Print(LOG_APP, LOG_INFO, CRAFT_LOG_DOMAIN, CRAFT_LOG_TAG,
                    "First frame callback: timestamp=%{public}llu target=%{public}llu",
                    static_cast<unsigned long long>(timestamp), static_cast<unsigned long long>(targetTimestamp));
                firstFrameReported = true;
            }
        }
    });
}

void DispatchTouch(OH_NativeXComponent* component, void* window) noexcept
{
    NativeBoundary([&] {
        if (!initialized || window != activeWindow || component != activeComponent) {
            return;
        }
        OH_NativeXComponent_TouchEvent event {};
        if (OH_NativeXComponent_GetTouchEvent(component, window, &event) != 0 ||
            event.type == OH_NATIVEXCOMPONENT_UNKNOWN) {
            return;
        }
        // egui receives one pointer. A second finger must not move it or release
        // the first finger's brush stroke when the second finger is lifted.
        if (event.type == OH_NATIVEXCOMPONENT_CANCEL) {
            primaryTouchActive = false;
        } else {
            if (primaryTouchActive && event.id != primaryTouchId) {
                return;
            }
            if (event.type == OH_NATIVEXCOMPONENT_DOWN) {
                primaryTouchId = event.id;
                primaryTouchActive = true;
            } else if (event.type == OH_NATIVEXCOMPONENT_UP) {
                primaryTouchActive = false;
            }
        }
        uint32_t index = 0;
        const uint32_t count = std::min(event.numPoints, uint32_t(OH_NATIVE_XCOMPONENT_MAX_TOUCH_POINTS_NUMBER));
        for (uint32_t i = 0; i < count; ++i) {
            if (event.touchPoints[i].id == event.id) {
                index = i;
                break;
            }
        }
        OH_NativeXComponent_TouchPointToolType tool = OH_NATIVEXCOMPONENT_TOOL_TYPE_UNKNOWN;
        OH_NativeXComponent_GetTouchPointToolType(component, index, &tool);
        float tiltX = 0.0f;
        float tiltY = 0.0f;
        if (tool == OH_NATIVEXCOMPONENT_TOOL_TYPE_PEN) {
            OH_NativeXComponent_GetTouchPointTiltX(component, index, &tiltX);
            OH_NativeXComponent_GetTouchPointTiltY(component, index, &tiltY);
        }
        // ArkUI's implementation forwards pixel local coordinates to native
        // XComponent callbacks. Normalize once before feeding egui logical points.
        const float x = event.x / density;
        const float y = event.y / density;
        if (std::isfinite(x) && std::isfinite(y)) {
            lastPointerX = x;
            lastPointerY = y;
        } else if (event.type != OH_NATIVEXCOMPONENT_UP && event.type != OH_NATIVEXCOMPONENT_CANCEL) {
            return;
        }
        const float pressure = tool == OH_NATIVEXCOMPONENT_TOOL_TYPE_PEN ? event.force : -1.0f;
        craft_pointer(lastPointerX, lastPointerY, static_cast<int32_t>(event.type), 1,
                      pressure, tiltX, tiltY);
    });
}

void SyncPointerModifiers(uint64_t modifiers) noexcept
{
    modifiers &= ARKUI_MODIFIER_KEY_CTRL | ARKUI_MODIFIER_KEY_SHIFT | ARKUI_MODIFIER_KEY_ALT;
    if (modifiersKnown && modifiers == lastModifiers) {
        return;
    }
    modifiersKnown = true;
    lastModifiers = modifiers;
    // KEY_UNKNOWN with pressed=true updates Rust's modifier state without a
    // physical key event. The pressed=false sentinel is reserved for blur.
    craft_key(KEY_UNKNOWN, true, (modifiers & ARKUI_MODIFIER_KEY_CTRL) != 0,
              (modifiers & ARKUI_MODIFIER_KEY_SHIFT) != 0,
              (modifiers & ARKUI_MODIFIER_KEY_ALT) != 0, "");
}

void DispatchMouse(OH_NativeXComponent* component, void* window) noexcept
{
    bool pointerLocationKnown = false;
    bool pointerInSurface = false;
    NativeBoundary([&] {
        if (!initialized || window != activeWindow || component != activeComponent) {
            return;
        }
        OH_NativeXComponent_MouseEvent event {};
        if (OH_NativeXComponent_GetMouseEvent(component, window, &event) != 0) {
            return;
        }
        int32_t action = 2;
        switch (event.action) {
            case OH_NATIVEXCOMPONENT_MOUSE_PRESS: action = 0; break;
            case OH_NATIVEXCOMPONENT_MOUSE_RELEASE: action = 1; break;
            case OH_NATIVEXCOMPONENT_MOUSE_MOVE: action = 2; break;
            case OH_NATIVEXCOMPONENT_MOUSE_CANCEL: action = 3; break;
            default: return;
        }
        OH_NativeXComponent_ExtraMouseEventInfo* extraInfo = nullptr;
        uint64_t modifiers = 0;
        if (OH_NativeXComponent_GetExtraMouseEventInfo(component, &extraInfo) == 0 && extraInfo &&
            OH_NativeXComponent_GetMouseEventModifierKeyStates(extraInfo, &modifiers) == 0) {
            SyncPointerModifiers(modifiers);
        }
        const float x = event.x / density;
        const float y = event.y / density;
        if (std::isfinite(x) && std::isfinite(y)) {
            lastPointerX = x;
            lastPointerY = y;
            uint32_t width = 0;
            uint32_t height = 0;
            if (SurfaceSize(component, window, width, height)) {
                pointerLocationKnown = true;
                pointerInSurface = event.x >= 0.0f && event.y >= 0.0f &&
                    event.x < static_cast<float>(width) && event.y < static_cast<float>(height);
            }
        } else if (action != 1 && action != 3) {
            return;
        }
        craft_pointer(lastPointerX, lastPointerY, action, static_cast<int32_t>(event.button), -1.0f, 0.0f, 0.0f);
    });
    if (pointerLocationKnown) {
        photocraft::platform::SetPointerInside(pointerInSurface);
    }
}

void DispatchHover(OH_NativeXComponent* component, bool hovering) noexcept
{
    if (component == inputComponent.load(std::memory_order_acquire)) {
        // The pointer service may call into the system; keep it outside the
        // renderer lifecycle mutex, just like IME attach/detach.
        photocraft::platform::SetPointerInside(hovering);
    }
    NativeBoundary([&] {
        if (initialized && component == activeComponent && !hovering) {
            craft_pointer(lastPointerX, lastPointerY, 4, 0, -1.0f, 0.0f, 0.0f);
        }
    });
}

void OnBlur(OH_NativeXComponent* component, void*) noexcept
{
    if (component != inputComponent.load(std::memory_order_acquire)) {
        return;
    }
    photocraft::platform::SetFocused(false);
    NativeBoundary([&] {
        if (initialized && component == activeComponent) {
            ReleaseInputs();
        }
    });
}

void OnFocus(OH_NativeXComponent* component, void*) noexcept
{
    if (component == inputComponent.load(std::memory_order_acquire)) {
        photocraft::platform::SetFocused(true);
    }
}

char KeyText(OH_NativeXComponent_KeyCode code, bool shift, bool capsLock, bool numLock) noexcept
{
    // API 26's legacy XComponent key event has no committed-text getter. This
    // covers a US physical keyboard; an IME needs a separate text-input bridge.
    if (code >= KEY_A && code <= KEY_Z) {
        return static_cast<char>((shift != capsLock ? 'A' : 'a') + (code - KEY_A));
    }
    if (code >= KEY_0 && code <= KEY_9) {
        constexpr char shiftedDigits[] = ")!@#$%^&*(";
        return shift ? shiftedDigits[code - KEY_0] : static_cast<char>('0' + (code - KEY_0));
    }
    if (numLock && code >= KEY_NUMPAD_0 && code <= KEY_NUMPAD_9) {
        return static_cast<char>('0' + (code - KEY_NUMPAD_0));
    }
    switch (code) {
        case KEY_SPACE: return ' ';
        case KEY_COMMA: return shift ? '<' : ',';
        case KEY_PERIOD: return shift ? '>' : '.';
        case KEY_GRAVE: return shift ? '~' : '`';
        case KEY_MINUS: return shift ? '_' : '-';
        case KEY_EQUALS: return shift ? '+' : '=';
        case KEY_LEFT_BRACKET: return shift ? '{' : '[';
        case KEY_RIGHT_BRACKET: return shift ? '}' : ']';
        case KEY_BACKSLASH: return shift ? '|' : '\\';
        case KEY_SEMICOLON: return shift ? ':' : ';';
        case KEY_APOSTROPHE: return shift ? '"' : '\'';
        case KEY_SLASH: return shift ? '?' : '/';
        case KEY_STAR: return '*';
        case KEY_POUND: return '#';
        case KEY_AT: return '@';
        case KEY_PLUS: return '+';
        case KEY_NUMPAD_DIVIDE: return '/';
        case KEY_NUMPAD_MULTIPLY: return '*';
        case KEY_NUMPAD_SUBTRACT: return '-';
        case KEY_NUMPAD_ADD: return '+';
        case KEY_NUMPAD_DOT: return numLock ? '.' : '\0';
        case KEY_NUMPAD_COMMA: return numLock ? ',' : '\0';
        case KEY_NUMPAD_EQUALS: return '=';
        case KEY_NUMPAD_LEFT_PAREN: return '(';
        case KEY_NUMPAD_RIGHT_PAREN: return ')';
        default: return '\0';
    }
}

bool DispatchKey(OH_NativeXComponent* component, void*) noexcept
{
    bool consumed = false;
    NativeBoundary([&] {
        if (!initialized || component != activeComponent) {
            return;
        }
        OH_NativeXComponent_KeyEvent* event = nullptr;
        OH_NativeXComponent_KeyCode code = KEY_UNKNOWN;
        OH_NativeXComponent_KeyAction action = OH_NATIVEXCOMPONENT_KEY_ACTION_UNKNOWN;
        uint64_t modifiers = 0;
        bool capsLock = false;
        bool numLock = false;
        if (OH_NativeXComponent_GetKeyEvent(component, &event) != 0 || !event ||
            OH_NativeXComponent_GetKeyEventCode(event, &code) != 0 ||
            OH_NativeXComponent_GetKeyEventAction(event, &action) != 0 ||
            OH_NativeXComponent_GetKeyEventModifierKeyStates(event, &modifiers) != 0 ||
            (action != OH_NATIVEXCOMPONENT_KEY_ACTION_DOWN && action != OH_NATIVEXCOMPONENT_KEY_ACTION_UP)) {
            return;
        }
        OH_NativeXComponent_GetKeyEventCapsLockState(event, &capsLock);
        OH_NativeXComponent_GetKeyEventNumLockState(event, &numLock);
        const bool pressed = action == OH_NATIVEXCOMPONENT_KEY_ACTION_DOWN;
        const bool ctrl = (modifiers & ARKUI_MODIFIER_KEY_CTRL) != 0;
        const bool shift = (modifiers & ARKUI_MODIFIER_KEY_SHIFT) != 0;
        const bool alt = (modifiers & ARKUI_MODIFIER_KEY_ALT) != 0;
        modifiersKnown = true;
        lastModifiers = modifiers & (ARKUI_MODIFIER_KEY_CTRL | ARKUI_MODIFIER_KEY_SHIFT | ARKUI_MODIFIER_KEY_ALT);
        // Stage dispatches keyboard events to the IME first. This callback only
        // receives the events it did not consume, so preserve printable text
        // even while attached (for example, an IME's English passthrough mode).
        const char text[2] = { pressed && !ctrl && !alt ?
            KeyText(code, shift, capsLock, numLock) : '\0', '\0' };
        const int32_t physicalCode = code == KEY_NUMPAD_ENTER ? KEY_ENTER : static_cast<int32_t>(code);
        craft_key(physicalCode, pressed, ctrl, shift, alt, text);
        consumed = true;
        photocraft::platform::LogKeyDiagnostic(pressed, text[0] != '\0');
    });
    // This is the post-IME XComponent phase; handled stops ArkUI bubbling.
    return consumed;
}

void DispatchAxis(OH_NativeXComponent* component, ArkUI_UIInputEvent* event,
                  ArkUI_UIInputEvent_Type type) noexcept
{
    bool pointerLocationKnown = false;
    bool pointerInSurface = false;
    NativeBoundary([&] {
        if (!initialized || component != activeComponent || !event || type != ARKUI_UIINPUTEVENT_TYPE_AXIS) {
            return;
        }
        const double horizontal = OH_ArkUI_AxisEvent_GetHorizontalAxisValue(event);
        const double vertical = OH_ArkUI_AxisEvent_GetVerticalAxisValue(event);
        const int32_t action = OH_ArkUI_AxisEvent_GetAxisAction(event);
        if (action == UI_AXIS_EVENT_ACTION_CANCEL || action == UI_AXIS_EVENT_ACTION_END) {
            lastPinchScale = 1.0;
            return;
        }
        if (action == UI_AXIS_EVENT_ACTION_BEGIN) {
            lastPinchScale = 1.0;
        }
        // API 12 pinchScale is relative to the gesture's initial state (1.0).
        // egui Event::Zoom expects this update's ratio. A scroll-only event has
        // scale zero and must not manufacture a zoom or reset a live baseline.
        const double cumulativeScale = OH_ArkUI_AxisEvent_GetPinchAxisScaleValue(event);
        float zoom = 1.0f;
        // ArkUI's PinchRecognizer only adopts the scale on UPDATE; END keeps
        // the last scale and may carry a default value, so never apply it again.
        if (action == UI_AXIS_EVENT_ACTION_UPDATE && std::isfinite(cumulativeScale) && cumulativeScale > 0.0) {
            const double factor = cumulativeScale / lastPinchScale;
            if (std::isfinite(factor) && factor > 0.0 && factor <= std::numeric_limits<float>::max()) {
                zoom = static_cast<float>(factor);
                lastPinchScale = cumulativeScale;
            }
        }
        const bool mouseWheel = OH_ArkUI_UIInputEvent_GetToolType(event) == UI_INPUT_EVENT_TOOL_TYPE_MOUSE;
        uint64_t modifiers = 0;
        if (OH_ArkUI_UIInputEvent_GetModifierKeyStates(event, &modifiers) == 0) {
            SyncPointerModifiers(modifiers);
        }
        // Mouse wheels report degrees (15 degrees per notch). Touchpads report
        // pixels. ArkUI AxisEvent::ConvertToOffset negates both axes to obtain
        // content movement. The wheel value already contains system scrollStep.
        const float dx = static_cast<float>(-horizontal / density);
        const float dy = static_cast<float>(mouseWheel ? -vertical * (40.0 / 15.0) : -vertical / density);
        const bool scrolling = std::isfinite(dx) && std::isfinite(dy) && (dx != 0.0f || dy != 0.0f);
        const bool pinching = std::isfinite(zoom) && zoom > 0.0f && zoom != 1.0f;
        if (scrolling || pinching) {
            // Axis events carry their own pixel location. A scroll can arrive
            // before a mouse move, so don't route it using a stale egui hover.
            const float x = OH_ArkUI_PointerEvent_GetX(event) / density;
            const float y = OH_ArkUI_PointerEvent_GetY(event) / density;
            if (std::isfinite(x) && std::isfinite(y)) {
                lastPointerX = x;
                lastPointerY = y;
                uint32_t width = 0;
                uint32_t height = 0;
                if (SurfaceSize(component, activeWindow, width, height)) {
                    pointerLocationKnown = true;
                    pointerInSurface = x >= 0.0f && y >= 0.0f &&
                        x < static_cast<float>(width) / density && y < static_cast<float>(height) / density;
                }
                craft_pointer(x, y, 2, 0, -1.0f, 0.0f, 0.0f);
            }
            if (scrolling) {
                craft_scroll(dx, dy);
            }
            if (pinching) {
                craft_zoom(zoom);
            }
        }
    });
    if (pointerLocationKnown) {
        photocraft::platform::SetPointerInside(pointerInSurface);
    }
}

OH_NativeXComponent_Callback surfaceCallbacks { OnSurfaceCreated, OnSurfaceChanged, OnSurfaceDestroyed, DispatchTouch };
OH_NativeXComponent_MouseEvent_Callback mouseCallbacks { DispatchMouse, DispatchHover };

void RegisterXComponent(napi_env env, napi_value exports)
{
    bool hasComponent = false;
    Check(napi_has_named_property(env, exports, OH_NATIVE_XCOMPONENT_OBJ, &hasComponent));
    if (!hasComponent) {
        return; // An ordinary ArkTS import does not contain an XComponent.
    }
    napi_value object = nullptr;
    Check(napi_get_named_property(env, exports, OH_NATIVE_XCOMPONENT_OBJ, &object));
    OH_NativeXComponent* component = nullptr;
    Check(napi_unwrap(env, object, reinterpret_cast<void**>(&component)));
    if (!component) {
        throw std::runtime_error("XComponent native handle is null");
    }
    inputComponent.store(component, std::memory_order_release);
    if (OH_NativeXComponent_RegisterCallback(component, &surfaceCallbacks) != 0 ||
        OH_NativeXComponent_RegisterMouseEventCallback(component, &mouseCallbacks) != 0 ||
        OH_NativeXComponent_RegisterKeyEventCallbackWithResult(component, DispatchKey) != 0 ||
        OH_NativeXComponent_RegisterFocusEventCallback(component, OnFocus) != 0 ||
        OH_NativeXComponent_RegisterBlurEventCallback(component, OnBlur) != 0 ||
        OH_NativeXComponent_RegisterUIInputEventCallback(component, DispatchAxis, ARKUI_UIINPUTEVENT_TYPE_AXIS) != 0) {
        throw std::runtime_error("Could not register PhotoCraft XComponent input callbacks");
    }
}

napi_value SetSystemLanguage(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[1] {};
        Arguments(env, info, args);
        const std::string languageTag = ReadString(env, args[0]);
        const bool accepted = craft_set_system_language(languageTag.c_str());
        ReportRustError();
        return Boolean(env, accepted);
    });
}

napi_value Initialize(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[3] {};
        Arguments(env, info, args);
        const std::string files = ReadString(env, args[0]);
        const std::string cache = ReadString(env, args[1]);
        double requestedDensity = 0.0;
        Check(napi_get_value_double(env, args[2], &requestedDensity));
        if (!std::isfinite(requestedDensity) || requestedDensity <= 0 || requestedDensity > 16) {
            throw std::runtime_error("Display density must be finite and in (0, 16]");
        }
        const float requestedNativeDensity = static_cast<float>(requestedDensity);
        const bool accepted = craft_initialize(files.c_str(), cache.c_str(), requestedNativeDensity);
        ReportRustError();
        // A rejected root change must preserve the existing editor and its scale.
        if (!accepted) {
            return Boolean(env, false);
        }
        density = requestedNativeDensity;
        initialized = true;
        if (!surfaceAttached) {
            AttachRenderer();
        }
        const char* error = craft_last_error();
        return Boolean(env, !error || *error == '\0');
    });
}

napi_value OpenDocument(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[1] {};
        Arguments(env, info, args);
        const std::string path = ReadString(env, args[0]);
        const bool opened = initialized && craft_open_document(path.c_str());
        ReportRustError();
        return Boolean(env, opened);
    });
}

napi_value SaveDocument(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[1] {};
        Arguments(env, info, args);
        const std::string path = ReadString(env, args[0]);
        const bool saved = initialized && craft_save_document(path.c_str());
        ReportRustError();
        return Boolean(env, saved);
    });
}

napi_value TakeFileRequest(napi_env env, napi_callback_info) noexcept
{
    return NapiBoundary(env, [&] {
        const char* request = initialized ? craft_take_file_request() : "";
        napi_value result = nullptr;
        Check(napi_create_string_utf8(env, request ? request : "", NAPI_AUTO_LENGTH, &result));
        return result;
    });
}

napi_value CompleteFileRequest(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[5] {};
        Arguments(env, info, args);
        const uint64_t requestedId = ReadRequestId(env, args[0]);
        const std::string result = ReadString(env, args[1]);
        if (result != "success" && result != "cancel" && result != "error") {
            throw std::runtime_error("File request result must be success, cancel, or error");
        }
        const std::string resolvedPath = ReadString(env, args[2]);
        const std::string displayName = ReadString(env, args[3]);
        const std::string error = ReadString(env, args[4]);
        const bool completed = initialized && craft_complete_file_request(requestedId,
            result.c_str(), resolvedPath.c_str(), displayName.c_str(), error.c_str());
        ReportRustError();
        return Boolean(env, completed);
    });
}

napi_value PrepareFileSave(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[2] {};
        Arguments(env, info, args);
        const uint64_t id = ReadRequestId(env, args[0]);
        const std::string destinationName = ReadString(env, args[1]);
        const char* stage = initialized ? craft_prepare_file_save(id, destinationName.c_str()) : "";
        napi_value result = nullptr;
        Check(napi_create_string_utf8(env, stage ? stage : "", NAPI_AUTO_LENGTH, &result));
        ReportRustError();
        return result;
    });
}

napi_value ReportFileError(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[1] {};
        Arguments(env, info, args);
        const std::string message = ReadString(env, args[0]);
        if (initialized) {
            craft_report_file_error(message.c_str());
            ReportRustError();
        }
        return Undefined(env);
    });
}

napi_value TakePlatformOutput(napi_env env, napi_callback_info) noexcept
{
    return PlatformNapiBoundary(env, [&] {
        const char* output = craft_take_platform_output();
        napi_value result = nullptr;
        Check(napi_create_string_utf8(env, output ? output : "{\"ime\":null,\"events\":[]}", NAPI_AUTO_LENGTH, &result));
        return result;
    });
}

napi_value ApplyPlatformOutput(napi_env env, napi_callback_info info) noexcept
{
    return PlatformNapiBoundary(env, [&] {
        napi_value args[5] {};
        Arguments(env, info, args);
        double originX = 0;
        double originY = 0;
        Check(napi_get_value_double(env, args[2], &originX));
        Check(napi_get_value_double(env, args[3], &originY));
        const uint64_t checkedId = ReadRequestId(env, args[4]);
        if (checkedId > static_cast<uint64_t>(std::numeric_limits<int32_t>::max())) {
            throw std::runtime_error("Window id exceeds the system input method limit");
        }
        int32_t windowId = 0;
        Check(napi_get_value_int32(env, args[4], &windowId));
        return Boolean(env, photocraft::platform::ApplyOutput(env, args[0], args[1], originX, originY, windowId));
    });
}

napi_value PlatformInput(napi_env env, napi_callback_info info) noexcept
{
    return PlatformNapiBoundary(env, [&] {
        napi_value args[1] {};
        Arguments(env, info, args);
        const std::string input = ReadString(env, args[0]);
        return Boolean(env, craft_platform_input(input.c_str()));
    });
}

napi_value ReleasePlatform(napi_env env, napi_callback_info) noexcept
{
    return PlatformNapiBoundary(env, [&] {
        photocraft::platform::Release();
        return Undefined(env);
    });
}

napi_value LastError(napi_env env, napi_callback_info) noexcept
{
    return NapiBoundary(env, [&] {
        const char* error = craft_last_error();
        napi_value result = nullptr;
        Check(napi_create_string_utf8(env, error ? error : "", NAPI_AUTO_LENGTH, &result));
        return result;
    });
}

napi_value Key(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[6] {};
        Arguments(env, info, args);
        int32_t code = 0;
        bool pressed = false;
        bool ctrl = false;
        bool shift = false;
        bool alt = false;
        Check(napi_get_value_int32(env, args[0], &code));
        Check(napi_get_value_bool(env, args[1], &pressed));
        Check(napi_get_value_bool(env, args[2], &ctrl));
        Check(napi_get_value_bool(env, args[3], &shift));
        Check(napi_get_value_bool(env, args[4], &alt));
        const std::string text = ReadString(env, args[5]);
        if (initialized) {
            craft_key(code, pressed, ctrl, shift, alt, text.c_str());
        }
        return Undefined(env);
    });
}

napi_value TextInput(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[1] {};
        Arguments(env, info, args);
        const std::string text = ReadString(env, args[0]);
        if (initialized) {
            craft_key(KEY_UNKNOWN, true, false, false, false, text.c_str());
        }
        return Undefined(env);
    });
}

napi_value Scroll(napi_env env, napi_callback_info info) noexcept
{
    return NapiBoundary(env, [&] {
        napi_value args[2] {};
        Arguments(env, info, args);
        double dx = 0.0;
        double dy = 0.0;
        Check(napi_get_value_double(env, args[0], &dx));
        Check(napi_get_value_double(env, args[1], &dy));
        if (!std::isfinite(dx) || !std::isfinite(dy)) {
            throw std::runtime_error("Scroll deltas must be finite");
        }
        if (initialized) {
            craft_scroll(static_cast<float>(dx), static_cast<float>(dy));
        }
        return Undefined(env);
    });
}

#ifdef PHOTOCRAFT_DEVICE_TESTS
napi_value TestSubmit(napi_env env, napi_callback_info info) noexcept
{
    return PlatformNapiBoundary(env, [&] {
        napi_value args[2] {};
        Arguments(env, info, args);
        const std::string runId = ReadString(env, args[0]);
        const std::string request = ReadString(env, args[1]);
        const char* output = craft_test_submit(runId.c_str(), request.c_str());
        napi_value result = nullptr;
        Check(napi_create_string_utf8(env, output ? output : "", NAPI_AUTO_LENGTH, &result));
        return result;
    });
}

napi_value TestPoll(napi_env env, napi_callback_info info) noexcept
{
    return PlatformNapiBoundary(env, [&] {
        napi_value args[2] {};
        Arguments(env, info, args);
        const std::string runId = ReadString(env, args[0]);
        const std::string ticket = ReadString(env, args[1]);
        const char* output = craft_test_poll(runId.c_str(), ticket.c_str());
        napi_value result = nullptr;
        Check(napi_create_string_utf8(env, output ? output : "", NAPI_AUTO_LENGTH, &result));
        return result;
    });
}

napi_value TestSnapshot(napi_env env, napi_callback_info info) noexcept
{
    return PlatformNapiBoundary(env, [&] {
        napi_value args[1] {};
        Arguments(env, info, args);
        const std::string runId = ReadString(env, args[0]);
        const char* output = craft_test_snapshot(runId.c_str());
        napi_value result = nullptr;
        Check(napi_create_string_utf8(env, output ? output : "", NAPI_AUTO_LENGTH, &result));
        return result;
    });
}
#endif

napi_value Init(napi_env env, napi_value exports) noexcept
{
    return NapiBoundary(env, [&] {
#ifdef PHOTOCRAFT_DEVICE_TESTS
        if (craft_device_tests_abi_v1() != 1) {
            throw std::runtime_error("PhotoCraft device-test Rust/native ABI does not match");
        }
#endif
        napi_property_descriptor properties[] = {
            { "initialize", nullptr, Initialize, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "setSystemLanguage", nullptr, SetSystemLanguage, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "openDocument", nullptr, OpenDocument, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "saveDocument", nullptr, SaveDocument, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "takeFileRequest", nullptr, TakeFileRequest, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "completeFileRequest", nullptr, CompleteFileRequest, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "prepareFileSave", nullptr, PrepareFileSave, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "reportFileError", nullptr, ReportFileError, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "takePlatformOutput", nullptr, TakePlatformOutput, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "applyPlatformOutput", nullptr, ApplyPlatformOutput, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "platformInput", nullptr, PlatformInput, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "releasePlatform", nullptr, ReleasePlatform, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "lastError", nullptr, LastError, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "onKey", nullptr, Key, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "textInput", nullptr, TextInput, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "onScroll", nullptr, Scroll, nullptr, nullptr, nullptr, napi_default, nullptr },
#ifdef PHOTOCRAFT_DEVICE_TESTS
            { "testSubmit", nullptr, TestSubmit, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "testPoll", nullptr, TestPoll, nullptr, nullptr, nullptr, napi_default, nullptr },
            { "testSnapshot", nullptr, TestSnapshot, nullptr, nullptr, nullptr, napi_default, nullptr },
#endif
        };
        Check(napi_define_properties(env, exports, sizeof(properties) / sizeof(properties[0]), properties));
        RegisterXComponent(env, exports);
        return exports;
    });
}

napi_module module { 1, 0, nullptr, Init, "photocraft", nullptr, { nullptr } };
} // namespace

extern "C" __attribute__((constructor)) void RegisterPhotoCraftModule() noexcept
{
    napi_module_register(&module);
}

extern "C" void craft_log(int32_t level, const char* message) noexcept
{
    // Called by Rust while a Node-API/frame callback may hold bridgeMutex and
    // synchronously wait for its render worker. Never take that mutex here.
    const LogLevel logLevel = level >= 2 ? LOG_ERROR : (level == 1 ? LOG_WARN : LOG_INFO);
    OH_LOG_Print(LOG_APP, logLevel, CRAFT_LOG_DOMAIN, CRAFT_LOG_TAG, "%{public}s", message ? message : "");
}
