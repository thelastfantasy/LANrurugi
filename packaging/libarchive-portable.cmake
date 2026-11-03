# CMake toolchain file for release builds (CMAKE_TOOLCHAIN_FILE): keeps the bundled libarchive
# (libarchive2-sys) from linking optional system libraries a user's machine may lack. The library
# only reads archives, so XAR (libxml2/expat) and ACL support are unused; on macOS, CommonCrypto
# replaces OpenSSL and libarchive's own BLAKE2 replaces Homebrew's libb2. The libraries
# libarchive2-sys names at link time must still be installed on the build machine.
set(ENABLE_LIBXML2 OFF CACHE BOOL "" FORCE)
set(ENABLE_EXPAT OFF CACHE BOOL "" FORCE)
set(ENABLE_ACL OFF CACHE BOOL "" FORCE)
if(APPLE)
  set(ENABLE_OPENSSL OFF CACHE BOOL "" FORCE)
  set(ENABLE_LIBB2 OFF CACHE BOOL "" FORCE)
endif()
