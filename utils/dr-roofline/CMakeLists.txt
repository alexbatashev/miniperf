cmake_minimum_required(VERSION 3.14)
project(miniperf-dr-roofline C)

# Path to the roofline-core Rust staticlib, e.g.
# <repo>/target/release/libroofline_core.a (build with
# `cargo build --release -p miniperf-roofline-core`).
set(ROOFLINE_CORE_LIB "" CACHE FILEPATH "Path to libroofline_core.a")
if(ROOFLINE_CORE_LIB STREQUAL "")
    get_filename_component(REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
    set(ROOFLINE_CORE_LIB "${REPO_ROOT}/target/release/libroofline_core.a")
endif()
if(NOT EXISTS "${ROOFLINE_CORE_LIB}")
    message(FATAL_ERROR
        "libroofline_core.a not found at ${ROOFLINE_CORE_LIB}; build it with "
        "'cargo build --release -p miniperf-roofline-core' or pass -DROOFLINE_CORE_LIB=")
endif()

find_package(DynamoRIO REQUIRED)

add_library(dr_roofline SHARED client.c)
configure_DynamoRIO_client(dr_roofline)
use_DynamoRIO_extension(dr_roofline drmgr)
use_DynamoRIO_extension(dr_roofline drutil)
use_DynamoRIO_extension(dr_roofline drreg)
target_link_libraries(dr_roofline ${ROOFLINE_CORE_LIB} pthread dl m)
