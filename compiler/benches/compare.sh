#!/bin/sh
# Moved: the benchmark suite lives in bench/ now. Kept so old commands and workflows keep working.
exec sh "$(dirname "$0")/../../bench/compare.sh" "$@"
