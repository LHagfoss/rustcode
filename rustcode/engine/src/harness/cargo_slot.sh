#!/bin/sh
# RustCode build slot: `cargo` for a delegate working in a worktree.
#
# Parallel delegates compiling at once block each other on cargo's locks and
# drain the laptop. This wrapper lets at most RUSTCODE_BUILD_SLOTS builds run
# at the same time, whatever thread or worktree they come from, and gives each
# slot a target directory of its own for the project
# ($RUSTCODE_BUILD_ROOT/slot-N). Two builds never share a build-directory lock,
# and a new worktree reuses a warm target instead of compiling from scratch.
# A delegate that waits prints why, so the wait reads as a queue, not a hang.
# Anything that does not compile (fmt, metadata, --version) runs untouched.

shim_dir=${RUSTCODE_BUILD_SHIM_DIR:-$(dirname "$0")}
clean=
old_ifs=$IFS
IFS=:
for dir in $PATH; do
  if [ -n "$dir" ] && [ "$dir" != "$shim_dir" ]; then
    clean="${clean:+$clean:}$dir"
  fi
done
IFS=$old_ifs
real=$(PATH=$clean command -v cargo) || {
  echo "cargo: not found on PATH" >&2
  exit 127
}

# Not in a delegate worktree, or already inside a build that holds a slot
# (cargo calling cargo): nothing to queue for.
if [ -z "$RUSTCODE_BUILD_ROOT" ] || [ -n "$RUSTCODE_BUILD_SLOT_HELD" ]; then
  PATH=$clean exec "$real" "$@"
fi

sub=
for arg in "$@"; do
  case "$arg" in
    +* | -*) ;;
    *)
      sub=$arg
      break
      ;;
  esac
done
case "$sub" in
  build | b | check | c | test | t | clippy | run | r | bench | doc | d | fix | nextest | llvm-cov | miri | rustc | install) ;;
  *) PATH=$clean exec "$real" "$@" ;;
esac

locks=${RUSTCODE_BUILD_LOCKS:-$RUSTCODE_BUILD_ROOT/locks}
max=${RUSTCODE_BUILD_SLOTS:-2}
mkdir -p "$locks" 2>/dev/null || {
  PATH=$clean exec "$real" "$@"
}

got=
lock=
since=$(date +%s)
told=0
while [ -z "$got" ]; do
  n=1
  while [ "$n" -le "$max" ]; do
    lock="$locks/slot-$n.lock"
    if mkdir "$lock" 2>/dev/null; then
      echo "$$" >"$lock/pid"
      got=$n
      break
    fi
    holder=$(cat "$lock/pid" 2>/dev/null)
    if [ -n "$holder" ]; then
      if ! kill -0 "$holder" 2>/dev/null; then
        # The build that held it died without releasing it.
        rm -rf "$lock"
        continue
      fi
    elif [ -n "$(find "$lock" -maxdepth 0 -mmin +1 2>/dev/null)" ]; then
      rm -rf "$lock"
      continue
    fi
    n=$((n + 1))
  done
  if [ -z "$got" ]; then
    now=$(date +%s)
    if [ "$told" -eq 0 ] || [ $((now - told)) -ge 30 ]; then
      echo "cargo: waiting for a build slot ($max builds already run for RustCode delegates; waited $((now - since))s)" >&2
      told=$now
    fi
    sleep 5
  fi
done

release() {
  rm -rf "$lock"
}
trap release EXIT
trap 'exit 143' HUP TERM
trap 'exit 130' INT

if [ -n "$told" ] && [ "$told" -ne 0 ]; then
  echo "cargo: got build slot $got after $(($(date +%s) - since))s" >&2
fi

# The default RustCode set, or none, becomes this slot's own target; a target
# the user chose stays theirs.
if [ -z "$CARGO_TARGET_DIR" ] || [ "$CARGO_TARGET_DIR" = "$RUSTCODE_BUILD_ROOT/slot-1" ]; then
  CARGO_TARGET_DIR="$RUSTCODE_BUILD_ROOT/slot-$got"
  export CARGO_TARGET_DIR
fi
RUSTCODE_BUILD_SLOT_HELD=$got
export RUSTCODE_BUILD_SLOT_HELD
PATH=$clean
export PATH
"$real" "$@"
exit $?
