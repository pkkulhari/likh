#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p target
cc -std=c11 -Wall -Wextra -Werror tests/desktop.c -o target/desktop-test $(pkg-config --cflags --libs xft) -lX11 -lXext -lXrandr -l:libXtst.so.6 -ldl
exec xvfb-run -a -s '-screen 0 1280x720x24 -nolisten tcp' target/desktop-test
