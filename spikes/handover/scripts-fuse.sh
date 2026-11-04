#!/usr/bin/env bash
# Scenario 7: run the server (and B's shims) from a squashfuse mount whose
# daemon dies, then hand over to a binary on a second mount. Linux Docker
# only, with /dev/fuse and SYS_ADMIN.
set -u
bin=/target/release
for a in a b; do
  dir=/tmp/run-$a
  mkdir -p $dir/old $dir/new $dir/mnt/old $dir/mnt/new
  cd $dir
  cp $bin/handover-spike $bin/handover-shim old/
  cp $bin/handover-spike $bin/handover-shim new/
  mksquashfs old old.sqfs -quiet -noappend >/dev/null
  mksquashfs new new.sqfs -quiet -noappend >/dev/null
  squashfuse old.sqfs mnt/old && squashfuse new.sqfs mnt/new || { echo "squashfuse failed"; exit 1; }
  echo "== approach $a"
  brk="pkill -KILL -f 'squashfuse [o]ld.sqfs'; sleep 0.2; echo daemon-left: \$(pgrep -f 'squashfuse [o]ld.sqfs') >&2; sync; echo 3 > /proc/sys/vm/drop_caches && echo caches-dropped >&2; ls $dir/mnt/old >&2; umount -l $dir/mnt/old && echo lazily-unmounted >&2; grep -c $dir/mnt/old /proc/mounts >&2"
  HS_SERVER_EXE=$dir/mnt/old/handover-spike \
  HS_TARGET_EXE=$dir/mnt/new/handover-spike \
  HS_BREAK_CMD="$brk" \
    timeout 120 $bin/handover-spike driver scenario appimage --approach $a \
    > /work/spikes/handover/results/linux-$a-appimage.json 2>/tmp/err
  echo "rc=$?"
  cut -c1-400 /tmp/err
  jq -c '.metrics' /work/spikes/handover/results/linux-$a-appimage.json 2>/dev/null
  pkill -f 'squashfuse new.sqfs'; sleep 0.3; umount -l $dir/mnt/new 2>/dev/null
  echo "leftover:"; ps -eo pid,ppid,stat,args | grep -v -E "ps -eo|bash|tini|grep"
done
