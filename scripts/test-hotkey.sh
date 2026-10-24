#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p target
cc -std=c11 -Wall -Wextra -Werror tests/hotkey.c -o target/hotkey-test $(pkg-config --cflags --libs xft) -lX11 -lXext -lXrandr -l:libXtst.so.6 -ldl
exec target/hotkey-test
