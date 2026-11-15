# Lanceur

Une seule fenêtre, légère, pour lancer ses jeux **Steam** (et, sous Windows, **Epic Games, Roblox et Minecraft**), plus ce qu'on ajoute à la main. Pour **Windows, Linux et macOS**.

Lanceur ne remplace pas Steam ni Epic : il lit leurs fichiers locaux pour dresser la liste, puis leur demande de lancer le jeu, comme le ferait un raccourci.

![Le dernier jeu lancé en grand, puis les jeux Steam installés, triés par temps de jeu](docs/jeux.jpg)

## Télécharger

| Système | Fichier | Taille |
|---|---|---|
| Windows 10 / 11 | [Lanceur-windows.zip](https://github.com/Pierre-Dunand-Chatellet/Lanceur/releases/latest/download/Lanceur-windows.zip) | 3 Mo |
| Linux (x86_64) | [Lanceur-linux-x86_64.tar.gz](https://github.com/Pierre-Dunand-Chatellet/Lanceur/releases/latest/download/Lanceur-linux-x86_64.tar.gz) | 4 Mo |
| macOS 11 ou plus (Apple Silicon et Intel) | [Lanceur-macos.zip](https://github.com/Pierre-Dunand-Chatellet/Lanceur/releases/latest/download/Lanceur-macos.zip) | 8 Mo |

Rien à installer : on extrait et on lance. Il faut Steam sur l'ordinateur, lancé au moins une fois.

- **Windows** : l'exécutable n'est pas signé, Windows peut afficher « Windows a protégé votre ordinateur » : « Informations complémentaires », puis « Exécuter quand même ».
- **Linux** : `./Lanceur`, ou `./installer.sh` pour l'ajouter au menu des applications (pour ton compte seulement). `zenity` ou `kdialog` pour « Changer la pochette » et « Ajouter un jeu ».
- **macOS** : l'application n'est pas signée par Apple : clic droit sur Lanceur, « Ouvrir », puis « Ouvrir » (une seule fois).

Les LISEZMOI du zip et de l'archive expliquent le reste. Page de présentation : <https://dunandchatellet.fr/lanceur/>

## Ce qu'il fait

- Le dernier jeu lancé en grand, avec son temps de jeu et un bouton Jouer.
- Onglets par provenance : Steam, Famille Steam, Epic, Autres, Tout.
- Jeux installés et à installer dans deux blocs ; un clic sur un jeu non installé lance son installation.
- Temps de jeu et dernier lancement lus dans les données de Steam ; tri par nom, par récents ou par temps de jeu.
- Clic droit : ouvrir le dossier, page du magasin, changer la pochette, masquer, désinstaller.
- Clavier : taper pour chercher, flèches et Entrée pour lancer, Échap pour effacer, F5 pour rescanner.
- Profil : heures totales, jeux les plus joués, derniers lancés, réglages et diagnostic.
- Se réduit ou se ferme au lancement d'un jeu, au choix ; retient la taille de la fenêtre.
- Même programme, même code sur les trois systèmes : seul `src/plat.rs` change d'un système à l'autre.

![Le menu du clic droit](docs/menu.jpg)

![L'onglet Profil](docs/profil.jpg)

## Comment ça marche

- **Rust** avec [eframe / egui](https://github.com/emilk/egui) (OpenGL). Un seul fichier source, `src/main.rs`. Police : Schibsted Grotesk (licence SIL OFL, dans `assets/fonts`). L'exécutable pèse 6 Mo et ne dépend que de Windows.
- **Steam** : la liste complète vient du cache local (`appcache/appinfo.vdf` et `packageinfo.vdf`), les jeux installés des manifestes `appmanifest_*.acf`, le temps de jeu de `userdata/<compte>/config/localconfig.vdf`. Le compte est celui marqué `MostRecent` dans `config/loginusers.vdf`. Aucun compte à saisir, aucune clé d'API.
- **Famille Steam** : Steam ne dit pas en local quels jeux non installés sont prêtés par la famille. Les jeux d'une collection Steam au choix (par défaut « Mes jeux ») sont considérés comme « à moi », le reste comme familial. Les jeux installés d'un autre compte sont reconnus par `LastOwner`.
- **Epic** : manifestes et cache du catalogue du lanceur Epic (`%ProgramData%\Epic\EpicGamesLauncher\Data`). La liste « à installer » vient de ce cache : elle peut contenir des jeux qu'on ne possède pas. Epic ne garde pas le temps de jeu sur le PC.
- **Pochettes** : cache de Steam, puis serveurs d'images de Steam et d'Epic (`curl.exe`), puis le dossier du jeu ; gardées dans `%LOCALAPPDATA%\Lanceur\covers`. Rien d'autre n'est envoyé.
- **Réglages** : `%APPDATA%\Lanceur\state.json`.
- Variable d'environnement `LANCEUR_STEAM` pour imposer le dossier de Steam (installation hors norme, essais).

## Compiler

Le fichier `.github/workflows/compiler.yml` compile pour les trois systèmes, lance les tests, ouvre l'application sur un écran virtuel avec un faux Steam et fabrique les archives.


Il faut la chaîne Rust (cible `x86_64-pc-windows-gnu` ou `-msvc`) :

```
cargo build --release
cargo test --release
cargo test --release -- --ignored --nocapture   # essai sur le vrai PC : dépend de l'installation Steam/Epic
```

## Limites

- Epic Games, Roblox et Minecraft : trouvés tout seuls sous Windows seulement (Roblox et Minecraft aussi sous macOS s'ils sont dans Applications, non vérifié). Epic n'a pas de lanceur pour Linux.
- Les versions Linux et macOS ont été compilées et ouvertes sur des machines virtuelles de GitHub (faux dossier Steam, `ci/faux_steam.py`), jamais avec un vrai Steam sous Linux ni sur un vrai Mac.
- Windows : essayé jusqu'ici sur un seul PC. Si Steam change le format de son cache, un bandeau le dit et seuls les jeux installés sont listés.
- GOG, EA, Ubisoft Connect et Battle.net ne sont pas détectés : « Ajouter un jeu… » dans le Profil.

## À propos

Application **codée avec l'IA** (Claude) : je ne programme pas en Rust. Le besoin, les couleurs, la séparation des jeux et les essais sur ma propre bibliothèque sont de moi. Voir [Pierre Dunand-Chatellet](https://dunandchatellet.fr/).

Les pochettes appartiennent à leurs éditeurs ; Steam et Epic Games sont des marques de leurs propriétaires. Aucune licence n'est donnée pour l'instant.
