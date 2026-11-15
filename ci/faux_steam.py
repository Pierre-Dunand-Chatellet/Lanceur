"""Fabrique un faux dossier Steam (jeux installés et à installer, compte, temps de jeu, pochettes dessinées) pour essayer
Lanceur sans Steam, sur Linux ou Mac. Aucun vrai jeu : des noms et des pochettes inventés.
usage : python3 faux_steam.py <dossier_steam>"""
import pathlib, struct, sys, time

racine = pathlib.Path(sys.argv[1])
# (appid, nom, minutes de jeu, il y a combien de jours, installé ?) : appid très hauts, aucun ne correspond à un vrai jeu
JEUX = [(99000001, "Course de Karts", 5400, 1, True), (99000002, "Les Tours Sombres", 2100, 6, True), (99000003, "Pixel Farm", 640, 12, True),
        (99000004, "Mini Golf Galaxie", 95, 30, True), (99000005, "Bataille des Plaines", 12300, 2, True), (99000006, "Escape Lab", 0, 0, True),
        (99000007, "Jungle Runner", 780, 45, True), (99000008, "Station Orbitale", 3300, 8, True),
        (99000009, "Rivages Perdus", 0, 0, False), (99000010, "Forge Mécanique", 0, 0, False), (99000011, "Les Nuits de Brume", 0, 0, False)]
COMPTE = 1833512308  # identifiant de compte inventé ; le SteamID64 s'en déduit
STEAMID = str(COMPTE + 76_561_197_960_265_728)

for d in ("steamapps/common", "config", f"userdata/{COMPTE}/config", "appcache/librarycache"):
    (racine / d).mkdir(parents=True, exist_ok=True)

(racine / "steamapps/libraryfolders.vdf").write_text(
    '"libraryfolders"\n{\n\t"0"\n\t{\n\t\t"path"\t\t"%s"\n\t}\n}\n' % str(racine).replace("\\", "\\\\"), encoding="utf-8")
(racine / "config/loginusers.vdf").write_text(
    '"users"\n{\n\t"%s"\n\t{\n\t\t"AccountName"\t\t"essai"\n\t\t"PersonaName"\t\t"Joueur d\'essai"\n\t\t"MostRecent"\t\t"1"\n\t}\n}\n' % STEAMID, encoding="utf-8")

maintenant = int(time.time())
apps = ""
for appid, nom, minutes, jours, installe in JEUX:
    dernier = maintenant - jours * 86400 if minutes else 0
    if installe:
        (racine / f"steamapps/common/jeu{appid}").mkdir(exist_ok=True)
        (racine / f"steamapps/appmanifest_{appid}.acf").write_text(
            '"AppState"\n{\n\t"appid"\t\t"%d"\n\t"name"\t\t"%s"\n\t"StateFlags"\t\t"4"\n\t"installdir"\t\t"jeu%d"\n\t"LastOwner"\t\t"%s"\n\t"SizeOnDisk"\t\t"%d"\n\t"LastPlayed"\t\t"%d"\n}\n'
            % (appid, nom, appid, STEAMID, 4_000_000_000 + appid % 1000 * 1000, dernier), encoding="utf-8")
        apps += '\t\t\t\t\t"%d"\n\t\t\t\t\t{\n\t\t\t\t\t\t"LastPlayed"\t\t"%d"\n\t\t\t\t\t\t"Playtime"\t\t"%d"\n\t\t\t\t\t}\n' % (appid, dernier, minutes)
(racine / f"userdata/{COMPTE}/config/localconfig.vdf").write_text(
    '"UserLocalConfigStore"\n{\n\t"Software"\n\t{\n\t\t"Valve"\n\t\t{\n\t\t\t"Steam"\n\t\t\t{\n\t\t\t\t"apps"\n\t\t\t\t{\n%s\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n' % apps, encoding="utf-8")

# --- bibliothèque complète (format binaire de Steam : appinfo.vdf et packageinfo.vdf) : jeux installés ET à installer ---
CLES = ["common", "type", "name"]
corps_apps, corps = [], b""
for appid, nom, *_ in JEUX:
    kv = bytes([0]) + struct.pack("<I", 0)                              # bloc « common »
    kv += bytes([1]) + struct.pack("<I", 1) + b"game\0"                 # type = game
    kv += bytes([1]) + struct.pack("<I", 2) + nom.encode() + b"\0"      # name
    kv += bytes([8]) + bytes([8])                                       # fin de « common », fin de la racine
    data = bytes(60) + kv
    corps += struct.pack("<II", appid, len(data)) + data
corps += struct.pack("<I", 0)
table = struct.pack("<I", len(CLES)) + b"".join(c.encode() + b"\0" for c in CLES)
(racine / "appcache").mkdir(exist_ok=True)
(racine / "appcache/appinfo.vdf").write_bytes(struct.pack("<IIQ", 0x0756_4429, 1, 16 + len(corps)) + corps + table)

licences = b"".join(bytes([2]) + str(i).encode() + b"\0" + struct.pack("<I", appid) for i, (appid, *_r) in enumerate(JEUX))
kv = bytes([0]) + b"1\0" + bytes([0]) + b"appids\0" + licences + bytes([8, 8, 8])  # licence n° 1 -> liste des appids
(racine / "appcache/packageinfo.vdf").write_bytes(bytes(8) + struct.pack("<I", 1) + bytes(32) + kv + struct.pack("<I", 0xFFFFFFFF))

# --- pochettes dessinées (Pillow), rangées comme le fait Steam : appcache/librarycache/<appid>/library_600x900.jpg ---
try:
    from PIL import Image, ImageDraw, ImageFont
    import colorsys
    for k, (appid, nom, *_r) in enumerate(JEUX):
        teinte = (k * 0.17 + 0.05) % 1
        haut = tuple(int(c * 255) for c in colorsys.hsv_to_rgb(teinte, 0.55, 0.85))
        bas = tuple(int(c * 255) for c in colorsys.hsv_to_rgb((teinte + 0.08) % 1, 0.7, 0.28))
        im = Image.new("RGB", (600, 900))
        px = im.load()
        for y in range(900):
            f = y / 899
            ligne = tuple(int(haut[i] * (1 - f) + bas[i] * f) for i in range(3))
            for x in range(600):
                px[x, y] = ligne
        d = ImageDraw.Draw(im)
        try:
            police = ImageFont.load_default(size=78)
        except TypeError:
            police = ImageFont.load_default()
        mots, lignes, cur = nom.split(), [], ""
        for m in mots:
            if len(cur + " " + m) > 11 and cur:
                lignes.append(cur); cur = m
            else:
                cur = (cur + " " + m).strip()
        lignes.append(cur)
        y = 450 - 50 * len(lignes)
        for l in lignes:
            largeur = d.textlength(l, font=police)
            d.text(((600 - largeur) / 2, y), l, fill=(255, 255, 255), font=police)
            y += 100
        dossier = racine / f"appcache/librarycache/{appid}"
        dossier.mkdir(exist_ok=True)
        im.save(dossier / "library_600x900.jpg", quality=88)
except ImportError:
    print("Pillow absent : pas de pochettes dessinées (des lettres à la place)")
print(f"faux Steam prêt dans {racine} : {len(JEUX)} jeux")
