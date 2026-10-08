#include "platform_bridge.h"
#include "photocraft_ffi.h"
#include "ime_text.h"

#include <hilog/log.h>
#include <inputmethod/inputmethod_controller_capi.h>
#include <inputmethod/inputmethod_cursor_info_capi.h>
#include <inputmethod/inputmethod_text_avoid_info_capi.h>
#include <inputmethod/inputmethod_text_config_capi.h>
#include <multimodalinput/oh_input_manager.h>
#include <dlfcn.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <exception>
#include <limits>
#include <mutex>
#include <stdexcept>
#include <string>
#include <utility>

namespace photocraft::platform {
namespace {
constexpr unsigned int CRAFT_LOG_DOMAIN = 0xD003F00;
using ime_text::CopyText;
using ime_text::Utf8;
constexpr size_t MAX_TEXT_UNITS = ime_text::MAX_TEXT_UNITS;
constexpr size_t MAX_NOTIFY_UNITS = 8192;

struct EditorSnapshot {
    std::string target;
    std::u16string text;
    int32_t start = 0;
    int32_t end = 0;
    int32_t compositionStart = -1;
    int32_t compositionEnd = -1;
    bool interrupt = false;
    int32_t windowId = 0;
    InputMethod_TextInputType inputType = IME_TEXT_INPUT_TYPE_MULTILINE;
    std::array<double, 4> cursor {};
};

// UI-thread-owned service handles. Callbacks access only editor/preview below.
InputMethod_TextEditorProxy* editorProxy = nullptr;
std::atomic<InputMethod_TextEditorProxy*> callbackProxy { nullptr };
InputMethod_InputMethodProxy* inputProxy = nullptr;
napi_env attachedEnvironment = nullptr;
int32_t attachedInstanceId = -1;
napi_ref attachedContextReference = nullptr;
std::mutex editorMutex;
EditorSnapshot editor;
std::u16string preview;
bool previewActive = false;
int32_t previewStart = -1;
int32_t previewEnd = -1;
std::atomic<bool> attached { false };
std::atomic<bool> focused { true };
std::atomic<bool> surfaceAvailable { false };
std::atomic<bool> pointerInside { false };
std::string lastServiceError;
int32_t cursorWindowId = -1;
Input_PointerStyle cursorStyle = Input_PointerStyle::DEFAULT;
bool cursorHidden = false;
bool cursorRequestKnown = false;
int32_t cursorRequestWindowId = -1;
Input_PointerStyle cursorRequestStyle = Input_PointerStyle::DEFAULT;
bool cursorRequestHidden = false;
std::string lastCursorError;

enum class DiagnosticKind : size_t {
    Config, Insert, Preview, FinishPreview, DeleteForward, DeleteBackward,
    Selection, Move, Enter, Action, KeyboardStatus, LeftText, RightText,
    CursorIndex, PrivateCommand, Attach, Detach, Stable, Key, Count
};
constexpr std::array<const char*, static_cast<size_t>(DiagnosticKind::Count)> DIAGNOSTIC_NAMES {
    "config", "insert", "preview", "finish-preview", "delete-forward", "delete-backward",
    "selection", "move", "enter", "action", "keyboard-status", "left-text", "right-text",
    "cursor-index", "private-command", "attach", "detach", "stable", "post-ime-key"
};
constexpr uint32_t DIAGNOSTIC_TOTAL_LIMIT = 64;
std::array<std::atomic<uint32_t>, static_cast<size_t>(DiagnosticKind::Count)> diagnosticCounts {};
std::atomic<uint32_t> diagnosticTotal { 0 };

enum AttachmentReason : uint32_t {
    INITIAL_ATTACHMENT = 1U, TARGET_CHANGED = 2U, WINDOW_CHANGED = 4U,
    CONTEXT_CHANGED = 8U, INTERRUPT_RISING = 16U, NO_EDITOR = 32U,
    FOCUS_LOST = 64U, SURFACE_LOST = 128U, RELEASE_REQUESTED = 256U,
    ATTACH_FAILED = 512U
};

bool ReserveDiagnosticCount(std::atomic<uint32_t>& counter, uint32_t limit) noexcept
{
    auto value = counter.load(std::memory_order_relaxed);
    while (value < limit) {
        if (counter.compare_exchange_weak(value, value + 1U, std::memory_order_relaxed)) {
            return true;
        }
    }
    return false;
}

bool PermitDiagnostic(DiagnosticKind kind) noexcept
{
    const auto index = static_cast<size_t>(kind);
    if (index >= diagnosticCounts.size()) {
        return false;
    }
    const uint32_t limit = kind == DiagnosticKind::Key ? 16U : (kind == DiagnosticKind::Stable ? 1U : 4U);
    return ReserveDiagnosticCount(diagnosticCounts[index], limit) &&
        ReserveDiagnosticCount(diagnosticTotal, DIAGNOSTIC_TOTAL_LIMIT);
}

class CallbackDiagnostic;
thread_local CallbackDiagnostic* currentCallbackDiagnostic = nullptr;

class CallbackDiagnostic {
public:
    CallbackDiagnostic(DiagnosticKind kind, InputMethod_TextEditorProxy* proxy) noexcept
        : kind_(kind), valid_(proxy && proxy == callbackProxy.load(std::memory_order_acquire)),
          previous_(currentCallbackDiagnostic)
    {
        currentCallbackDiagnostic = this;
    }

    ~CallbackDiagnostic() noexcept
    {
        currentCallbackDiagnostic = previous_;
        if (!PermitDiagnostic(kind_)) {
            return;
        }
        if (kind_ == DiagnosticKind::Config) {
            OH_LOG_Print(LOG_APP, LOG_INFO, CRAFT_LOG_DOMAIN, "PhotoCraft",
                "IME diag event=config valid=%{public}d queue=%{public}d inputType=%{public}d window=%{public}d previewSupport=%{public}d",
                static_cast<int>(valid_), queued_, inputType_, windowId_, static_cast<int>(configured_));
        } else {
            OH_LOG_Print(LOG_APP, LOG_INFO, CRAFT_LOG_DOMAIN, "PhotoCraft",
                "IME diag event=%{public}s valid=%{public}d queue=%{public}d",
                DIAGNOSTIC_NAMES[static_cast<size_t>(kind_)], static_cast<int>(valid_), queued_);
        }
    }

    void QueueResult(bool accepted) noexcept
    {
        // -1: no queue attempt; 0: any attempt rejected; 1: every attempt accepted.
        if (!accepted || queued_ != 0) {
            queued_ = accepted ? 1 : 0;
        }
    }

    void Configuration(int32_t windowId, InputMethod_TextInputType inputType) noexcept
    {
        windowId_ = windowId;
        inputType_ = static_cast<int32_t>(inputType);
        configured_ = true;
    }

private:
    DiagnosticKind kind_;
    bool valid_;
    CallbackDiagnostic* previous_;
    int32_t queued_ = -1;
    int32_t inputType_ = -1;
    int32_t windowId_ = -1;
    bool configured_ = false;
};

void LogAttachmentDiagnostic(DiagnosticKind kind, uint32_t reasons, int32_t windowId,
                             int32_t instanceId) noexcept
{
    if (PermitDiagnostic(kind)) {
        OH_LOG_Print(LOG_APP, LOG_INFO, CRAFT_LOG_DOMAIN, "PhotoCraft",
            "IME diag event=%{public}s reasons=0x%{public}x window=%{public}d instance=%{public}d",
            DIAGNOSTIC_NAMES[static_cast<size_t>(kind)], reasons, windowId, instanceId);
    }
}

struct PointerApi {
    decltype(&OH_Input_SetPointerStyle) setStyle = nullptr;
    decltype(&OH_Input_SetPointerVisible) setVisible = nullptr;

    PointerApi() noexcept
    {
        // These functions were added in API 22; the application still supports
        // API 20. Keep the service library alive for this process and resolve
        // its new functions without adding unavailable strong imports.
        void* library = dlopen("libohinput.so", RTLD_NOW | RTLD_LOCAL);
        if (library) {
            setStyle = reinterpret_cast<decltype(setStyle)>(dlsym(library, "OH_Input_SetPointerStyle"));
            setVisible = reinterpret_cast<decltype(setVisible)>(dlsym(library, "OH_Input_SetPointerVisible"));
        }
    }
};

PointerApi& CursorApi() noexcept
{
    static PointerApi api;
    return api;
}

void LogError(const char* message) noexcept
{
    OH_LOG_Print(LOG_APP, LOG_ERROR, CRAFT_LOG_DOMAIN, "PhotoCraft", "%{public}s", message);
}

void ReportCursorError(const char* operation, Input_Result result)
{
    if (result == INPUT_SUCCESS) {
        lastCursorError.clear();
        return;
    }
    const std::string error = std::string("System pointer ") + operation + " failed (" +
        std::to_string(static_cast<int32_t>(result)) + ")";
    if (error != lastCursorError) {
        LogError(error.c_str());
        lastCursorError = error;
    }
}

Input_PointerStyle CursorStyle(const std::string& icon) noexcept
{
    // Use InputKit's named SDK enum, never the egui enum's ordinal values.
    using Style = Input_PointerStyle;
    if (icon == "Help") return Style::HELP;
    if (icon == "PointingHand") return Style::HAND_POINTING;
    if (icon == "Progress") return Style::RUNNING;
    if (icon == "Wait") return Style::LOADING;
    if (icon == "Cell") return Style::CURSOR_CROSS;
    if (icon == "Crosshair") return Style::CROSS;
    if (icon == "Text") return Style::TEXT_CURSOR;
    if (icon == "VerticalText") return Style::HORIZONTAL_TEXT_CURSOR;
    if (icon == "Copy" || icon == "Alias") return Style::CURSOR_COPY;
    if (icon == "Move") return Style::MOVE;
    if (icon == "NoDrop" || icon == "NotAllowed") return Style::CURSOR_FORBID;
    if (icon == "Grab") return Style::HAND_OPEN;
    if (icon == "Grabbing") return Style::HAND_GRABBING;
    if (icon == "AllScroll") return Style::MIDDLE_BTN_NORTH_SOUTH_WEST_EAST;
    if (icon == "ResizeHorizontal") return Style::WEST_EAST;
    if (icon == "ResizeVertical") return Style::NORTH_SOUTH;
    if (icon == "ResizeNeSw") return Style::NORTH_EAST_SOUTH_WEST;
    if (icon == "ResizeNwSe") return Style::NORTH_WEST_SOUTH_EAST;
    if (icon == "ResizeEast") return Style::EAST;
    if (icon == "ResizeSouthEast") return Style::SOUTH_EAST;
    if (icon == "ResizeSouth") return Style::SOUTH;
    if (icon == "ResizeSouthWest") return Style::SOUTH_WEST;
    if (icon == "ResizeWest") return Style::WEST;
    if (icon == "ResizeNorthWest") return Style::NORTH_WEST;
    if (icon == "ResizeNorth") return Style::NORTH;
    if (icon == "ResizeNorthEast") return Style::NORTH_EAST;
    if (icon == "ResizeColumn") return Style::RESIZE_LEFT_RIGHT;
    if (icon == "ResizeRow") return Style::RESIZE_UP_DOWN;
    if (icon == "ZoomIn") return Style::ZOOM_IN;
    if (icon == "ZoomOut") return Style::ZOOM_OUT;
    // ContextMenu and future egui variants fall back to the default arrow.
    return Style::DEFAULT;
}

void ResetCursor()
{
    auto& api = CursorApi();
    if (cursorHidden && api.setVisible) {
        const auto result = api.setVisible(true);
        ReportCursorError("visibility restore", result);
        if (result == INPUT_SUCCESS) {
            cursorHidden = false;
        }
    }
    if (cursorWindowId >= 0 && cursorStyle != Input_PointerStyle::DEFAULT && api.setStyle) {
        ReportCursorError("style restore", api.setStyle(cursorWindowId, Input_PointerStyle::DEFAULT));
    }
    cursorWindowId = -1;
    cursorStyle = Input_PointerStyle::DEFAULT;
    cursorRequestKnown = false;
}

template <typename Function>
void CallbackBoundary(Function&& function) noexcept
{
    try {
        function();
    } catch (const std::exception& error) {
        LogError(error.what());
    } catch (...) {
        LogError("Unexpected exception at the IME callback boundary");
    }
}

void Check(napi_status status)
{
    if (status != napi_ok) {
        throw std::runtime_error("Invalid PhotoCraft platform output");
    }
}

napi_value Property(napi_env env, napi_value object, const char* name)
{
    napi_value value = nullptr;
    Check(napi_get_named_property(env, object, name, &value));
    return value;
}

bool IsNullish(napi_env env, napi_value value)
{
    napi_valuetype type = napi_undefined;
    Check(napi_typeof(env, value, &type));
    return type == napi_null || type == napi_undefined;
}

std::string String(napi_env env, napi_value value)
{
    size_t length = 0;
    Check(napi_get_value_string_utf8(env, value, nullptr, 0, &length));
    if (length > MAX_TEXT_UNITS * 4) {
        throw std::runtime_error("PhotoCraft platform string exceeds its limit");
    }
    std::string text(length + 1, '\0');
    Check(napi_get_value_string_utf8(env, value, text.data(), text.size(), &length));
    text.resize(length);
    return text;
}

void ApplyCursor(napi_env env, napi_value output, int32_t windowId)
{
    if (!focused.load(std::memory_order_acquire) || !surfaceAvailable.load(std::memory_order_acquire) ||
        !pointerInside.load(std::memory_order_acquire)) {
        ResetCursor();
        return;
    }
    const auto value = Property(env, output, "cursorIcon");
    const std::string icon = IsNullish(env, value) ? "Default" : String(env, value);
    const auto style = CursorStyle(icon);
    const bool hidden = icon == "None";
    if (cursorRequestKnown && cursorRequestWindowId == windowId && cursorRequestStyle == style &&
        cursorRequestHidden == hidden) {
        return;
    }
    const bool windowChanged = cursorWindowId != windowId;
    if (windowChanged) {
        ResetCursor();
    }
    cursorRequestKnown = true;
    cursorRequestWindowId = windowId;
    cursorRequestStyle = style;
    cursorRequestHidden = hidden;
    auto& api = CursorApi();
    if (!api.setStyle || !api.setVisible) {
        static bool unavailableReported = false;
        if (!unavailableReported) {
            OH_LOG_Print(LOG_APP, LOG_WARN, CRAFT_LOG_DOMAIN, "PhotoCraft",
                "System cursor styles require the API 22 InputKit service; keeping the default pointer");
            unavailableReported = true;
        }
        return;
    }
    cursorWindowId = windowId;
    if (cursorHidden != hidden) {
        const auto result = api.setVisible(!hidden);
        ReportCursorError("visibility", result);
        if (result == INPUT_SUCCESS) {
            cursorHidden = hidden;
        }
    }
    if (windowChanged || cursorStyle != style) {
        const auto result = api.setStyle(windowId, style);
        ReportCursorError("style", result);
        if (result == INPUT_SUCCESS) {
            cursorStyle = style;
            static bool firstStyleReported = false;
            if (!firstStyleReported) {
                OH_LOG_Print(LOG_APP, LOG_INFO, CRAFT_LOG_DOMAIN, "PhotoCraft",
                    "System pointer styles active: window=%{public}d style=%{public}d",
                    windowId, static_cast<int32_t>(style));
                firstStyleReported = true;
            }
        }
    }
}

std::u16string String16(napi_env env, napi_value value)
{
    size_t length = 0;
    Check(napi_get_value_string_utf16(env, value, nullptr, 0, &length));
    if (length > MAX_TEXT_UNITS) {
        throw std::runtime_error("PhotoCraft editor text exceeds its limit");
    }
    std::u16string text(length + 1, u'\0');
    Check(napi_get_value_string_utf16(env, value, text.data(), text.size(), &length));
    text.resize(length);
    return text;
}

double Number(napi_env env, napi_value value)
{
    double number = 0;
    Check(napi_get_value_double(env, value, &number));
    if (!std::isfinite(number)) {
        throw std::runtime_error("Nonfinite PhotoCraft platform geometry");
    }
    return number;
}

int32_t ContextInstanceId(napi_env env, napi_value context)
{
    // getId is public since API 22. Earlier supported releases identify the
    // context by its NAPI object reference together with the actual window ID.
    bool available = false;
    Check(napi_has_named_property(env, context, "getId", &available));
    if (!available) {
        return -1;
    }
    const auto method = Property(env, context, "getId");
    napi_valuetype type = napi_undefined;
    Check(napi_typeof(env, method, &type));
    if (type != napi_function) {
        throw std::runtime_error("Invalid ArkUI context identity method");
    }
    napi_value value = nullptr;
    Check(napi_call_function(env, context, method, 0, nullptr, &value));
    const double id = Number(env, value);
    if (id < 0 || id > std::numeric_limits<int32_t>::max() || std::floor(id) != id) {
        throw std::runtime_error("Invalid ArkUI context instance identity");
    }
    int32_t instanceId = -1;
    Check(napi_get_value_int32(env, value, &instanceId));
    return instanceId;
}

bool ContextChanged(napi_env env, napi_value context, int32_t instanceId)
{
    if (attachedEnvironment != env) {
        return true;
    }
    if (instanceId >= 0) {
        return attachedInstanceId != instanceId;
    }
    if (attachedInstanceId >= 0 || !attachedContextReference) {
        return true;
    }
    napi_value retained = nullptr;
    Check(napi_get_reference_value(env, attachedContextReference, &retained));
    bool same = false;
    Check(napi_strict_equals(env, retained, context, &same));
    return !same;
}

bool NeedsAttachment(bool hasProxy, const EditorSnapshot& previous, const EditorSnapshot& next,
                     bool contextChanged) noexcept
{
    return !hasProxy || previous.target != next.target || previous.windowId != next.windowId ||
        contextChanged || (next.interrupt && !previous.interrupt);
}

uint32_t AttachmentReasons(bool hasProxy, const EditorSnapshot& previous, const EditorSnapshot& next,
                           bool contextChanged) noexcept
{
    uint32_t reasons = hasProxy ? 0U : INITIAL_ATTACHMENT;
    if (hasProxy && previous.target != next.target) { reasons |= TARGET_CHANGED; }
    if (hasProxy && previous.windowId != next.windowId) { reasons |= WINDOW_CHANGED; }
    if (hasProxy && contextChanged) { reasons |= CONTEXT_CHANGED; }
    if (hasProxy && next.interrupt && !previous.interrupt) { reasons |= INTERRUPT_RISING; }
    return reasons;
}

void ForgetContext() noexcept
{
    const auto environment = attachedEnvironment;
    const auto reference = attachedContextReference;
    attachedEnvironment = nullptr;
    attachedInstanceId = -1;
    attachedContextReference = nullptr;
    if (environment && reference && napi_delete_reference(environment, reference) != napi_ok) {
        LogError("Cannot release the retained ArkUI context reference");
    }
}

int32_t Index(napi_env env, napi_value value, size_t textLength)
{
    const double number = Number(env, value);
    if (number < 0 || number > static_cast<double>(textLength) || std::trunc(number) != number) {
        throw std::runtime_error("PhotoCraft IME selection is outside its UTF-16 text");
    }
    int32_t index = 0;
    Check(napi_get_value_int32(env, value, &index));
    return index;
}

std::string Quote(const std::string& text)
{
    constexpr char hex[] = "0123456789abcdef";
    std::string quoted;
    quoted.reserve(text.size() + 2);
    quoted += '"';
    for (const unsigned char value : text) {
        if (value == '"' || value == '\\') {
            quoted += '\\';
            quoted += static_cast<char>(value);
        } else if (value < 0x20) {
            quoted += "\\u00";
            quoted += hex[value >> 4];
            quoted += hex[value & 15];
        } else {
            quoted += static_cast<char>(value);
        }
    }
    quoted += '"';
    return quoted;
}

bool Enqueue(const std::string& kind, const std::string& target, const std::string& fields = "")
{
    if (target.empty()) {
        return false;
    }
    const std::string input = "{\"kind\":" + Quote(kind) + ",\"target\":" + Quote(target) + fields + "}";
    const bool queued = craft_platform_input(input.c_str());
    if (currentCallbackDiagnostic) {
        currentCallbackDiagnostic->QueueResult(queued);
    }
    if (!queued) {
        LogError("PhotoCraft IME input queue rejected an event");
    }
    return queued;
}

EditorSnapshot Snapshot(InputMethod_TextEditorProxy* source = nullptr)
{
    std::lock_guard<std::mutex> lock(editorMutex);
    if (source && source != callbackProxy.load(std::memory_order_acquire)) {
        return EditorSnapshot {};
    }
    return editor;
}

void GetTextConfig(InputMethod_TextEditorProxy* proxy, InputMethod_TextConfig* config) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::Config, proxy);
    CallbackBoundary([&] {
        const auto state = Snapshot(proxy);
        if (state.target.empty()) {
            return;
        }
        diagnostic.Configuration(state.windowId, state.inputType);
        OH_TextConfig_SetInputType(config, state.inputType);
        OH_TextConfig_SetEnterKeyType(config, IME_ENTER_KEY_NEWLINE);
        OH_TextConfig_SetPreviewTextSupport(config, true);
        OH_TextConfig_SetSelection(config, state.start, state.end);
        OH_TextConfig_SetWindowId(config, state.windowId);
        // Attach constructs its TextConfig on the stack. Populate the optional
        // metadata it subsequently reads, including each string's terminator.
        OH_TextConfig_SetPlaceholder(config, u"", 1);
        OH_TextConfig_SetAbilityName(config, u"", 1);
        InputMethod_TextAvoidInfo* avoid = nullptr;
        if (OH_TextConfig_GetTextAvoidInfo(config, &avoid) == IME_ERR_OK && avoid) {
            OH_TextAvoidInfo_SetPositionY(avoid, 0.0);
            OH_TextAvoidInfo_SetHeight(avoid, 0.0);
        }
        InputMethod_CursorInfo* cursor = nullptr;
        if (OH_TextConfig_GetCursorInfo(config, &cursor) == IME_ERR_OK && cursor) {
            OH_CursorInfo_SetRect(cursor, state.cursor[0], state.cursor[1], state.cursor[2], state.cursor[3]);
        }
    });
}

void InsertText(InputMethod_TextEditorProxy* proxy, const char16_t* text, size_t length) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::Insert, proxy);
    CallbackBoundary([&] {
        const auto committed = CopyText(text, length);
        std::string target;
        {
            std::lock_guard<std::mutex> lock(editorMutex);
            if (proxy != callbackProxy.load(std::memory_order_acquire) || editor.target.empty()) {
                return;
            }
            target = editor.target;
            const int32_t begin = previewActive ? previewStart : std::min(editor.start, editor.end);
            const int32_t finish = previewActive ? previewEnd : std::max(editor.start, editor.end);
            if (begin >= 0 && finish >= begin && static_cast<size_t>(finish) <= editor.text.size()) {
                editor.text.replace(static_cast<size_t>(begin), static_cast<size_t>(finish - begin), committed);
                editor.start = begin + static_cast<int32_t>(committed.size());
                editor.end = editor.start;
                editor.compositionStart = -1;
                editor.compositionEnd = -1;
            }
            preview.clear();
            previewActive = false;
            previewStart = -1;
            previewEnd = -1;
        }
        Enqueue("imeCommit", target, ",\"text\":" + Quote(Utf8(committed)));
    });
}

int32_t SetPreview(InputMethod_TextEditorProxy* proxy, const char16_t* text, size_t length,
                   int32_t start, int32_t end) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::Preview, proxy);
    int32_t result = -1;
    CallbackBoundary([&] {
        const auto replacement = CopyText(text, length);
        std::string target;
        std::u16string composed;
        {
            std::lock_guard<std::mutex> lock(editorMutex);
            if (proxy != callbackProxy.load(std::memory_order_acquire) || editor.target.empty()) {
                return;
            }
            target = editor.target;
            if (!previewActive) {
                previewStart = std::min(editor.start, editor.end);
                previewEnd = std::max(editor.start, editor.end);
            }
            if (start == -1 && end == -1) {
                composed = replacement;
            } else if (!previewActive && start == previewStart && end == previewEnd && start == end) {
                composed = replacement;
            } else if (previewActive && start >= previewStart && end >= start && end <= previewEnd) {
                composed = preview;
                composed.replace(static_cast<size_t>(start - previewStart), static_cast<size_t>(end - start), replacement);
            } else {
                return;
            }
            if (composed.size() > MAX_TEXT_UNITS) {
                return;
            }
            preview = composed;
            previewActive = true;
            if (previewStart < 0 || previewEnd < previewStart ||
                static_cast<size_t>(previewEnd) > editor.text.size()) {
                return;
            }
            editor.text.replace(static_cast<size_t>(previewStart), static_cast<size_t>(previewEnd - previewStart), composed);
            previewEnd = previewStart + static_cast<int32_t>(composed.size());
            editor.compositionStart = previewStart;
            editor.compositionEnd = previewEnd;
            editor.start = previewEnd;
            editor.end = previewEnd;
        }
        result = Enqueue("imePreedit", target, ",\"text\":" + Quote(Utf8(composed))) ? 0 : -1;
    });
    return result;
}

void FinishPreview(InputMethod_TextEditorProxy* proxy) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::FinishPreview, proxy);
    CallbackBoundary([&] {
        std::string target;
        std::u16string composed;
        {
            std::lock_guard<std::mutex> lock(editorMutex);
            if (proxy != callbackProxy.load(std::memory_order_acquire) || !previewActive) {
                return;
            }
            target = editor.target;
            composed = preview;
            previewActive = false;
            preview.clear();
            previewStart = -1;
            previewEnd = -1;
            editor.compositionStart = -1;
            editor.compositionEnd = -1;
        }
        if (composed.empty()) {
            Enqueue("imeCancel", target);
        } else {
            Enqueue("imeCommit", target, ",\"text\":" + Quote(Utf8(composed)));
        }
    });
}

void Delete(InputMethod_TextEditorProxy* proxy, int32_t length, bool before) noexcept
{
    CallbackBoundary([&] {
        if (length <= 0 || static_cast<size_t>(length) > MAX_TEXT_UNITS) {
            return;
        }
        std::string target;
        std::u16string composed;
        bool updatePreview = false;
        bool deleteSelection = false;
        int32_t outsideBefore = 0;
        int32_t outsideAfter = 0;
        {
            std::lock_guard<std::mutex> lock(editorMutex);
            if (proxy != callbackProxy.load(std::memory_order_acquire) || editor.target.empty()) {
                return;
            }
            target = editor.target;
            const int32_t cursor = editor.end;
            if (previewActive && cursor >= previewStart && cursor <= previewEnd) {
                const bool selected = editor.start != editor.end && editor.start >= previewStart &&
                    editor.start <= previewEnd;
                int32_t begin = selected ? std::min(editor.start, cursor) :
                    (before ? std::max(previewStart, cursor - length) : cursor);
                int32_t finish = selected ? std::max(editor.start, cursor) :
                    (before ? cursor : std::min(previewEnd, cursor + length));
                // A system count may cut through a supplementary character.
                // Expand the deletion to its complete UTF-16 surrogate pair.
                if (begin > previewStart && begin < previewEnd && editor.text[begin] >= 0xDC00 &&
                    editor.text[begin] <= 0xDFFF && editor.text[begin - 1] >= 0xD800 && editor.text[begin - 1] <= 0xDBFF) {
                    --begin;
                }
                if (finish > previewStart && finish < previewEnd && editor.text[finish] >= 0xDC00 &&
                    editor.text[finish] <= 0xDFFF && editor.text[finish - 1] >= 0xD800 && editor.text[finish - 1] <= 0xDBFF) {
                    ++finish;
                }
                const int32_t removed = finish - begin;
                preview.erase(static_cast<size_t>(begin - previewStart), static_cast<size_t>(removed));
                editor.text.erase(static_cast<size_t>(begin), static_cast<size_t>(removed));
                previewEnd -= removed;
                editor.compositionStart = previewStart;
                editor.compositionEnd = previewEnd;
                editor.start = begin;
                editor.end = begin;
                composed = preview;
                updatePreview = true;
                if (!selected) {
                    outsideBefore = before ? std::max(0, length - removed) : 0;
                    outsideAfter = before ? 0 : std::max(0, length - removed);
                }
            } else if (editor.start != cursor) {
                const auto begin = std::min(editor.start, cursor);
                const auto finish = std::max(editor.start, cursor);
                editor.text.erase(static_cast<size_t>(begin), static_cast<size_t>(finish - begin));
                editor.start = begin;
                editor.end = begin;
                deleteSelection = true;
            } else {
                outsideBefore = before ? length : 0;
                outsideAfter = before ? 0 : length;
            }
        }
        if (updatePreview) {
            // egui DeleteSurrounding deliberately preserves its composition.
            // Deleting inside HarmonyOS's composing text updates the preedit.
            Enqueue("imePreedit", target, ",\"text\":" + Quote(Utf8(composed)));
        } else if (deleteSelection) {
            Enqueue("imeCommit", target, ",\"text\":\"\"");
        }
        if (outsideBefore != 0 || outsideAfter != 0) {
            Enqueue("imeDelete", target, ",\"before\":" + std::to_string(outsideBefore) +
                ",\"after\":" + std::to_string(outsideAfter));
        }
    });
}

void DeleteForward(InputMethod_TextEditorProxy* proxy, int32_t length) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::DeleteForward, proxy);
    Delete(proxy, length, true);
}

void DeleteBackward(InputMethod_TextEditorProxy* proxy, int32_t length) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::DeleteBackward, proxy);
    Delete(proxy, length, false);
}

void SetSelection(InputMethod_TextEditorProxy* proxy, int32_t start, int32_t end) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::Selection, proxy);
    CallbackBoundary([&] {
        std::string target;
        {
            std::lock_guard<std::mutex> lock(editorMutex);
            if (proxy != callbackProxy.load(std::memory_order_acquire) || start < 0 || end < 0 ||
                static_cast<size_t>(start) > editor.text.size() ||
                static_cast<size_t>(end) > editor.text.size()) {
                return;
            }
            editor.start = start;
            editor.end = end;
            target = editor.target;
        }
        Enqueue("imeSelection", target, ",\"start\":" + std::to_string(start) +
            ",\"end\":" + std::to_string(end));
    });
}

void MoveCursor(InputMethod_TextEditorProxy* proxy, InputMethod_Direction direction) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::Move, proxy);
    CallbackBoundary([&] {
        const char* movement = nullptr;
        switch (direction) {
            case IME_DIRECTION_LEFT: movement = "left"; break;
            case IME_DIRECTION_RIGHT: movement = "right"; break;
            case IME_DIRECTION_UP: movement = "up"; break;
            case IME_DIRECTION_DOWN: movement = "down"; break;
            default: return;
        }
        Enqueue("imeMove", Snapshot(proxy).target, ",\"direction\":" + Quote(movement));
    });
}

void EnterKey(InputMethod_TextEditorProxy* proxy, InputMethod_EnterKeyType) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::Enter, proxy);
    CallbackBoundary([&] { Enqueue("imeEnter", Snapshot(proxy).target); });
}

void ExtendAction(InputMethod_TextEditorProxy* proxy, InputMethod_ExtendAction action) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::Action, proxy);
    CallbackBoundary([&] {
        const char* requestedAction = nullptr;
        switch (action) {
            case IME_EXTEND_ACTION_SELECT_ALL: requestedAction = "selectAll"; break;
            case IME_EXTEND_ACTION_CUT: requestedAction = "cut"; break;
            case IME_EXTEND_ACTION_COPY: requestedAction = "copy"; break;
            case IME_EXTEND_ACTION_PASTE: requestedAction = "paste"; break;
            default: return;
        }
        Enqueue("imeAction", Snapshot(proxy).target, ",\"action\":" + Quote(requestedAction));
    });
}

void KeyboardStatus(InputMethod_TextEditorProxy* proxy, InputMethod_KeyboardStatus) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::KeyboardStatus, proxy);
}

void TextOfCursor(InputMethod_TextEditorProxy* proxy, int32_t number, char16_t* text, size_t* length, bool before) noexcept
{
    if (!length) {
        return;
    }
    const size_t capacity = *length;
    *length = 0;
    CallbackBoundary([&] {
        if (!text || number <= 0 || capacity == 0) {
            return;
        }
        const auto state = Snapshot(proxy);
        const size_t cursor = static_cast<size_t>(std::clamp(state.end, 0, static_cast<int32_t>(state.text.size())));
        const size_t available = before ? cursor : state.text.size() - cursor;
        const size_t count = std::min({ capacity, static_cast<size_t>(number), available });
        const size_t offset = before ? cursor - count : cursor;
        std::copy_n(state.text.data() + offset, count, text);
        *length = count;
    });
}

void LeftText(InputMethod_TextEditorProxy* proxy, int32_t number, char16_t* text, size_t* length) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::LeftText, proxy);
    TextOfCursor(proxy, number, text, length, true);
}

void RightText(InputMethod_TextEditorProxy* proxy, int32_t number, char16_t* text, size_t* length) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::RightText, proxy);
    TextOfCursor(proxy, number, text, length, false);
}

int32_t CursorIndex(InputMethod_TextEditorProxy* proxy) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::CursorIndex, proxy);
    int32_t index = 0;
    CallbackBoundary([&] { index = Snapshot(proxy).end; });
    return index;
}

int32_t ReceivePrivateCommand(InputMethod_TextEditorProxy* proxy, InputMethod_PrivateCommand*[], size_t) noexcept
{
    CallbackDiagnostic diagnostic(DiagnosticKind::PrivateCommand, proxy);
    // The API 12 attach validator requires this callback even when the editor
    // has no private-command protocol. Reject unsupported commands without
    // reading or retaining their callback-owned payload.
    return -1;
}

void RequireIme(InputMethod_ErrorCode code)
{
    if (code != IME_ERR_OK) {
        throw std::runtime_error("System input method operation failed (" + std::to_string(code) + ")");
    }
}

void CreateEditorProxy()
{
    if (editorProxy) {
        return;
    }
    auto* candidate = OH_TextEditorProxy_Create();
    if (!candidate) {
        throw std::runtime_error("Cannot create the system input method editor proxy");
    }
    try {
        RequireIme(OH_TextEditorProxy_SetGetTextConfigFunc(candidate, GetTextConfig));
        RequireIme(OH_TextEditorProxy_SetInsertTextFunc(candidate, InsertText));
        RequireIme(OH_TextEditorProxy_SetDeleteForwardFunc(candidate, DeleteForward));
        RequireIme(OH_TextEditorProxy_SetDeleteBackwardFunc(candidate, DeleteBackward));
        RequireIme(OH_TextEditorProxy_SetSendKeyboardStatusFunc(candidate, KeyboardStatus));
        RequireIme(OH_TextEditorProxy_SetSendEnterKeyFunc(candidate, EnterKey));
        RequireIme(OH_TextEditorProxy_SetMoveCursorFunc(candidate, MoveCursor));
        RequireIme(OH_TextEditorProxy_SetHandleSetSelectionFunc(candidate, SetSelection));
        RequireIme(OH_TextEditorProxy_SetHandleExtendActionFunc(candidate, ExtendAction));
        RequireIme(OH_TextEditorProxy_SetGetLeftTextOfCursorFunc(candidate, LeftText));
        RequireIme(OH_TextEditorProxy_SetGetRightTextOfCursorFunc(candidate, RightText));
        RequireIme(OH_TextEditorProxy_SetGetTextIndexAtCursorFunc(candidate, CursorIndex));
        RequireIme(OH_TextEditorProxy_SetReceivePrivateCommandFunc(candidate, ReceivePrivateCommand));
        RequireIme(OH_TextEditorProxy_SetSetPreviewTextFunc(candidate, SetPreview));
        RequireIme(OH_TextEditorProxy_SetFinishTextPreviewFunc(candidate, FinishPreview));
    } catch (...) {
        OH_TextEditorProxy_Destroy(candidate);
        throw;
    }
    editorProxy = candidate;
    callbackProxy.store(editorProxy, std::memory_order_release);
}

void Detach(uint32_t reasons = RELEASE_REQUESTED)
{
    if (inputProxy || editorProxy) {
        LogAttachmentDiagnostic(DiagnosticKind::Detach, reasons, Snapshot().windowId, attachedInstanceId);
    }
    // Clear callbacks' target before a service call that may synchronously ask
    // the editor for its configuration or text. No editor lock spans SDK calls.
    std::string cancelledTarget;
    callbackProxy.store(nullptr, std::memory_order_release);
    {
        std::lock_guard<std::mutex> lock(editorMutex);
        if (previewActive) {
            cancelledTarget = editor.target;
        }
        editor = EditorSnapshot {};
        preview.clear();
        previewActive = false;
        previewStart = -1;
        previewEnd = -1;
    }
    attached.store(false, std::memory_order_release);
    if (!cancelledTarget.empty()) {
        Enqueue("imeCancel", cancelledTarget);
    }
    if (inputProxy) {
        const auto result = OH_InputMethodController_Detach(inputProxy);
        if (result != IME_ERR_OK) {
            LogError("System input method detach failed");
        }
        inputProxy = nullptr;
    }
    ForgetContext();
    if (editorProxy) {
        OH_TextEditorProxy_Destroy(editorProxy);
        editorProxy = nullptr;
    }
}

napi_value ParseOutput(napi_env env, napi_value json)
{
    napi_value global = nullptr;
    Check(napi_get_global(env, &global));
    const auto jsonObject = Property(env, global, "JSON");
    const auto parse = Property(env, jsonObject, "parse");
    napi_value parsed = nullptr;
    Check(napi_call_function(env, jsonObject, parse, 1, &json, &parsed));
    return parsed;
}

EditorSnapshot ReadEditor(napi_env env, napi_value value, double originX, double originY, int32_t windowId)
{
    EditorSnapshot state;
    state.target = String(env, Property(env, value, "target"));
    if (state.target.empty() || state.target.size() > 4096) {
        throw std::runtime_error("Invalid PhotoCraft IME target");
    }
    state.text = String16(env, Property(env, value, "text"));
    state.start = Index(env, Property(env, value, "selectionStart"), state.text.size());
    state.end = Index(env, Property(env, value, "selectionEnd"), state.text.size());
    const auto compositionStart = Property(env, value, "compositionStart");
    const auto compositionEnd = Property(env, value, "compositionEnd");
    if (!IsNullish(env, compositionStart) && !IsNullish(env, compositionEnd)) {
        state.compositionStart = Index(env, compositionStart, state.text.size());
        state.compositionEnd = Index(env, compositionEnd, state.text.size());
        if (state.compositionEnd < state.compositionStart) {
            throw std::runtime_error("Invalid PhotoCraft IME composition range");
        }
        // egui selects the composing range internally. IME's document selection
        // remains a caret unless the system explicitly selects within it.
        if (std::min(state.start, state.end) == state.compositionStart &&
            std::max(state.start, state.end) == state.compositionEnd) {
            state.start = state.end;
        }
    }
    state.windowId = windowId;
    const auto interrupt = Property(env, value, "interrupt");
    if (!IsNullish(env, interrupt)) {
        Check(napi_get_value_bool(env, interrupt, &state.interrupt));
    }
    const auto purpose = String(env, Property(env, value, "purpose"));
    state.inputType = purpose == "password" ? IME_TEXT_INPUT_TYPE_VISIBLE_PASSWORD : IME_TEXT_INPUT_TYPE_MULTILINE;
    const double scale = Number(env, Property(env, value, "zoom")) * Number(env, Property(env, value, "density"));
    if (scale <= 0 || scale > 64 || !std::isfinite(originX) || !std::isfinite(originY)) {
        throw std::runtime_error("Invalid PhotoCraft IME screen geometry");
    }
    const auto rect = Property(env, value, "cursorRect");
    uint32_t length = 0;
    Check(napi_get_array_length(env, rect, &length));
    if (length != 4) {
        throw std::runtime_error("Invalid PhotoCraft IME cursor rectangle");
    }
    for (uint32_t offset = 0; offset != 4; ++offset) {
        napi_value coordinate = nullptr;
        Check(napi_get_element(env, rect, offset, &coordinate));
        state.cursor[offset] = Number(env, coordinate) * scale;
    }
    state.cursor[0] += originX;
    state.cursor[1] += originY;
    state.cursor[2] = std::max(1.0, state.cursor[2]);
    state.cursor[3] = std::max(1.0, state.cursor[3]);
    return state;
}

void NotifyEditor(const EditorSnapshot& state)
{
    // The SDK limits a selection notification to 8K UTF-16 units. Larger
    // documents remain available through the left/right text callbacks.
    if (state.text.size() <= MAX_NOTIFY_UNITS) {
        auto text = state.text;
        RequireIme(OH_InputMethodProxy_NotifySelectionChange(inputProxy, text.data(), text.size(), state.start, state.end));
    }
    auto* cursor = OH_CursorInfo_Create(state.cursor[0], state.cursor[1], state.cursor[2], state.cursor[3]);
    if (!cursor) {
        throw std::runtime_error("Cannot create the input method cursor rectangle");
    }
    const auto result = OH_InputMethodProxy_NotifyCursorUpdate(inputProxy, cursor);
    OH_CursorInfo_Destroy(cursor);
    RequireIme(result);
}
void ReleaseWithReason(uint32_t reasons) noexcept
{
    CallbackBoundary([&] { Detach(reasons); });
    CallbackBoundary([] { ResetCursor(); });
}
} // namespace

bool ApplyOutput(napi_env env, napi_value uiContext, napi_value outputJson,
                 double physicalOriginX, double physicalOriginY, int32_t windowId)
{
    const auto output = ParseOutput(env, outputJson);
    bool closeRequested = false;
    const auto events = Property(env, output, "events");
    uint32_t count = 0;
    Check(napi_get_array_length(env, events, &count));
    for (uint32_t offset = 0; offset < count; ++offset) {
        napi_value event = nullptr;
        Check(napi_get_element(env, events, offset, &event));
        closeRequested = String(env, Property(env, event, "kind")) == "close" || closeRequested;
    }
    // Cursor service errors never suppress the independent close/IME flow.
    CallbackBoundary([&] { ApplyCursor(env, output, windowId); });
    const auto ime = Property(env, output, "ime");
    if (IsNullish(env, ime) || !focused.load(std::memory_order_acquire) ||
        !surfaceAvailable.load(std::memory_order_acquire)) {
        uint32_t reasons = IsNullish(env, ime) ? NO_EDITOR : 0U;
        if (!focused.load(std::memory_order_acquire)) { reasons |= FOCUS_LOST; }
        if (!surfaceAvailable.load(std::memory_order_acquire)) { reasons |= SURFACE_LOST; }
        Detach(reasons);
    } else {
        auto state = ReadEditor(env, ime, physicalOriginX, physicalOriginY, windowId);
        const int32_t instanceId = ContextInstanceId(env, uiContext);
        const auto previous = Snapshot();
        const bool contextChanged = ContextChanged(env, uiContext, instanceId);
        const bool replace = NeedsAttachment(inputProxy != nullptr, previous, state, contextChanged);
        const uint32_t reasons = AttachmentReasons(inputProxy != nullptr, previous, state, contextChanged);
        if (replace) {
            Detach(reasons);
        }
        {
            std::lock_guard<std::mutex> lock(editorMutex);
            editor = state;
            if (previewActive && state.compositionStart >= 0) {
                previewStart = state.compositionStart;
                previewEnd = state.compositionEnd;
                preview = state.text.substr(static_cast<size_t>(previewStart),
                    static_cast<size_t>(previewEnd - previewStart));
            }
        }
        if (replace) {
            CreateEditorProxy();
            auto* options = OH_AttachOptions_Create(true);
            if (!options) {
                throw std::runtime_error("Cannot create the system input method attach options");
            }
            // API 12 Attach consumes the real window ID in GetTextConfig.
            // GetContextFromNapiValue allocates a new opaque wrapper on every
            // call and exposes no public disposer; do not create one per poll.
            const auto result = OH_InputMethodController_Attach(editorProxy, options, &inputProxy);
            OH_AttachOptions_Destroy(options);
            if (result != IME_ERR_OK) {
                Detach(ATTACH_FAILED);
                const std::string error = "System input method attach failed (" + std::to_string(result) + ")";
                if (error != lastServiceError) {
                    LogError(error.c_str());
                    lastServiceError = error;
                }
                return closeRequested;
            }
            attachedEnvironment = env;
            attachedInstanceId = instanceId;
            if (instanceId < 0) {
                try {
                    Check(napi_create_reference(env, uiContext, 1, &attachedContextReference));
                } catch (...) {
                    Detach(ATTACH_FAILED);
                    throw;
                }
            }
            attached.store(true, std::memory_order_release);
            lastServiceError.clear();
            LogAttachmentDiagnostic(DiagnosticKind::Attach, reasons, windowId, instanceId);
        } else {
            // One process-level confirmation proves stable polls retained the
            // existing service attachment without logging every frame.
            LogAttachmentDiagnostic(DiagnosticKind::Stable, 0U, windowId, instanceId);
        }
        if (!replace && previous.inputType != state.inputType) {
            RequireIme(OH_InputMethodProxy_NotifyConfigurationChange(inputProxy, IME_ENTER_KEY_NEWLINE, state.inputType));
        }
        NotifyEditor(state);
    }
    return closeRequested;
}

void Release() noexcept
{
    ReleaseWithReason(RELEASE_REQUESTED);
}

void SetFocused(bool value) noexcept
{
    focused.store(value, std::memory_order_release);
    if (!value) {
        pointerInside.store(false, std::memory_order_release);
        ReleaseWithReason(FOCUS_LOST);
    }
}

void SetSurfaceAvailable(bool value) noexcept
{
    surfaceAvailable.store(value, std::memory_order_release);
    if (!value) {
        pointerInside.store(false, std::memory_order_release);
        ReleaseWithReason(SURFACE_LOST);
    }
}

void SetPointerInside(bool value) noexcept
{
    pointerInside.store(value, std::memory_order_release);
    if (!value) {
        CallbackBoundary([] { ResetCursor(); });
    }
}

bool ImeAttached() noexcept
{
    return attached.load(std::memory_order_acquire);
}

void LogKeyDiagnostic(bool pressed, bool hasTextFallback) noexcept
{
    if (PermitDiagnostic(DiagnosticKind::Key)) {
        OH_LOG_Print(LOG_APP, LOG_INFO, CRAFT_LOG_DOMAIN, "PhotoCraft",
            "IME diag event=post-ime-key action=%{public}s attached=%{public}d fallback=%{public}d",
            pressed ? "down" : "up", static_cast<int>(ImeAttached()), static_cast<int>(hasTextFallback));
    }
}
} // namespace photocraft::platform
