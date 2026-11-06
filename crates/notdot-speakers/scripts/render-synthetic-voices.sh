#!/usr/bin/env bash
# Render the synthetic speaker evidence set with macOS system voices.
# Writes WAV files only; nothing is played aloud. No real person is recorded.
# Usage: render-synthetic-voices.sh <output-dir>
set -euo pipefail
F="${1:?output directory}"
enrol=("notdot, this is my voice, and I'm happy for you to recognise it." "The quick brown fox jumps over the lazy dog." "Could you remind me to water the plants tomorrow morning?" "What's the weather looking like this weekend?")
tests=("Hey notdot, what's on my calendar today?" "Please add milk and eggs to the shopping list." "How long will it take to drive to the station?" "Tell me a short story about a lighthouse keeper." "Set a timer for twelve minutes, please." "Can you check whether the build has finished yet?" "Remind me to call my sister on Sunday afternoon." "What did we talk about yesterday evening?")
voices=("Daniel" "Karen" "Moira" "Rishi" "Samantha (English (US))" "Tessa" "Eddy (English (UK))" "Flo (English (UK))" "Aman" "Fred" "Ralph" "Kathy" "Reed (English (UK))" "Grandpa (English (UK))" "Grandma (English (UK))" "Albert")
for v in "${voices[@]}"; do
  slug=$(echo "$v" | cut -d' ' -f1 | tr 'A-Z' 'a-z')
  mkdir -p "$F/$slug"
  i=0; for t in "${enrol[@]}"; do say -v "$v" -o "$F/$slug/enrol-$i.wav" --file-format=WAVE --data-format=LEI16@16000 "$t"; i=$((i+1)); done
  i=0; for t in "${tests[@]}"; do say -v "$v" -o "$F/$slug/test-$i.wav" --file-format=WAVE --data-format=LEI16@16000 "$t"; i=$((i+1)); done
done
