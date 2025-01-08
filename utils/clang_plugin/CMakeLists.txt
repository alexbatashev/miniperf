cmake_minimum_required(VERSION 3.20)

project(miniperf-clang-plugin LANGUAGES CXX C)

set(CMAKE_CXX_STANDARD 17)
SET(CMAKE_CXX_FLAGS "-fno-rtti -fno-exceptions")

find_package(LLVM REQUIRED CONFIG)

message(STATUS "Found LLVM ${LLVM_PACKAGE_VERSION}")
message(STATUS "Using LLVMConfig.cmake in: ${LLVM_DIR}")

add_definitions(${LLVM_DEFINITIONS})
include_directories(${LLVM_INCLUDE_DIRS})

set(LLVM_RUNTIME_OUTPUT_INTDIR ${CMAKE_BINARY_DIR}/bin)
set(LLVM_LIBRARY_OUTPUT_INTDIR ${CMAKE_BINARY_DIR}/lib)

list(APPEND CMAKE_MODULE_PATH "${LLVM_CMAKE_DIR}")

include(AddLLVM)
include(HandleLLVMOptions)

add_llvm_pass_plugin(miniperf_plugin
  pass.cpp
)
