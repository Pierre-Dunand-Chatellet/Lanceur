#!/bin/sh
# Ouvre Lanceur sur l'écran virtuel, vérifie qu'il tourne, puis fait comme un utilisateur (vraie souris et vrai clavier
# simulés par xdotool) : clic sur l'onglet Profil, clic sur Tout, frappe « ka » dans la recherche. Une capture à chaque étape.
# usage : capture_linux.sh <préfixe>   (écrit <préfixe>-steam.png, <préfixe>-profil.png, <préfixe>-recherche.png)
# un gestionnaire de fenêtres minimal : sans lui, la fenêtre n'a jamais le clavier
openbox > /dev/null 2>&1 &
sleep 2
./target/release/lanceur > "$RUNNER_TEMP/lanceur.log" 2>&1 &
pid=$!
sleep 10
if kill -0 $pid 2>/dev/null; then echo "L appli tourne toujours apres 10 s"; else echo "L APPLI S EST ARRETEE"; cat "$RUNNER_TEMP/lanceur.log"; exit 1; fi
import -window root "$1-steam.png"
xdotool mousemove 209 67 click 1; sleep 3
import -window root "$1-profil.png"
xdotool mousemove 127 67 click 1; sleep 2
xdotool search --name Lanceur windowfocus 2>/dev/null || true
sleep 1
xdotool type --delay 150 ka; sleep 3
import -window root "$1-recherche.png"
kill -0 $pid 2>/dev/null && echo "toujours vivante apres les clics et la frappe" || { echo "L APPLI S EST ARRETEE PENDANT LES CLICS"; exit 1; }
kill $pid
