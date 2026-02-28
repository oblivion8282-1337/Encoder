cmake_minimum_required(VERSION 3.16)
project(r3d-bridge LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

set(SDK_DIR ${CMAKE_CURRENT_SOURCE_DIR}/R3DSDKv9_1_2)

add_executable(r3d-bridge src/main.cpp)

target_include_directories(r3d-bridge PRIVATE
    ${SDK_DIR}/Include
)

target_link_libraries(r3d-bridge PRIVATE
    ${SDK_DIR}/Lib/linux64/libR3DSDKPIC.a
    dl
    pthread
)

target_compile_options(r3d-bridge PRIVATE -O2)

install(TARGETS r3d-bridge RUNTIME DESTINATION bin)
