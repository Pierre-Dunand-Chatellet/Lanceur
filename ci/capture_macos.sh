#!/bin/sh
# Ouvre Lanceur.app, vérifie qu'il tourne, garde une capture de l'écran. usage : capture_macos.sh <sortie.png>
Lanceur.app/Contents/MacOS/lanceur > "$RUNNER_TEMP/lanceur.log" 2>&1 &
pid=$!
sleep 12
if kill -0 $pid 2>/dev/null; then echo "L appli tourne toujours apres 12 s"; else echo "L APPLI S EST ARRETEE"; cat "$RUNNER_TEMP/lanceur.log"; fi
screencapture -x "$1" || echo "capture impossible"
kill $pid 2>/dev/null || true
