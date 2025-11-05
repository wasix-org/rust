#!/bin/bash

set -euxo pipefail

cd src/llvm-project
IS_PATCHED=1
git status | grep WebAssemblyISelLowering.cpp &> /dev/null || IS_PATCHED=0
if [ ${IS_PATCHED} -eq 0 ]; then
    git apply ../../wasix-llvm.patch
fi
cd ../..

./x.py build --stage 2

rustup toolchain uninstall wasix-dev
rustup toolchain link wasix-dev ./build/host/stage2

# toolchain uninstall prints info messages, but toolchain link doesn't, which
# makes it look like the script uninstalled the toolchain instead of installing
# it, so print an extra message to give users peace of mind
echo Successfully linked new toolchain
echo Done
