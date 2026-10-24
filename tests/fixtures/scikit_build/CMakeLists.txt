cmake_minimum_required(VERSION 3.24)
project(bitcount LANGUAGES C)

find_package(Python REQUIRED COMPONENTS Interpreter Development.Module)
Python_add_library(_native MODULE native.c WITH_SOABI)
install(TARGETS _native DESTINATION bitcount)
