# W51: the JavaScript engine in ring 3 (`run jsrun`): the built-in self-test with its benchmark, a script
# file written in the Terminal, an uncaught exception (message, stack, status 1), a script from a pipe
# with top-level await, `-e` with a BigInt, a missing file, and a script that eats memory until the
# engine's limit (out of memory is an exception; the next run works). The verdict is written to
# $OUT/verdict.txt from what the process printed (each output line is also in serial.log as
# `[jsrun:PID] ...`). Run with QEMU_MEM=256M: at the default 128M the processes share ~20 MiB of frames,
# less than the self-test's object benchmark wants.
source "$(dirname "$0")/../lib.sh"
wait_for() { # wait_for PATTERN SECONDS: until serial.log has the pattern
  local i
  for ((i = 0; i < $2 * 2; i++)); do grep -aq -- "$1" "$OUT/serial.log" && return 0; sleep 0.5; done
  return 1
}
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
typestr "run jsrun --selftest"; key ret
wait_for "bench 100k objects" 240
sleep 2
shot a-selftest
typestr "echo 'const xs = [3, 1, 2].map((x) => x * 2); console.log(\"arquivo:\", xs, xs.length)' > t.js"; key ret; sleep 1
typestr "run jsrun t.js"; key ret
wait_for "arquivo:" 60
typestr "echo 'function f() { return null.x } f()' > bad.js"; key ret; sleep 1
typestr "run jsrun bad.js"; key ret
wait_for "Uncaught TypeError" 60
sleep 2
typestr "echo 'console.log(\"pipe:\", await Promise.resolve(Date.now() > 0))' | run jsrun"; key ret
wait_for "pipe: true" 60
typestr "run jsrun -e 'console.log(\"big:\", 2n ** 64n)'"; key ret
wait_for "big: 18446744073709551616" 60
typestr "run jsrun nada.js"; key ret
wait_for "nada.js:" 30
sleep 3
shot b-scripts
typestr "run jsrun -e 'const a = []; for (;;) a.push(new Array(100000).fill(1))'"; key ret
wait_for "out of memory" 120
sleep 2
typestr "run jsrun -e 'console.log(\"vivo\")'"; key ret
wait_for "\] vivo" 60
sleep 3
shot c-oom

{
  grep -a "\[jsrun:" "$OUT/serial.log" | tr -d '\r' | sed 's/^.*\[jsrun:[0-9]*\] //' > "$OUT/jsrun-out.txt"
  ok=1
  need() { if grep -aq -- "$1" "$OUT/jsrun-out.txt"; then echo "PASS $2"; else echo "MISS $2"; ok=0; fi; }
  need "^selftest: [0-9]*/[0-9]* ok$" "selftest all ok"
  need "^ok   recursion: RangeError, process alive" "deep recursion is a RangeError"
  need "^bench fib(27) = 196418 in" "benchmark fib(27)"
  need "^bench loop 5e6 sums = 12499997500000 in" "benchmark loop"
  need "^arquivo: \[6,2,4\] 3" "script file"
  need "^Uncaught TypeError: cannot read property 'x' of null" "uncaught exception"
  need "at f (bad.js" "stack trace"
  need "^pipe: true" "stdin + top-level await"
  need "^big: 18446744073709551616" "-e with BigInt"
  need "jsrun: nada.js: no such file" "missing file"
  need "^Uncaught InternalError: out of memory" "memory limit is an exception"
  need "^vivo$" "next run after out of memory"
  if grep -aq "FAIL " "$OUT/jsrun-out.txt"; then echo "MISS no FAIL lines"; ok=0; fi
  grep -a "^bench\|^info" "$OUT/jsrun-out.txt"
  [ $ok = 1 ] && echo "VERDICT: PASS" || echo "VERDICT: FAIL"
} > "$OUT/verdict.txt" 2>&1
finish
