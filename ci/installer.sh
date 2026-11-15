#!/bin/sh
# Installe Lanceur pour ton compte seulement (aucun droit administrateur) : programme, icône et entrée dans le menu des applications.
# Pour tout enlever : ~/.local/bin/lanceur, ~/.local/share/applications/lanceur.desktop et ~/.local/share/icons/hicolor/256x256/apps/lanceur.png.
set -e
ici="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/256x256/apps"
cp "$ici/Lanceur" "$HOME/.local/bin/lanceur"
chmod +x "$HOME/.local/bin/lanceur"
cp "$ici/lanceur.png" "$HOME/.local/share/icons/hicolor/256x256/apps/lanceur.png"
sed -e "s|@EXEC@|$HOME/.local/bin/lanceur|" -e "s|@ICON@|lanceur|" "$ici/lanceur.desktop" > "$HOME/.local/share/applications/lanceur.desktop"
echo "Lanceur est installé. Cherche « Lanceur » dans le menu des applications, ou lance ~/.local/bin/lanceur."
