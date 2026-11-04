#!/usr/bin/env bash
# Runs every #394 scenario and measurement for both approaches and writes
# JSON results to results/<platform>-*. Usage: run-all.sh <bin-dir> [huterm-exe]
set -u
bin=$1
huterm=${2:-}
here=$(cd "$(dirname "$0")" && pwd)
os=$(uname -s | tr A-Z a-z)
out=$here/results
mkdir -p "$out"
spike=$bin/handover-spike
for a in a b; do
  for s in jobcontrol tui heavy exit close failure; do
    timeout 300 "$spike" driver scenario $s --approach $a > "$out/$os-$a-$s.json" 2> "$out/$os-$a-$s.log"
    echo "$a $s rc=$?"
  done
  for i in 1 2 3 4 5; do
    timeout 300 "$spike" driver scenario heavy --approach $a > "$out/$os-$a-heavy-$i.json" 2>/dev/null
    echo "$a heavy#$i rc=$?"
  done
done
: > "$out/$os-bench.jsonl"
for a in a b; do
  "$spike" driver bench --approach $a --terminals 1,16,128 --repeat 20 >> "$out/$os-bench.jsonl"
done
"$spike" driver bench --approach a --terminals 1,16,128 --repeat 20 --precheck >> "$out/$os-bench.jsonl"
shim=$bin/../shim/handover-shim
[ -x "$shim" ] || shim=$bin/handover-shim
if [ -n "$huterm" ]; then
  "$spike" driver rss --count 16 --shim-exe "$shim" --huterm-exe "$huterm" > "$out/$os-rss.json"
else
  "$spike" driver rss --count 16 --shim-exe "$shim" > "$out/$os-rss.json"
fi
echo "rss rc=$?"
