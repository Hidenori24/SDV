cmake_minimum_required(VERSION 3.20)
project(sdv_autosar_like_cpp LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

add_executable(sdv_sim
  src/app/main.cpp
  src/rte/rte.cpp
  src/bsw/timebase.cpp
  src/bsw/logging.cpp
  src/bsw/diag.cpp
  src/swc/engine_swc.cpp
  src/swc/brake_swc.cpp
  src/swc/steering_swc.cpp
  src/swc/vehicledynamics_swc.cpp
  src/swc/driverinput_swc.cpp
  src/swc/safety_swc.cpp
)

target_include_directories(sdv_sim PRIVATE
  src
)

if (MSVC)
  target_compile_options(sdv_sim PRIVATE /W4)
else()
  target_compile_options(sdv_sim PRIVATE -Wall -Wextra -Wpedantic)
endif()
