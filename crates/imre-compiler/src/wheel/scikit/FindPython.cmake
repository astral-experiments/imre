# A host interpreter can run build tools. Only the development target uses Imre.
if(NOT Python_FIND_COMPONENTS OR Python_FIND_COMPONENTS STREQUAL "Interpreter")
  include("${CMAKE_ROOT}/Modules/FindPython.cmake")
  return()
endif()

string(JSON Python_VERSION GET "${_imre_target}" python_version)
set(Python_INCLUDE_DIRS "${_imre_include}")
if(NOT EXISTS "${Python_INCLUDE_DIRS}/Python.h")
  message(FATAL_ERROR "Missing Imre Python.h in ${Python_INCLUDE_DIRS}")
endif()
set(Python_Development.Module_FOUND TRUE)

if("Interpreter" IN_LIST Python_FIND_COMPONENTS)
  # scikit-build-core passes its isolated build host as Python_EXECUTABLE.
  find_program(Python_EXECUTABLE NAMES python3)
  if(EXISTS "${Python_EXECUTABLE}")
    set(Python_Interpreter_FOUND TRUE)
    if(NOT TARGET Python::Interpreter)
      add_executable(Python::Interpreter IMPORTED)
      set_target_properties(Python::Interpreter PROPERTIES IMPORTED_LOCATION "${Python_EXECUTABLE}")
    endif()
  endif()
endif()

include(FindPackageHandleStandardArgs)
find_package_handle_standard_args(Python
  REQUIRED_VARS Python_INCLUDE_DIRS VERSION_VAR Python_VERSION HANDLE_VERSION_RANGE HANDLE_COMPONENTS)
if(Python_FOUND AND NOT TARGET Python::Module)
  add_library(Python::Module INTERFACE IMPORTED)
  set_target_properties(Python::Module PROPERTIES INTERFACE_INCLUDE_DIRECTORIES "${Python_INCLUDE_DIRS}")
endif()

function(Python_add_library name)
  cmake_parse_arguments(PARSE_ARGV 1 module "MODULE;STATIC;SHARED;WITH_SOABI" "USE_SABI" "")
  if(module_STATIC OR module_SHARED OR DEFINED module_USE_SABI OR module_KEYWORDS_MISSING_VALUES)
    message(FATAL_ERROR "Imre's Python_add_library supports MODULE extensions with the Imre ABI")
  endif()
  add_library("${name}" STATIC ${module_UNPARSED_ARGUMENTS})
  target_link_libraries("${name}" PRIVATE Python::Module)
  set_target_properties("${name}" PROPERTIES
    PREFIX "" SUFFIX "${_imre_suffix}" POSITION_INDEPENDENT_CODE ON NO_SYSTEM_FROM_IMPORTED ON)
  set_property(GLOBAL APPEND PROPERTY IMRE_EXTENSION_TARGETS "${name}")
endfunction()
