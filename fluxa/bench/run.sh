#!/bin/sh
# AI 撰寫實驗：./run.sh fluxa|veryl KERNEL [MODEL]
# 便宜的模型讀規格與文件，自己反覆跑 reda 直到通過；每次 reda 呼叫記在 KERNEL/LANG/reda.log
# SIMONLY=1：只要求模擬通過；MAXSTEPS=N：agent 的步數上限（預設 40）
# EXTRA="檔案 ..."：多給的參考資料；TAG=名稱：工作目錄改成 KERNEL/LANG-名稱
set -u
cd "$(dirname "$0")"
LANG_=$1; K=$2; MODEL=${3:-minimax,glm}
ROOT=$(cd ../.. && pwd)
export PATH="$(pwd)/bin:$PATH"
# 每個組合一個獨立的工作目錄：兩種語言的版本互相看不到
W="$K/$LANG_${TAG:+-$TAG}"
mkdir -p "$W" && cp "$K/spec.md" "$W/"
export REDA_LOG="$(pwd)/$W/reda.log"
: > "$REDA_LOG"
DOCS="$ROOT/docs/manual/tw"
if [ "$LANG_" = fluxa ]; then
  FILES="-f $DOCS/17-fluxa.md -f $ROOT/fluxa/docs/synthesis-plan.md -f $ROOT/fluxa/examples/fir4.fx -f $ROOT/fluxa/examples/mac.fx"
  if [ "${SIMONLY:-}" = 1 ]; then
  TASK="Write $K.fx in the current directory: the task described in spec.md and a test block checking the test vectors.
Iterate until it passes:  reda sim --format json $K.fx   (simulation only; no synthesis needed).
Use only the reda command to check your work."
  else
  TASK="Write $K.fx in the current directory: the task described in spec.md and a test block checking the test vectors.
Iterate until both pass:  reda sim --format json $K.fx   and   reda synth --format json --top <TaskName> $K.fx
(also run  reda check --format json $K.fx  and fix what it reports where it makes sense).
Use only the reda command to check your work."
  fi
else
  FILES="-f $DOCS/04-veryl.md -f $DOCS/05-sim.md -f $ROOT/veryl/examples/src/fifo.veryl -f $ROOT/veryl/examples/src/seq101.veryl"
  TASK="Write $K.veryl in the current directory: the module described in spec.md and a #[test] testbench module checking the test vectors.
Iterate until both pass:  reda sim $K.veryl   and   reda synth --top <ModuleName> $K.veryl
Use only the reda command to check your work."
fi
for f in ${EXTRA:-}; do FILES="$FILES -f $f"; done
START=$(date +%s)
agge ask -y -q --brief 3 --max-steps "${MAXSTEPS:-40}" -m "$MODEL" -o "$W" -f common.md -f "$W/spec.md" $FILES "$TASK
When both pass, answer with one line: DONE. If you give up, answer: GAVE UP and why." > "$W/answer" 2>&1
END=$(date +%s)
echo "$LANG_ $K: $(( END - START ))s, $(wc -l < "$REDA_LOG" | tr -d ' ') reda runs, answer: $(tail -1 "$W/answer")"
