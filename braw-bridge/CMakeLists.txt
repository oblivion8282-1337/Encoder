cmake_minimum_required(VERSION 3.16)
project(braw-bridge LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_executable(braw-bridge
    src/main.cpp
    sdk/Include/BlackmagicRawAPIDispatch.cpp
)

target_include_directories(braw-bridge PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/sdk/Include
)

target_link_libraries(braw-bridge PRIVATE
    pthread
    dl
)

# RPATH so the dynamic linker finds the SDK .so files at runtime
set_target_properties(braw-bridge PROPERTIES
    BUILD_RPATH   "${CMAKE_CURRENT_SOURCE_DIR}/sdk/Libraries/Linux"
    INSTALL_RPATH "$ORIGIN/../sdk/Libraries/Linux"
)

install(TARGETS braw-bridge RUNTIME DESTINATION bin)
