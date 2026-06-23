#!/usr/bin/env bash
# Scripted state walk for the native notch HUD. Pipe it into the sidecar to
# watch the real notch on your screen (no Tauri needed):
#
#   ./demo.sh | .build/release/AirNoteNotch
#
# Ctrl-C to stop (closing the pipe makes the sidecar exit cleanly).
set -euo pipefail

emit() { printf '%s\n' "$1"; sleep "${2:-1}"; }

while true; do
  # listening — drive the audio bars with a stream of levels
  emit '{"type":"state","kind":"recording"}' 0.2
  for _ in $(seq 1 20); do
    emit "{\"type\":\"level\",\"value\":0.$((RANDOM % 9 + 1))}" 0.09
  done
  emit '{"type":"status","phase":"server_polish","transcript":"kal ka demo teen baje shift kar dena"}' 1.0

  # polishing → pasted
  emit '{"type":"state","kind":"processing"}' 1.2
  emit '{"type":"done"}' 2.0

  # feedback: confirm a brand
  emit '{"type":"confirm","term":"Salesforce","original":"sales force","recording_id":"r1"}' 3.0
  emit '{"type":"learned","term":"Salesforce","message":"Learned"}' 2.6

  # feedback: review corrections
  emit '{"type":"review","candidates":[{"original":"sales force","corrected":"Salesforce","tag":"brand","learnable":true},{"original":"cuber netting","corrected":"Kubernetes","tag":"term","learnable":true},{"original":"a niche","corrected":"Anish","tag":"name","learnable":true}],"recording_id":"r2"}' 4.0

  # learning toasts
  emit '{"type":"queued","term":"Kubernetes","remaining":2}' 2.4
  emit '{"type":"wrong_fixed","term":"GIF","wrong_replacement":"JIF"}' 3.0
  emit '{"type":"retraining"}' 1.5
  emit '{"type":"retrain_done","duration_s":1.2}' 2.4

  # system
  emit '{"type":"error","message":"Network timed out reaching the server","audio_id":"a1"}' 3.0
  emit '{"type":"update_ready","version":"2.4.1","message":"Restart to finish updating."}' 3.0
  emit '{"type":"output","status":"manual_paste","message":"Press Cmd-V to paste"}' 2.4

  emit '{"type":"state","kind":"idle"}' 2.5
done
