#!/bin/bash

set -euo pipefail

cd src/llvm-project
git status | grep WebAssemblyISelLowering.cpp &> /dev/null
if [ $? -ne 0 ]; then
    git apply ../../wasix-llvm.patch
fi
cd ../..

./x.py build --stage 2

rustup toolchain uninstall wasix-dev
rustup toolchain link wasix-dev ./build/host/stage2

echo Done
