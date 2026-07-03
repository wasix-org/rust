#!/bin/bash

set -euxo pipefail

cd src/llvm-project
IS_PATCHED=1
git status | grep WebAssemblyISelLowering.cpp &> /dev/null || IS_PATCHED=0
if [ ${IS_PATCHED} -eq 0 ]; then
    git apply ../../wasix-llvm.patch
fi
cd ../..

# Development-friendly defaults! In a dev environment, we can justifiably expect
# a local installation of rustc to exist already.
if [ -z "${HOST:-}" ]; then
  HOST=$(
      rustc -vV |
        grep "host: " |
        sed -e "s|host: ||"
  )
fi

# Too many backslashes? Yes there are! Out of the 8 below,
# 4 get eaten by the sed command here, and 2 more get eaten
# by the sed command that updates config.toml.wasix-template,
# leaving 2 backslashes in the final config.toml file for the
# TOML parser to then interpret correctly.
SYSROOT_EH=${SYSROOT_EH:-../wasix-libc/sysroot32-eh}
SYSROOT_EH=$(sed -e 's|\\|\\\\\\\\|g' <<< "$SYSROOT_EH")
SYSROOT_EHPIC=${SYSROOT_EHPIC:-../wasix-libc/sysroot32-ehpic}
SYSROOT_EHPIC=$(sed -e 's|\\|\\\\\\\\|g' <<< "$SYSROOT_EHPIC")

cat config.toml.wasix-template | \
  sed \
    -e "s|%HOST%|$HOST|g" \
    -e "s|%SYSROOT_EH%|$SYSROOT_EH|g" \
    -e "s|%SYSROOT_EHPIC%|$SYSROOT_EHPIC|g" \
  > config.toml

echo Final config.toml:
cat config.toml

./x.py build --stage 2

rustup toolchain uninstall wasix-dev
rustup toolchain link wasix-dev ./build/host/stage2

# toolchain uninstall prints info messages, but toolchain link doesn't, which
# makes it look like the script uninstalled the toolchain instead of installing
# it, so print an extra message to give users peace of mind
echo Successfully linked new toolchain
echo Done
