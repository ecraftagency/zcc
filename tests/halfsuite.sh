#!/bin/sh
# halfsuite.sh — VÒNG NHANH, cũng chạy 100% TRONG BOX (Vu 2026-08-20: box nhanh,
# runner mac đã bỏ; mac chỉ để clang làm oracle ad-hoc). Chỉ là alias mỏng:
# = fullsuite.sh base  (run.sh cases+ext — differential viết tay, feedback nhanh).
# Seek 1 unit:  sh tests/halfsuite.sh <chuỗi-tên-case>   (vd: float, sizeof).
# Muốn 1 gate/suite/tất cả -> gọi thẳng fullsuite.sh (xem đầu file đó).
exec sh "$(dirname "$0")/fullsuite.sh" base "${1:-}"
