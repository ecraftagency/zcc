#!/bin/sh
# halfsuite.sh — FAST LOOP, also runs 100% INSIDE the BOX (the box is fast; the
# mac runner has been removed; the mac is only an ad-hoc clang oracle). Merely a
# thin alias: = fullsuite.sh base  (run.sh cases+ext — hand-written differential,
# fast feedback).
# Seek 1 unit:  sh tests/halfsuite.sh <case-name-substring>   (e.g. float, sizeof).
# For 1 gate/suite/all -> invoke fullsuite.sh directly (see its file header).
exec sh "$(dirname "$0")/fullsuite.sh" base "${1:-}"
