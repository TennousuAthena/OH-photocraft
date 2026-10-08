cmake_minimum_required(VERSION 3.16)
project(photocraft LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
option(PHOTOCRAFT_DEVICE_TESTS "Expose the isolated PhotoCraft device-test bridge" OFF)
if(PHOTOCRAFT_DEVICE_TESTS)
  string(TOLOWER "${CMAKE_BUILD_TYPE}" PHOTOCRAFT_NATIVE_BUILD_TYPE)
  if(NOT PHOTOCRAFT_NATIVE_BUILD_TYPE STREQUAL "debug")
    message(FATAL_ERROR "PHOTOCRAFT_DEVICE_TESTS is allowed only in a Debug native/HAP build.")
  endif()
endif()

if(NOT DEFINED OHOS_ARCH)
  set(OHOS_ARCH arm64-v8a)
endif()
set(PHOTOCRAFT_RUST_ARCHIVE "${CMAKE_CURRENT_SOURCE_DIR}/../../../libs/${OHOS_ARCH}/libphotocraft_ohos.a"
    CACHE FILEPATH "PhotoCraft Rust static library built by scripts/build-ohos.sh")
if(NOT EXISTS "${PHOTOCRAFT_RUST_ARCHIVE}")
  message(FATAL_ERROR "Missing ${PHOTOCRAFT_RUST_ARCHIVE}. Build the Rust OHOS archive before assembling the HAP.")
endif()
# Rust is built in release mode independently of HAP buildMode. Verify the
# feature-enabled archive explicitly in both directions before the native link.
find_package(Python3 COMPONENTS Interpreter REQUIRED)
set(PHOTOCRAFT_ARCHIVE_INDEX_TOOL "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../scripts/archive_symbol_index.py"
  CACHE FILEPATH "PhotoCraft archive-index feature detector")
execute_process(COMMAND "${Python3_EXECUTABLE}"
  "${PHOTOCRAFT_ARCHIVE_INDEX_TOOL}"
  "${PHOTOCRAFT_RUST_ARCHIVE}" --contains craft_device_tests_abi_v1
  OUTPUT_VARIABLE PHOTOCRAFT_TEST_ARCHIVE_STATE ERROR_VARIABLE PHOTOCRAFT_ARCHIVE_SYMBOL_ERRORS
  OUTPUT_STRIP_TRAILING_WHITESPACE
  RESULT_VARIABLE PHOTOCRAFT_ARCHIVE_SYMBOL_STATUS)
if(NOT PHOTOCRAFT_ARCHIVE_SYMBOL_STATUS EQUAL 0)
  message(FATAL_ERROR "Cannot inspect PhotoCraft Rust archive: ${PHOTOCRAFT_ARCHIVE_SYMBOL_ERRORS}")
endif()
if(PHOTOCRAFT_TEST_ARCHIVE_STATE STREQUAL "present")
  if(NOT PHOTOCRAFT_DEVICE_TESTS)
    message(FATAL_ERROR "The Rust device-tests archive cannot be used for an ordinary HAP.")
  endif()
elseif(NOT PHOTOCRAFT_TEST_ARCHIVE_STATE STREQUAL "absent")
  message(FATAL_ERROR "PhotoCraft Rust archive feature state could not be determined.")
elseif(PHOTOCRAFT_DEVICE_TESTS)
  message(FATAL_ERROR "PHOTOCRAFT_DEVICE_TESTS requires a Rust archive built with --features device-tests.")
endif()
unset(PHOTOCRAFT_TEST_ARCHIVE_STATE)

add_library(photocraft_rust STATIC IMPORTED)
set_target_properties(photocraft_rust PROPERTIES IMPORTED_LOCATION "${PHOTOCRAFT_RUST_ARCHIVE}")
add_library(photocraft SHARED native_bridge.cpp platform_bridge.cpp)
if(PHOTOCRAFT_DEVICE_TESTS)
  target_compile_definitions(photocraft PRIVATE PHOTOCRAFT_DEVICE_TESTS=1)
endif()
# DevEco's BiSheng toolchain adds --gcc-toolchain even when using its own
# compiler runtime. Ignore that SDK argument warning while keeping code strict.
target_compile_options(photocraft PRIVATE -Wall -Wextra -Werror -Wno-unused-command-line-argument)
target_link_options(photocraft PRIVATE -Wl,--no-undefined)
target_link_libraries(photocraft PRIVATE photocraft_rust
  libace_napi.z.so libace_ndk.z.so libnative_window.so libhilog_ndk.z.so libohinputmethod.so
  pthread dl m)
