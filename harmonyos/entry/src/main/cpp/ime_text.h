#pragma once

#include <cstddef>
#include <cstdint>
#include <stdexcept>
#include <string>

namespace photocraft::platform::ime_text {
inline constexpr size_t MAX_TEXT_UNITS = 1024 * 1024;

inline std::u16string CopyText(const char16_t* text, size_t length)
{
    if ((!text && length != 0) || length > MAX_TEXT_UNITS) {
        throw std::runtime_error("Invalid UTF-16 text from the system input method");
    }
    return length == 0 ? std::u16string() : std::u16string(text, length);
}

inline std::string Utf8(const std::u16string& text)
{
    std::string result;
    result.reserve(text.size() * 3);
    for (size_t offset = 0; offset < text.size(); ++offset) {
        uint32_t code = text[offset];
        if (code >= 0xD800 && code <= 0xDBFF && offset + 1 < text.size() &&
            text[offset + 1] >= 0xDC00 && text[offset + 1] <= 0xDFFF) {
            code = 0x10000 + ((code - 0xD800) << 10) + (text[++offset] - 0xDC00);
        } else if (code >= 0xD800 && code <= 0xDFFF) {
            code = 0xFFFD;
        }
        if (code <= 0x7F) {
            result += static_cast<char>(code);
        } else if (code <= 0x7FF) {
            result += static_cast<char>(0xC0 | (code >> 6));
            result += static_cast<char>(0x80 | (code & 0x3F));
        } else if (code <= 0xFFFF) {
            result += static_cast<char>(0xE0 | (code >> 12));
            result += static_cast<char>(0x80 | ((code >> 6) & 0x3F));
            result += static_cast<char>(0x80 | (code & 0x3F));
        } else {
            result += static_cast<char>(0xF0 | (code >> 18));
            result += static_cast<char>(0x80 | ((code >> 12) & 0x3F));
            result += static_cast<char>(0x80 | ((code >> 6) & 0x3F));
            result += static_cast<char>(0x80 | (code & 0x3F));
        }
    }
    return result;
}
}
