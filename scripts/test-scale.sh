#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p target
cc -std=c11 -Wall -Wextra -Werror tests/scale.c -o target/scale-test
exec target/scale-test
