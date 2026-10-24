#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p target
cc -std=c11 -Wall -Wextra -Werror -fPIC -shared tests/xres-stub.c -o target/liblikh-test-xres.so
cc -std=c11 -Wall -Wextra -Werror -rdynamic tests/shutdown.c -o target/shutdown-test $(pkg-config --cflags --libs xft) -lX11 -lXext -lXrandr -l:libXtst.so.6 -ldl
for mode in 0 1 2 3 4 5; do
    target/shutdown-test "$PWD/target/liblikh-test-xres.so" "$mode"
done
