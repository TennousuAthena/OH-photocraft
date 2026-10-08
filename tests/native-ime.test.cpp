#include "ime_text.h"

#include <exception>
#include <iostream>
#include <stdexcept>
#include <string>

namespace {
using photocraft::platform::ime_text::CopyText;
using photocraft::platform::ime_text::MAX_TEXT_UNITS;
using photocraft::platform::ime_text::Utf8;

void Expect(bool condition, const char* message)
{
    if (!condition) {
        throw std::runtime_error(message);
    }
}

template <typename Action>
void ExpectInvalid(Action action)
{
    try {
        action();
    } catch (const std::runtime_error& error) {
        Expect(std::string(error.what()) == "Invalid UTF-16 text from the system input method",
            "unexpected validation error");
        return;
    }
    throw std::runtime_error("invalid UTF-16 input was accepted");
}

void ChineseAndEmoji()
{
    const char16_t units[] { 0x771F, 0x673A, 0x4E2D, 0x6587, 0xD83D, 0xDE00 };
    Expect(Utf8(CopyText(units, 6)) == u8"真机中文😀", "Chinese/emoji bytes changed");
}

void EncodingBoundaries()
{
    const char16_t units[] { 0x007F, 0x0080, 0x07FF, 0x0800, 0xFFFF };
    Expect(Utf8(CopyText(units, 5)) == std::string("\x7f\xc2\x80\xdf\xbf\xe0\xa0\x80\xef\xbf\xbf", 11),
        "UTF-8 encoding boundary changed");
}

void SupplementaryBoundaries()
{
    const char16_t units[] { 0xD800, 0xDC00, 0xDBFF, 0xDFFF };
    Expect(Utf8(CopyText(units, 4)) == std::string("\xf0\x90\x80\x80\xf4\x8f\xbf\xbf", 8),
        "supplementary Unicode boundaries changed");
}

void TrailingHighSurrogate()
{
    const char16_t units[] { u'a', 0xD83D };
    Expect(Utf8(CopyText(units, 2)) == u8"a�", "trailing high surrogate was not replaced");
}

void HighSurrogateBeforeBmp()
{
    const char16_t units[] { 0xD83D, 0x4E2D };
    Expect(Utf8(CopyText(units, 2)) == u8"�中", "invalid pair consumed the following BMP unit");
}

void IsolatedLowSurrogate()
{
    const char16_t units[] { 0xDE00, u'b' };
    Expect(Utf8(CopyText(units, 2)) == u8"�b", "isolated low surrogate was not replaced");
}

void HighSurrogateBeforeValidPair()
{
    const char16_t units[] { 0xD800, 0xD83D, 0xDE00 };
    Expect(Utf8(CopyText(units, 3)) == u8"�😀", "replacement swallowed a following valid pair");
}

void ExactLengthAndEmbeddedNull()
{
    const char16_t units[] { 0x4E2D, 0, 0xD83D, 0xDE00, u'x' };
    const auto copied = CopyText(units, 4);
    Expect(copied.size() == 4 && copied[1] == 0, "copy lost its explicit length or embedded NUL");
    Expect(Utf8(copied) == std::string("\xe4\xb8\xad\0\xf0\x9f\x98\x80", 8),
        "UTF-8 conversion lost an embedded NUL or read past the explicit length");
}

void EmptyNullInput()
{
    Expect(CopyText(nullptr, 0).empty(), "null zero-length input was rejected");
    Expect(Utf8(CopyText(nullptr, 0)).empty(), "empty text produced nonempty UTF-8");
}

void InvalidNullInput()
{
    ExpectInvalid([] { CopyText(nullptr, 1); });
}

void MaximumLengthAccepted()
{
    const std::u16string text(MAX_TEXT_UNITS, u'\u4E2D');
    const auto copied = CopyText(text.data(), text.size());
    Expect(copied == text, "maximum allowed UTF-16 text was rejected or truncated");
    const auto encoded = Utf8(copied);
    Expect(encoded.size() == MAX_TEXT_UNITS * 3 && encoded.substr(encoded.size() - 3) == u8"中",
        "maximum allowed text was not fully converted");
}

void MaximumLengthExceeded()
{
    const char16_t unit = u'x';
    // The source contains one unit: validation must run before attempting a copy.
    ExpectInvalid([&] { CopyText(&unit, MAX_TEXT_UNITS + 1); });
}

struct Test {
    const char* name;
    void (*run)();
};
}

int main()
{
    const Test tests[] {
        { "production decoder preserves Chinese and emoji", ChineseAndEmoji },
        { "production decoder encodes BMP byte boundaries", EncodingBoundaries },
        { "production decoder encodes supplementary Unicode boundaries", SupplementaryBoundaries },
        { "production decoder replaces a trailing high surrogate", TrailingHighSurrogate },
        { "production decoder preserves BMP after an invalid high surrogate", HighSurrogateBeforeBmp },
        { "production decoder replaces an isolated low surrogate", IsolatedLowSurrogate },
        { "production decoder preserves a valid pair after an invalid high surrogate", HighSurrogateBeforeValidPair },
        { "production decoder respects length and embedded NUL", ExactLengthAndEmbeddedNull },
        { "production decoder accepts null zero-length input", EmptyNullInput },
        { "production decoder rejects null nonempty input", InvalidNullInput },
        { "production decoder accepts its UTF-16 length limit", MaximumLengthAccepted },
        { "production decoder rejects input above its limit before reading", MaximumLengthExceeded },
    };
    const size_t count = sizeof(tests) / sizeof(tests[0]);
    std::cout << "TAP version 13\n1.." << count << '\n';
    bool failed = false;
    for (size_t index = 0; index < count; ++index) {
        try {
            tests[index].run();
            std::cout << "ok " << index + 1 << " - " << tests[index].name << '\n';
        } catch (const std::exception& error) {
            std::cout << "not ok " << index + 1 << " - " << tests[index].name
                      << "\n# " << error.what() << '\n';
            failed = true;
        }
    }
    return failed ? 1 : 0;
}
