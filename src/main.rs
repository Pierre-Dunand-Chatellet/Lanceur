#![windows_subsystem = "windows"]
use eframe::egui::{self, Align, Color32, ColorImage, FontId, Layout, RichText, Sense, TextureHandle, Vec2};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        mpsc, Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// Vrai quand la bibliothèque Steam n'a pas pu être lue (format du cache changé) : seuls les jeux installés s'affichent.
static LIBRARY_UNREADABLE: AtomicBool = AtomicBool::new(false);

const ACCENTS: [Color32; 2] = [Color32::from_rgb(0x8b, 0x5c, 0xf6), Color32::from_rgb(0xf4, 0x3f, 0x5e)]; // violet, rouge
const TEX_W: u32 = 200;
const TEX_H: u32 = 300;
const NO_WINDOW: u32 = 0x0800_0000;
const MINECRAFT: &str = "Microsoft.4297127D64EC6_8wekyb3d8bbwe";

#[derive(Clone, Copy, PartialEq, Default)]
enum Store {
    #[default]
    Steam,
    Epic,
    Other, // Roblox, Minecraft, jeux ajoutés à la main
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Installed,
    Fav,
    Hidden,
}

/// Ce qu'on peut demander sur un jeu (clic, clic droit, clavier).
#[derive(Clone, Copy)]
enum Act {
    Play,
    Fav,
    Hide,
    Folder,
    StorePage,
    Uninstall,
    Cover,
    CoverReset,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
/// 95 -> « 1,6 h » ; 40 -> « 40 min » ; 1537 -> « 26 h »
fn fmt_time(min: u32) -> String {
    match min {
        0..=59 => format!("{min} min"),
        60..=599 => format!("{},{} h", min / 60, min % 60 / 6),
        _ => format!("{} h", (min + 30) / 60),
    }
}
fn fmt_ago(t: u64) -> String {
    match now().saturating_sub(t) / 86_400 {
        0 => "aujourd'hui".into(),
        1 => "hier".into(),
        d @ 2..=59 => format!("il y a {d} jours"),
        d @ 60..=729 => format!("il y a {} mois", d / 30),
        d => format!("il y a {} ans", d / 365),
    }
}

/// Où chercher une pochette (essayées dans l'ordre).
#[derive(Clone)]
enum Src {
    File(PathBuf),
    Dir(PathBuf), // dossier du jeu : exploré plus tard, dans le thread des pochettes
    Url(String, String), // url, nom du fichier en cache
}

#[derive(Clone, Default)]
struct Game {
    id: String,
    name: String,
    store: Store,
    installed: bool,
    family: bool, // jeu de la bibliothèque famille Steam (pas à moi)
    update: bool, // installé mais Steam doit d'abord le mettre à jour / finir de le télécharger
    launch_url: String,
    install_url: String,
    covers: Vec<Src>,
    last_played: u64,
    playtime: u32,        // minutes (Steam seulement : Epic ne garde rien en local)
    dir: Option<PathBuf>, // dossier d'installation
    size: u64,            // octets sur le disque
}

impl Game {
    /// Onglet du jeu : 0 = mes jeux Steam, 1 = famille Steam, 4 = Epic, 5 = autres
    /// (2 = Tout et 3 = Profil ne sont pas des groupes ; les numéros 0 à 3 restent ceux de state.json).
    fn group(&self) -> usize {
        match self.store {
            Store::Steam => self.family as usize,
            Store::Epic => 4,
            Store::Other => 5,
        }
    }
}

// ---------- état sauvegardé (favoris, dernier lancement, préférences) ----------

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct State {
    favs: HashSet<String>,
    last: HashMap<String, u64>,
    accent: usize,
    card_w: f32,
    scope: usize,                  // 0 = mes jeux Steam, 1 = famille Steam, 2 = tout, 3 = profil, 4 = Epic, 5 = autres (voir Game::group)
    sort: usize,                   // 0 = nom, 1 = récents, 2 = temps de jeu
    hidden: HashSet<String>,       // jeux masqués
    on_launch: usize,              // 0 = rester ouvert, 1 = se réduire, 2 = se fermer
    win: Option<[f32; 2]>,         // taille de la fenêtre à la fermeture
    maximized: bool,
    custom: Vec<(String, String)>, // jeux ajoutés à la main : nom, chemin (exe, raccourci)
    collection: String,            // collection Steam qui contient « mes jeux » : vide = « Mes jeux », « - » = aucune (voir coll_name)
}
impl Default for State {
    fn default() -> Self {
        State { favs: HashSet::new(), last: HashMap::new(), accent: 0, card_w: 170.0, scope: 0, sort: 0, hidden: HashSet::new(), on_launch: 0, win: None, maximized: false, custom: vec![], collection: String::new() }
    }
}
fn app_dir(var: &str) -> PathBuf {
    PathBuf::from(std::env::var(var).unwrap_or_default()).join("Lanceur")
}
fn state_path() -> PathBuf {
    app_dir("APPDATA").join("state.json")
}
fn load_state() -> State {
    fs::read_to_string(state_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}
fn save_state(s: &State) {
    let p = state_path();
    let _ = fs::create_dir_all(p.parent().unwrap());
    let _ = fs::write(p, serde_json::to_string(s).unwrap_or_default());
}

// ---------- lecture du format binaire Valve (appinfo.vdf / packageinfo.vdf) ----------

enum V {
    S(String),
    N(i64),
    M(Vec<(String, V)>),
}
impl V {
    fn get(&self, k: &str) -> Option<&V> {
        match self {
            V::M(m) => m.iter().find(|(a, _)| a == k).map(|(_, v)| v),
            _ => None,
        }
    }
    fn s(&self) -> Option<&str> {
        if let V::S(s) = self { Some(s) } else { None }
    }
    fn n(&self) -> Option<i64> {
        if let V::N(n) = self { Some(*n) } else { None }
    }
}
fn u32le(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn cstr(b: &[u8], o: &mut usize) -> Option<String> {
    let e = b.get(*o..)?.iter().position(|&c| c == 0)?;
    let s = String::from_utf8_lossy(&b[*o..*o + e]).into_owned();
    *o += e + 1;
    Some(s)
}
/// `st` = table de chaînes (appinfo v41) ; sinon les clés sont écrites en clair (packageinfo).
fn kv(b: &[u8], o: &mut usize, st: Option<&[String]>) -> Option<V> {
    let mut m = vec![];
    loop {
        let t = *b.get(*o)?;
        *o += 1;
        if t == 8 {
            return Some(V::M(m));
        }
        let key = match st {
            Some(st) => {
                let i = u32le(b, *o)? as usize;
                *o += 4;
                st.get(i)?.clone()
            }
            None => cstr(b, o)?,
        };
        let v = match t {
            0 => kv(b, o, st)?,
            1 => V::S(cstr(b, o)?),
            2 | 3 | 4 | 6 => {
                let v = u32le(b, *o)? as i32;
                *o += 4;
                V::N(v as i64)
            }
            5 => {
                while !(*b.get(*o)? == 0 && *b.get(*o + 1)? == 0) {
                    *o += 2;
                }
                *o += 2;
                V::N(0)
            }
            7 | 10 => {
                let v = u64::from_le_bytes(b.get(*o..*o + 8)?.try_into().ok()?);
                *o += 8;
                V::N(v as i64)
            }
            _ => return None,
        };
        m.push((key, v));
    }
}

/// Bibliothèque complète du compte, reconstituée depuis le cache local de Steam :
/// packageinfo = les licences, appinfo = les noms et types. Aucun réseau, aucune clé d'API.
fn steam_owned(root: &Path, extra: &HashSet<u32>) -> Vec<(u32, String)> {
    let (Ok(a), Ok(q)) = (fs::read(root.join("appcache/appinfo.vdf")), fs::read(root.join("appcache/packageinfo.vdf"))) else {
        LIBRARY_UNREADABLE.store(true, Relaxed);
        return vec![];
    };
    let r = owned_inner(&a, &q, extra);
    LIBRARY_UNREADABLE.store(r.is_none(), Relaxed);
    r.unwrap_or_default()
}
/// `extra` : appids à inclure même sans licence lisible ni type « game » (ex. jeux gratuits de la collection « Mes jeux »).
fn owned_inner(a: &[u8], q: &[u8], extra: &HashSet<u32>) -> Option<Vec<(u32, String)>> {
    if u32le(a, 0)? != 0x0756_4429 {
        return None; // autre version du format : on retombe sur les jeux installés
    }
    let st_off = u64::from_le_bytes(a.get(8..16)?.try_into().ok()?) as usize;
    let n = u32le(a, st_off)? as usize;
    let mut o = st_off + 4;
    let mut st = Vec::with_capacity(n);
    for _ in 0..n {
        st.push(cstr(a, &mut o)?);
    }
    let mut games: HashMap<u32, String> = HashMap::new();
    let mut o = 16;
    loop {
        let aid = u32le(a, o)?;
        if aid == 0 {
            break;
        }
        let size = u32le(a, o + 4)? as usize;
        let body = a.get(o + 8..o + 8 + size)?;
        let mut p = 60;
        if let Some(r) = kv(body, &mut p, Some(&st)) {
            let c = r.get("appinfo").unwrap_or(&r).get("common");
            let ty = c.and_then(|c| c.get("type")).and_then(V::s).unwrap_or("").to_lowercase();
            if let (true, Some(name)) = (ty == "game" || extra.contains(&aid), c.and_then(|c| c.get("name")).and_then(V::s)) {
                games.insert(aid, name.to_string());
            }
        }
        o += 8 + size;
    }
    let mut owned = HashSet::new();
    let mut o = 8;
    loop {
        let pid = u32le(q, o)?;
        if pid == u32::MAX {
            break;
        }
        o += 4 + 20 + 4 + 8;
        let d = kv(q, &mut o, None)?;
        if let Some(V::M(ids)) = d.get(&pid.to_string()).unwrap_or(&d).get("appids") {
            owned.extend(ids.iter().filter_map(|(_, v)| v.n()).map(|n| n as u32));
        }
    }
    let mut out: Vec<_> = games.into_iter().filter(|(id, _)| owned.contains(id) || extra.contains(id)).collect();
    out.sort();
    Some(out)
}

// ---------- scan Steam ----------

/// Toutes les chaînes entre guillemets d'une ligne VDF : `"k"  "v"` -> [k, v]
fn quoted(line: &str) -> Vec<String> {
    line.split('"').enumerate().filter(|(i, _)| i % 2 == 1).map(|(_, s)| s.replace("\\\\", "\\")).collect()
}

fn steam_root() -> Option<PathBuf> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    if let Some(p) = std::env::var_os("LANCEUR_STEAM") {
        return Some(PathBuf::from(p)).filter(|p| p.exists()); // dossier Steam imposé (installation hors norme, essais)
    }
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Valve\\Steam")
        .ok()
        .and_then(|k| k.get_value::<String, _>("SteamPath").ok())
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(r"C:\Program Files (x86)\Steam")))
        .filter(|p| p.exists())
}

fn cache_file(root: &Path, id: &str, name: &str) -> Option<PathBuf> {
    let cache = root.join("appcache").join("librarycache");
    let flat = cache.join(format!("{id}_{name}"));
    if flat.exists() {
        return Some(flat);
    }
    for e in fs::read_dir(cache.join(id)).into_iter().flatten().flatten() {
        let p = e.path();
        let f = if p.is_dir() { p.join(name) } else { p };
        if f.file_name().is_some_and(|n| n == name) && f.exists() {
            return Some(f);
        }
    }
    None
}

/// Meilleure image trouvée dans le dossier d'installation (2 niveaux) : plus gros fichier dont le nom ressemble à une pochette.
fn folder_image(dir: &Path) -> Option<PathBuf> {
    let hints = ["cover", "poster", "key_art", "keyart", "splash", "banner", "box", "logo", "icon"];
    let mut best: Option<(u64, PathBuf)> = None;
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 2 {
                    stack.push((e.path(), depth + 1));
                }
                continue;
            }
            let n = e.file_name().to_string_lossy().to_lowercase();
            if !(n.ends_with(".jpg") || n.ends_with(".jpeg") || n.ends_with(".png")) || !hints.iter().any(|h| n.contains(h)) {
                continue;
            }
            let len = e.metadata().map(|m| m.len()).unwrap_or(0);
            if (8_000..8_000_000).contains(&len) && best.as_ref().is_none_or(|b| len > b.0) {
                best = Some((len, e.path()));
            }
        }
    }
    best.map(|b| b.1)
}

fn steam_covers(root: &Path, id: &str, dir: Option<&Path>) -> Vec<Src> {
    let cdn = |f: &str| format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{id}/{f}");
    let mut v = vec![];
    v.extend(cache_file(root, id, "library_600x900.jpg").map(Src::File));
    v.push(Src::Url(cdn("library_600x900.jpg"), format!("steam_{id}_p.jpg")));
    v.extend(dir.map(|d| Src::Dir(d.to_path_buf())));
    v.extend(["header.jpg", "library_header.jpg"].iter().filter_map(|n| cache_file(root, id, n)).map(Src::File));
    v.push(Src::Url(cdn("header.jpg"), format!("steam_{id}_h.jpg")));
    // jeux récents sans pochette ni bannière : la grande image « hero », recadrée au centre
    v.extend(cache_file(root, id, "library_hero.jpg").map(Src::File));
    v.push(Src::Url(cdn("library_hero.jpg"), format!("steam_{id}_hero.jpg")));
    v
}

/// Compte Steam connecté : (SteamID64, pseudo), lus dans loginusers.vdf. Plusieurs comptes sur le PC :
/// celui marqué `MostRecent` (le dernier connecté), à défaut le premier.
fn steam_user(root: &Path) -> Option<(String, String)> {
    parse_users(&String::from_utf8_lossy(&fs::read(root.join("config/loginusers.vdf")).ok()?))
}
fn parse_users(t: &str) -> Option<(String, String)> {
    let mut users: Vec<(String, String, bool)> = vec![]; // (id, pseudo, dernier connecté)
    for q in t.lines().map(quoted) {
        if q.len() == 1 && q[0].len() == 17 && q[0].chars().all(|c| c.is_ascii_digit()) {
            users.push((q[0].clone(), String::new(), false));
        } else if let (Some(u), 2) = (users.last_mut(), q.len()) {
            match q[0].as_str() {
                "PersonaName" => u.1 = q[1].clone(),
                "MostRecent" => u.2 = q[1] == "1",
                _ => {}
            }
        }
    }
    let i = users.iter().position(|u| u.2).unwrap_or(0);
    users.into_iter().nth(i).map(|u| (u.0, u.1))
}
fn steam_id(root: &Path) -> Option<String> {
    steam_user(root).map(|u| u.0)
}

/// Dossier `userdata` du compte connecté (ID de compte = SteamID64 - 76561197960265728) ; à défaut, tous ceux qui existent.
fn user_dirs(root: &Path) -> Vec<PathBuf> {
    let base = root.join("userdata");
    let mine = steam_id(root).and_then(|s| s.parse::<u64>().ok()).map(|n| base.join(n.saturating_sub(76_561_197_960_265_728).to_string())).filter(|p| p.exists());
    mine.map(|p| vec![p]).unwrap_or_else(|| fs::read_dir(&base).into_iter().flatten().flatten().map(|e| e.path()).collect())
}

/// Pseudo Steam et fichier de l'avatar (cache local de Steam).
fn steam_profile() -> (String, Option<PathBuf>) {
    let Some(root) = steam_root() else { return (String::new(), None) };
    let (id, name) = steam_user(&root).unwrap_or_default();
    let avatar = Some(root.join("config/avatarcache").join(format!("{id}.png"))).filter(|p| !id.is_empty() && p.exists());
    (name, avatar)
}

/// Temps de jeu (minutes) et dernier lancement par appid : Steam les garde en local dans localconfig.vdf,
/// même pour les jeux désinstallés.
fn steam_playtime(root: &Path) -> HashMap<String, (u32, u64)> {
    let mut out = HashMap::new();
    for u in user_dirs(root) {
        if let Ok(b) = fs::read(u.join("config/localconfig.vdf")) {
            parse_playtime(&String::from_utf8_lossy(&b), &mut out);
        }
    }
    out
}
/// VDF texte : on suit l'imbrication des blocs et on ne garde que `apps/<appid>/{Playtime, LastPlayed}`.
fn parse_playtime(t: &str, out: &mut HashMap<String, (u32, u64)>) {
    let (mut stack, mut last): (Vec<String>, String) = (vec![], String::new());
    for l in t.lines() {
        let s = l.trim();
        if s == "{" {
            stack.push(std::mem::take(&mut last));
        } else if s == "}" {
            stack.pop();
        } else {
            let q = quoted(s);
            let n = stack.len();
            if q.len() == 1 {
                last = q[0].clone();
            } else if q.len() == 2 && n >= 2 && stack[n - 2].eq_ignore_ascii_case("apps") && (q[0] == "Playtime" || q[0] == "LastPlayed") {
                let e = out.entry(stack[n - 1].clone()).or_insert((0, 0));
                if q[0] == "Playtime" {
                    e.0 = e.0.max(q[1].parse().unwrap_or(0));
                } else {
                    e.1 = e.1.max(q[1].parse().unwrap_or(0));
                }
            }
        }
    }
}

fn owned_cache() -> PathBuf {
    app_dir("LOCALAPPDATA").join("owned.json")
}
fn load_owned() -> Option<HashSet<u32>> {
    serde_json::from_str(&fs::read_to_string(owned_cache()).ok()?).ok()
}

/// Collections Steam (statiques, créées par l'utilisateur) du compte connecté : (nom, appids).
/// Lues dans userdata/<compte>/config/cloudstorage, donc en local et instantané.
fn collections() -> Vec<(String, HashSet<u32>)> {
    let mut out = vec![];
    let Some(root) = steam_root() else { return out };
    for u in user_dirs(&root) {
        let Ok(t) = fs::read_to_string(u.join("config/cloudstorage/cloud-storage-namespace-1.json")) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
        for e in v.as_array().into_iter().flatten() {
            let Some(c) = e[1]["value"].as_str().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok()) else { continue };
            let ids: HashSet<u32> = c["added"].as_array().into_iter().flatten().filter_map(|x| x.as_u64().map(|n| n as u32)).collect();
            if let (Some(name), false, false) = (c["name"].as_str(), ids.is_empty(), e[1]["is_deleted"].as_bool().unwrap_or(false)) {
                out.push((name.to_string(), ids));
            }
        }
    }
    out
}

/// Nom de la collection « mes jeux » d'après le réglage : vide = « Mes jeux » (le nom de Pierre), « - » = aucune.
fn coll_name(setting: &str) -> Option<&str> {
    match setting {
        "" => Some("Mes jeux"),
        "-" => None,
        n => Some(n),
    }
}

/// Les jeux de cette collection : ce sont « mes jeux » (le reste de la bibliothèque = famille Steam).
fn collection_owned(setting: &str) -> Option<HashSet<u32>> {
    let name = coll_name(setting)?;
    collections().into_iter().find(|c| c.0.eq_ignore_ascii_case(name)).map(|c| c.1)
}

/// Mes jeux (hors famille) : liste publique de mon profil. Vide/None si le profil est privé.
fn fetch_owned(sid: &str) -> Option<HashSet<u32>> {
    let url = format!("https://steamcommunity.com/profiles/{sid}/games?tab=all&xml=1");
    let out = Command::new("curl.exe").args(["-s", "-L", "-m", "20", &url]).creation_flags(NO_WINDOW).output().ok()?;
    let t = String::from_utf8_lossy(&out.stdout);
    if !t.contains("<gamesList>") {
        return None;
    }
    let ids: HashSet<u32> = t.split("<appID>").skip(1).filter_map(|s| s.split('<').next()?.trim().parse().ok()).collect();
    (!ids.is_empty()).then_some(ids)
}

fn spawn_owned(ctx: egui::Context, coll: &str) -> mpsc::Receiver<HashSet<u32>> {
    let (tx, rx) = mpsc::channel();
    if collection_owned(coll).is_some() {
        return rx; // la collection locale suffit : aucun accès réseau
    }
    std::thread::spawn(move || {
        let Some(sid) = steam_root().and_then(|r| steam_id(&r)) else { return };
        if let Some(set) = fetch_owned(&sid) {
            let _ = fs::create_dir_all(app_dir("LOCALAPPDATA"));
            let _ = fs::write(owned_cache(), serde_json::to_string(&set).unwrap_or_default());
            let _ = tx.send(set);
            ctx.request_repaint();
        }
    });
    rx
}

/// Jeux Steam non installés : famille = absent de ma liste publique (si on la connaît).
fn apply_family(games: &mut [Game], owned: &Option<HashSet<u32>>) {
    let Some(owned) = owned else { return };
    for g in games.iter_mut().filter(|g| g.store == Store::Steam && !g.installed) {
        let id: u32 = g.id.trim_start_matches("steam:").parse().unwrap_or(0);
        g.family = !owned.contains(&id);
    }
}

fn scan_steam(coll: &str) -> Vec<Game> {
    let Some(root) = steam_root() else { return vec![] };
    let me = steam_id(&root).unwrap_or_default();
    let played = steam_playtime(&root);
    let mut libs = vec![root.clone()];
    if let Ok(t) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        for l in t.lines() {
            let q = quoted(l);
            if q.len() == 2 && q[0] == "path" {
                libs.push(PathBuf::from(&q[1]));
            }
        }
    }
    let mut out: HashMap<String, Game> = HashMap::new();
    // 1) jeux installés (manifests)
    for lib in libs {
        let Ok(rd) = fs::read_dir(lib.join("steamapps")) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
                continue;
            }
            let Ok(t) = fs::read_to_string(e.path()) else { continue };
            let (mut id, mut name, mut last, mut dir, mut owner) = (String::new(), String::new(), 0u64, String::new(), String::new());
            let mut flags = 4u32; // 4 = entièrement installé
            let mut size = 0u64;
            for l in t.lines() {
                let q = quoted(l);
                if q.len() == 2 {
                    match q[0].as_str() {
                        "StateFlags" => flags = q[1].parse().unwrap_or(4),
                        "SizeOnDisk" => size = q[1].parse().unwrap_or(0),
                        "LastOwner" => owner = q[1].clone(),
                        "appid" => id = q[1].clone(),
                        "name" => name = q[1].clone(),
                        "installdir" => dir = q[1].clone(),
                        "LastPlayed" => last = q[1].parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            let skip = ["Steamworks Common", "Proton", "Steam Linux Runtime", "Redistributable"];
            if id.is_empty() || name.is_empty() || skip.iter().any(|s| name.contains(s)) {
                continue;
            }
            let folder = lib.join("steamapps").join("common").join(&dir);
            let (pt, lp) = played.get(&id).copied().unwrap_or((0, 0));
            out.insert(
                format!("steam:{id}"),
                Game {
                    id: format!("steam:{id}"),
                    covers: steam_covers(&root, &id, Some(&folder)),
                    launch_url: format!("steam://rungameid/{id}"),
                    install_url: format!("steam://install/{id}"),
                    name,
                    store: Store::Steam,
                    installed: true,
                    family: !owner.is_empty() && !me.is_empty() && owner != me,
                    update: flags != 4,
                    last_played: last.max(lp),
                    playtime: pt,
                    dir: Some(folder),
                    size,
                },
            );
        }
    }
    // 2) le reste de la bibliothèque (non installé)
    let skip = ["Test Server", "Dedicated Server", "Playtest", "SDK"];
    for (aid, name) in steam_owned(&root, &collection_owned(coll).unwrap_or_default()) {
        let key = format!("steam:{aid}");
        if aid == 480 || out.contains_key(&key) || skip.iter().any(|s| name.contains(s)) {
            continue;
        }
        let id = aid.to_string();
        let (pt, lp) = played.get(&id).copied().unwrap_or((0, 0));
        out.insert(
            key.clone(),
            Game {
                id: key,
                covers: steam_covers(&root, &id, None),
                launch_url: format!("steam://rungameid/{id}"),
                install_url: format!("steam://install/{id}"),
                name,
                last_played: lp,
                playtime: pt,
                ..Default::default()
            },
        );
    }
    out.into_values().collect()
}

// ---------- scan Epic ----------

struct CatItem {
    title: String,
    ns: String,
    app: String,
    image: Option<String>,
}

/// Données du lanceur Epic (%ProgramData%, pas forcément sur C:).
fn epic_data() -> PathBuf {
    PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join(r"Epic\EpicGamesLauncher\Data")
}

/// Cache du catalogue Epic (base64 -> JSON) : titres, identifiants et pochettes des jeux connus du compte.
fn epic_catalog() -> HashMap<String, CatItem> {
    use base64::Engine;
    let mut out = HashMap::new();
    let Ok(raw) = fs::read(epic_data().join(r"Catalog\catcache.bin")) else { return out };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(raw.iter().copied().filter(|c| !c.is_ascii_whitespace()).collect::<Vec<_>>()) else { return out };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return out };
    for e in v.as_array().into_iter().flatten() {
        let is_game = e["categories"].as_array().is_some_and(|a| a.iter().any(|c| c["path"] == "games"));
        let s = |x: &serde_json::Value| x.as_str().unwrap_or("").to_string();
        let imgs = e["keyImages"].as_array();
        let img = |t: &str| imgs.and_then(|a| a.iter().find(|k| k["type"] == t)).map(|k| s(&k["url"]));
        let item = CatItem {
            title: s(&e["title"]),
            ns: s(&e["namespace"]),
            app: s(&e["releaseInfo"][0]["appId"]),
            image: img("DieselGameBoxTall").or_else(|| img("DieselGameBox")),
        };
        if is_game && !item.app.is_empty() && !item.title.contains("Public Testing") {
            out.insert(s(&e["id"]), item);
        }
    }
    out
}

fn epic_url(ns: &str, item: &str, app: &str, action: &str) -> String {
    format!("com.epicgames.launcher://apps/{ns}%3A{item}%3A{app}?action={action}&silent=true")
}

fn scan_epic() -> Vec<Game> {
    let cat = epic_catalog();
    let mut out = vec![];
    let mut seen = HashSet::new();
    for e in fs::read_dir(epic_data().join("Manifests")).into_iter().flatten().flatten() {
        if e.path().extension().is_none_or(|x| x != "item") {
            continue;
        }
        let Some(v) = fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let is_game = v["AppCategories"].as_array().is_none_or(|a| a.iter().any(|c| c == "games"));
        let (name, app, ns, item) = (s("DisplayName"), s("AppName"), s("CatalogNamespace"), s("CatalogItemId"));
        if !is_game || name.is_empty() || app.is_empty() {
            continue;
        }
        let mut covers: Vec<Src> = cat.get(&item).and_then(|c| c.image.clone()).map(|u| Src::Url(u, format!("epic_{app}.jpg"))).into_iter().collect();
        covers.push(Src::Dir(PathBuf::from(s("InstallLocation"))));
        seen.insert(app.clone());
        out.push(Game {
            id: format!("epic:{app}"),
            launch_url: epic_url(&ns, &item, &app, "launch"),
            install_url: epic_url(&ns, &item, &app, "install"),
            name,
            store: Store::Epic,
            installed: true,
            covers,
            dir: Some(PathBuf::from(s("InstallLocation"))),
            size: v["InstallSize"].as_u64().unwrap_or(0),
            ..Default::default()
        });
    }
    for (item, c) in &cat {
        if seen.contains(&c.app) {
            continue;
        }
        seen.insert(c.app.clone());
        out.push(Game {
            id: format!("epic:{}", c.app),
            launch_url: epic_url(&c.ns, item, &c.app, "launch"),
            install_url: epic_url(&c.ns, item, &c.app, "install"),
            name: c.title.clone(),
            store: Store::Epic,
            covers: c.image.clone().map(|u| Src::Url(u, format!("epic_{}.jpg", c.app))).into_iter().collect(),
            ..Default::default()
        });
    }
    // un même jeu peut avoir deux identifiants (manifeste installé + catalogue) : on garde l'installé, déjà placé en premier
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let mut titles = HashSet::new();
    out.retain(|g| titles.insert(norm(&g.name)));
    out
}

/// Ce que le lanceur a trouvé sur le PC : (trouvé ?, phrase). Sert au premier lancement et au diagnostic du Profil.
fn setup_lines() -> Vec<(bool, String)> {
    let mut v = vec![];
    match steam_root() {
        Some(r) => {
            let (id, name) = steam_user(&r).unwrap_or_default();
            v.push((true, format!("Steam trouvé : {}", r.display())));
            v.push((!id.is_empty(), if id.is_empty() { "Aucun compte Steam connecté (ouvre Steam et connecte-toi)".to_string() } else { format!("Compte Steam : {}", if name.is_empty() { &id } else { &name }) }));
            if LIBRARY_UNREADABLE.load(Relaxed) {
                v.push((false, "Bibliothèque Steam illisible : seuls les jeux installés sont listés".to_string()));
            }
        }
        None => v.push((false, "Steam introuvable".to_string())),
    }
    let epic = epic_data().join("Manifests").exists();
    v.push((epic, if epic { "Epic Games Launcher trouvé" } else { "Epic Games Launcher introuvable" }.to_string()));
    v
}

/// Hors boutiques : Roblox et Minecraft s'ils sont là, plus les jeux ajoutés à la main (Profil > Ajouter un jeu).
/// Ubisoft Connect : rien à détecter ici (ses jeux installés passent par Steam).
fn scan_other(custom: &[(String, String)]) -> Vec<Game> {
    let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default());
    let other = |id: String, name: &str, target: String, dir: Option<PathBuf>| Game {
        id,
        name: name.to_string(),
        store: Store::Other,
        installed: true,
        launch_url: target,
        // pas de recherche d'image dans le dossier : pour un raccourci du Bureau ou un .exe de System32, elle ramène n'importe quoi
        // (pochette = lettre, ou celle choisie par clic droit)
        dir,
        ..Default::default()
    };
    let mut out = vec![];
    // Roblox : un dossier par version, on prend la plus récente
    let roblox = fs::read_dir(local.join("Roblox/Versions")).into_iter().flatten().flatten().map(|e| e.path().join("RobloxPlayerBeta.exe")).filter(|p| p.exists());
    if let Some(exe) = roblox.max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok()) {
        out.push(other("other:roblox".into(), "Roblox", exe.to_string_lossy().into_owned(), exe.parent().map(Path::to_path_buf)));
    }
    if local.join("Packages").join(MINECRAFT).exists() {
        out.push(other("other:minecraft".into(), "Minecraft", format!("shell:AppsFolder\\{MINECRAFT}!Minecraft"), None));
    }
    for (name, path) in custom {
        out.push(other(format!("other:{path}"), name, path.clone(), Path::new(path).parent().map(Path::to_path_buf)));
    }
    out
}

fn scan_all(owned: &Option<HashSet<u32>>, custom: &[(String, String)], coll: &str) -> Vec<Game> {
    let mut g = scan_steam(coll);
    apply_family(&mut g, owned);
    g.extend(scan_epic());
    g.extend(scan_other(custom));
    g
}

fn open_url(url: &str) {
    let _ = Command::new("explorer.exe").arg(url).creation_flags(NO_WINDOW).spawn();
}

/// Boîte « Ouvrir » de Windows, dans un thread (le chemin choisi arrive par le canal).
// ponytail: passe par PowerShell (≈1 s d'attente) pour ne pas ajouter de dépendance ; crate `rfd` si ça gêne.
fn pick_file(filter: &'static str, what: Pick, tx: mpsc::Sender<(Pick, String)>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; [Console]::OutputEncoding=[Text.Encoding]::UTF8; \
             $f=New-Object Windows.Forms.Form -Property @{{TopMost=$true}}; $d=New-Object Windows.Forms.OpenFileDialog; \
             $d.Filter='{filter}'; $d.DereferenceLinks=$false; if($d.ShowDialog($f) -eq 'OK'){{$d.FileName}}"
        );
        let Ok(out) = Command::new("powershell.exe").args(["-NoProfile", "-STA", "-Command", &script]).creation_flags(NO_WINDOW).output() else { return };
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !path.is_empty() {
            let _ = tx.send((what, path));
            ctx.request_repaint();
        }
    });
}
enum Pick {
    Cover(String), // pochette personnalisée pour ce jeu
    AddGame,
}
/// Pochette choisie par l'utilisateur : elle passe avant toutes les autres.
fn custom_cover(id: &str) -> PathBuf {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    app_dir("LOCALAPPDATA").join("covers").join(format!("custom_{safe}"))
}

// ---------- pochettes : récupération + mise à la bonne taille (200x300) ----------

fn fetch(url: &str, to: &Path) -> bool {
    let ok = Command::new("curl.exe").args(["-s", "-f", "-L", "-m", "15", "-o"]).arg(to).arg(url).creation_flags(NO_WINDOW).status().is_ok_and(|s| s.success());
    if !ok || fs::metadata(to).map(|m| m.len()).unwrap_or(0) < 1000 {
        let _ = fs::remove_file(to);
        return false;
    }
    true
}

fn open_image(p: &Path) -> Option<image::DynamicImage> {
    image::ImageReader::open(p).ok()?.with_guessed_format().ok()?.decode().ok()
}

fn resolve(src: &Src, cache: &Path) -> Option<image::DynamicImage> {
    match src {
        Src::File(p) => open_image(p),
        Src::Dir(d) => open_image(&folder_image(d)?),
        Src::Url(u, name) => {
            let f = cache.join(name);
            if !f.exists() && !fetch(u, &f) {
                return None;
            }
            let img = open_image(&f);
            if img.is_none() {
                let _ = fs::remove_file(&f);
            }
            img
        }
    }
}

/// Ratio vertical (~2:3) : on remplit la carte. Autre ratio (bannière, icône) : on l'ajuste au centre sur fond sombre.
fn fit(img: image::DynamicImage, crop: bool) -> image::RgbaImage {
    let f = image::imageops::FilterType::Triangle;
    let r = img.width() as f32 / img.height().max(1) as f32;
    if crop || (0.55..=0.8).contains(&r) {
        return img.resize_to_fill(TEX_W, TEX_H, f).to_rgba8();
    }
    let h = img.resize(TEX_W, TEX_H, f).to_rgba8();
    let mut c = image::RgbaImage::from_pixel(TEX_W, TEX_H, image::Rgba([20, 22, 28, 255]));
    image::imageops::overlay(&mut c, &h, ((TEX_W - h.width()) / 2) as i64, ((TEX_H - h.height()) / 2) as i64);
    c
}

type Job = (String, Vec<Src>);
/// Pochettes à la demande : une carte qui devient visible dépose sa demande, 4 threads se partagent la file
/// (le téléchargement est le point lent). Rien n'est chargé pour les jeux qu'on ne regarde jamais.
fn spawn_loader(ctx: egui::Context) -> (mpsc::Sender<Job>, mpsc::Receiver<(String, ColorImage)>) {
    let cache = app_dir("LOCALAPPDATA").join("covers");
    let _ = fs::create_dir_all(&cache);
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (tx, rx) = mpsc::channel();
    for _ in 0..4 {
        let (tx, ctx, cache, job_rx) = (tx.clone(), ctx.clone(), cache.clone(), job_rx.clone());
        std::thread::spawn(move || loop {
            let job = job_rx.lock().unwrap().recv();
            let Ok((id, covers)) = job else { return };
            let Some((img, crop)) = covers.iter().find_map(|s| resolve(s, &cache).map(|i| (i, matches!(s, Src::Url(_, n) if n.contains("hero")) || matches!(s, Src::File(p) if p.to_string_lossy().contains("hero"))))) else { continue };
            let t = fit(img, crop);
            let ci = ColorImage::from_rgba_unmultiplied([TEX_W as usize, TEX_H as usize], t.as_raw());
            if tx.send((id, ci)).is_err() {
                return;
            }
            ctx.request_repaint();
        });
    }
    (job_tx, rx)
}

// ---------- interface ----------

struct App {
    games: Vec<Game>,
    tex: HashMap<String, TextureHandle>,
    requested: HashSet<String>, // pochettes déjà demandées au chargeur
    jobs: mpsc::Sender<Job>,
    rx: mpsc::Receiver<(String, ColorImage)>,
    owned: Option<HashSet<u32>>,
    owned_rx: mpsc::Receiver<HashSet<u32>>,
    scan_rx: Option<mpsc::Receiver<(Vec<Game>, Option<HashSet<u32>>)>>, // rescan en cours (arrière-plan)
    last_scan: f64,
    was_focused: bool,
    pick_tx: mpsc::Sender<(Pick, String)>,
    pick_rx: mpsc::Receiver<(Pick, String)>,
    query: String,
    focus_search: bool,
    sel: Option<usize>, // carte choisie au clavier (rang dans la liste affichée)
    sel_moved: bool,
    filter: Filter,
    st: State,
    toast: Option<(String, f64)>, // message bref en bas de fenêtre, heure de fin
    persona: String,
    avatar: Option<TextureHandle>,
    colls: Vec<String>,         // noms des collections Steam (réglage du Profil)
    diag: Vec<(bool, String)>,  // ce qui a été trouvé sur le PC (Profil, premier lancement)
}

fn apply_theme(ctx: &egui::Context, accent: Color32) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(0x0e, 0x10, 0x14);
    v.window_fill = Color32::from_rgb(0x16, 0x19, 0x1f);
    v.extreme_bg_color = Color32::from_rgb(0x16, 0x19, 0x1f);
    v.selection.bg_fill = accent.gamma_multiply(0.45);
    v.widgets.hovered.bg_stroke.color = accent;
    ctx.set_visuals(v);
}

fn draw_star(p: &egui::Painter, c: egui::Pos2, r: f32, filled: bool, col: Color32) {
    use std::f32::consts::PI;
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let a = -PI / 2.0 + i as f32 * PI / 5.0;
            c + Vec2::new(a.cos(), a.sin()) * if i % 2 == 0 { r } else { r * 0.45 }
        })
        .collect();
    if filled {
        for i in 0..10 {
            p.add(egui::Shape::convex_polygon(vec![c, pts[i], pts[(i + 1) % 10]], col, egui::Stroke::NONE));
        }
    }
    p.add(egui::Shape::closed_line(pts, egui::Stroke::new(1.4, col)));
}

/// Bouton rafraîchir : flèche circulaire dessinée (la police système n'a pas le symbole ↻).
fn draw_refresh(p: &egui::Painter, r: egui::Rect, hover: bool, accent: Color32) {
    use std::f32::consts::PI;
    p.rect_filled(r, 6.0, if hover { Color32::from_white_alpha(22) } else { Color32::from_white_alpha(8) });
    let col = if hover { accent } else { Color32::from_white_alpha(200) };
    let (c, rad) = (r.center(), 7.0);
    let at = |a: f32| c + Vec2::new(a.cos(), a.sin()) * rad;
    let pts: Vec<egui::Pos2> = (0..=24).map(|i| at(-PI / 3.0 + i as f32 * (1.5 * PI) / 24.0)).collect();
    let end = *pts.last().unwrap();
    p.add(egui::Shape::line(pts, egui::Stroke::new(1.8, col)));
    // pointe de flèche à l'extrémité de l'arc (tangente dans le sens du tracé)
    let a = -PI / 3.0 + 1.5 * PI;
    let tan = Vec2::new(-a.sin(), a.cos());
    let n = Vec2::new(a.cos(), a.sin());
    p.add(egui::Shape::convex_polygon(vec![end + tan * 5.0, end + n * 3.5, end - n * 3.5], col, egui::Stroke::NONE));
}

fn draw_download(p: &egui::Painter, c: egui::Pos2, col: Color32) {
    p.circle_filled(c, 24.0, Color32::from_black_alpha(190));
    p.circle_stroke(c, 24.0, egui::Stroke::new(2.0, col));
    let s = egui::Stroke::new(3.0, col);
    p.line_segment([c + Vec2::new(0.0, -10.0), c + Vec2::new(0.0, 7.0)], s);
    p.line_segment([c + Vec2::new(-8.0, 0.0), c + Vec2::new(0.0, 8.0)], s);
    p.line_segment([c + Vec2::new(8.0, 0.0), c + Vec2::new(0.0, 8.0)], s);
    p.line_segment([c + Vec2::new(-9.0, 13.0), c + Vec2::new(9.0, 13.0)], s);
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Police système Segoe UI si présente (aucun poids ajouté au binaire)
        if let Ok(bytes) = fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert("segoe".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "segoe".into());
            cc.egui_ctx.set_fonts(fonts);
        }
        let mut st = load_state();
        st.scope = st.scope.min(5);
        st.sort = st.sort.min(2);
        st.on_launch = st.on_launch.min(2);
        apply_theme(&cc.egui_ctx, ACCENTS[st.accent.min(ACCENTS.len() - 1)]);
        let owned = collection_owned(&st.collection).or_else(load_owned);
        let games = scan_all(&owned, &st.custom, &st.collection);
        let (jobs, rx) = spawn_loader(cc.egui_ctx.clone());
        let owned_rx = spawn_owned(cc.egui_ctx.clone(), &st.collection);
        let (pick_tx, pick_rx) = mpsc::channel();
        let (persona, avatar) = steam_profile();
        let avatar = avatar.and_then(|p| open_image(&p)).map(|i| {
            let i = i.to_rgba8();
            cc.egui_ctx.load_texture("avatar", ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], i.as_raw()), egui::TextureOptions::LINEAR)
        });
        let mut app = App {
            games,
            tex: HashMap::new(),
            requested: HashSet::new(),
            jobs,
            rx,
            owned,
            owned_rx,
            scan_rx: None,
            last_scan: 0.0,
            was_focused: true,
            pick_tx,
            pick_rx,
            query: String::new(),
            focus_search: false,
            sel: None,
            sel_moved: false,
            filter: Filter::All,
            st,
            toast: None,
            persona,
            avatar,
            colls: vec![],
            diag: vec![],
        };
        app.refresh_diag();
        app
    }

    /// Relit ce qui dépend de l'installation (collections, Steam/Epic trouvés) : au démarrage et après chaque rescan.
    fn refresh_diag(&mut self) {
        self.colls = collections().into_iter().map(|c| c.0).collect();
        self.colls.sort();
        self.colls.dedup();
        self.diag = setup_lines();
    }

    /// Rescan dans un thread (≈300 ms de lecture disque) : la fenêtre ne se fige pas.
    fn rescan(&mut self, ctx: &egui::Context) {
        if self.scan_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (coll, custom, ctx) = (self.st.collection.clone(), self.st.custom.clone(), ctx.clone());
        std::thread::spawn(move || {
            let owned = collection_owned(&coll).or_else(load_owned); // load_owned : liste du profil public, gardée en cache
            let _ = tx.send((scan_all(&owned, &custom, &coll), owned));
            ctx.request_repaint();
        });
        self.scan_rx = Some(rx);
    }

    fn toast(&mut self, ctx: &egui::Context, text: String) {
        self.toast = Some((text, ctx.input(|i| i.time) + 3.0));
    }

    /// Exécute une action sur le jeu `i` (clic, menu du clic droit, Entrée).
    fn act(&mut self, ctx: &egui::Context, i: usize, a: Act) {
        let g = self.games[i].clone();
        let steam_id = g.id.trim_start_matches("steam:");
        match a {
            Act::Play => {
                self.toast(ctx, format!("{} {}…", if g.installed { "Lancement de" } else { "Installation demandée :" }, g.name));
                open_url(if g.installed { &g.launch_url } else { &g.install_url });
                if g.installed {
                    self.st.last.insert(g.id, now());
                    match self.st.on_launch {
                        1 => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                        2 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        _ => {}
                    }
                }
            }
            Act::Fav => {
                if !self.st.favs.remove(&g.id) {
                    self.st.favs.insert(g.id);
                }
            }
            Act::Hide => {
                if !self.st.hidden.remove(&g.id) {
                    self.st.hidden.insert(g.id);
                    self.toast(ctx, format!("{} masqué (filtre « Masqués » pour le retrouver)", g.name));
                }
            }
            Act::Folder => {
                if let Some(d) = &g.dir {
                    let _ = Command::new("explorer.exe").arg(d).spawn();
                }
            }
            Act::StorePage => match g.store {
                Store::Steam => open_url(&format!("steam://store/{steam_id}")),
                _ => {
                    let q: String = g.name.chars().map(|c| if c.is_alphanumeric() { c.to_string() } else { "%20".into() }).collect();
                    open_url(&format!("https://store.epicgames.com/fr/browse?q={q}"));
                }
            },
            Act::Uninstall => open_url(&format!("steam://uninstall/{steam_id}")), // Steam demande lui-même confirmation
            Act::Cover => pick_file("Images|*.jpg;*.jpeg;*.png", Pick::Cover(g.id), self.pick_tx.clone(), ctx.clone()),
            Act::CoverReset => {
                let _ = fs::remove_file(custom_cover(&g.id));
                self.tex.remove(&g.id);
                self.requested.remove(&g.id);
            }
        }
        save_state(&self.st);
    }

    fn card(&mut self, ui: &mut egui::Ui, i: usize, w: f32, accent: Color32, selected: bool) {
        let g = &self.games[i];
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, w * 1.5), Sense::click());
        if selected && self.sel_moved {
            ui.scroll_to_rect(rect.expand(20.0), None);
            self.sel_moved = false;
        }
        if !ui.is_rect_visible(rect) {
            return; // hors de l'écran : rien à dessiner, rien à charger
        }
        let fav = self.st.favs.contains(&g.id);
        let p = ui.painter_at(rect);
        let r = 10.0;
        match self.tex.get(&g.id) {
            Some(t) => {
                egui::Image::new(t).corner_radius(r).paint_at(ui, rect);
            }
            None => {
                if self.requested.insert(g.id.clone()) {
                    let mut srcs = vec![Src::File(custom_cover(&g.id))];
                    srcs.extend(g.covers.iter().cloned());
                    let _ = self.jobs.send((g.id.clone(), srcs));
                }
                let h = g.name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
                let c = Color32::from_rgb(40 + (h % 40) as u8, 44 + ((h >> 8) % 40) as u8, 60 + ((h >> 16) % 50) as u8);
                p.rect_filled(rect, r, c);
                let initial = g.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                p.text(rect.center() - Vec2::new(0.0, w * 0.12), egui::Align2::CENTER_CENTER, initial, FontId::proportional(w * 0.4), Color32::from_white_alpha(40));
            }
        }
        if !g.installed {
            p.rect_filled(rect, r, Color32::from_black_alpha(if resp.hovered() { 90 } else { 120 }));
            if resp.hovered() {
                draw_download(&p, rect.center() - Vec2::new(0.0, 20.0), accent);
            }
        }
        // bandeau titre : 2 lignes au plus (coupé par « … »), le bandeau s'agrandit pour ne pas recouvrir l'étiquette
        let mut job = egui::text::LayoutJob::simple(g.name.clone(), FontId::proportional(13.0), Color32::WHITE, w - 16.0);
        job.wrap.max_rows = 2;
        let galley = p.layout_job(job);
        let band = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - galley.size().y - 30.0), rect.max);
        p.rect_filled(band, egui::CornerRadius { nw: 0, ne: 0, sw: r as u8, se: r as u8 }, Color32::from_black_alpha(190));
        p.galley(band.min + Vec2::new(8.0, 7.0), galley, Color32::WHITE);
        let mut tag = match g.store {
            Store::Steam => "STEAM",
            Store::Epic => "EPIC",
            Store::Other => "AUTRE",
        }
        .to_string();
        if g.family {
            tag += " · FAMILLE";
        }
        if g.update {
            tag += " · MISE À JOUR";
        }
        if g.playtime > 0 {
            tag += &format!(" · {}", fmt_time(g.playtime));
        }
        p.text(egui::pos2(rect.min.x + 8.0, rect.max.y - 10.0), egui::Align2::LEFT_CENTER, tag, FontId::proportional(10.0), accent);
        // étoile favori
        let star_rect = egui::Rect::from_center_size(rect.right_top() + Vec2::new(-18.0, 18.0), Vec2::splat(26.0));
        let star = ui.interact(star_rect, ui.next_auto_id().with(("star", &g.id)), Sense::click());
        if fav || resp.hovered() {
            p.circle_filled(star_rect.center(), 13.0, Color32::from_black_alpha(150));
            draw_star(&p, star_rect.center(), 8.0, fav, if fav { accent } else { Color32::from_white_alpha(190) });
        }
        let stroke = if selected {
            egui::Stroke::new(3.0, Color32::WHITE)
        } else if resp.hovered() {
            egui::Stroke::new(2.0, accent)
        } else {
            egui::Stroke::new(1.0, Color32::from_white_alpha(14))
        };
        p.rect_stroke(rect, r, stroke, egui::StrokeKind::Inside);
        let verb = if g.installed { "Lancer" } else { "Installer" };
        let mut tip = match (g.installed, g.update) {
            (false, _) => "Installer (ouvre Steam / Epic)".to_string(),
            (true, true) => "Lancer (Steam fera d'abord la mise à jour)".to_string(),
            (true, false) => "Lancer".to_string(),
        };
        let last = g.last_played.max(*self.st.last.get(&g.id).unwrap_or(&0));
        if last > 0 {
            tip += &format!("\nDernier lancement : {}", fmt_ago(last));
        }
        if g.size > 0 {
            tip += &format!("\n{} Go sur le disque", format!("{:.1}", g.size as f64 / 1e9).replace('.', ","));
        }
        tip += "\nClic droit : plus d'actions";
        resp.clone().on_hover_text(tip);

        let mut act = None;
        let hidden = self.st.hidden.contains(&g.id);
        resp.context_menu(|ui| {
            let has_custom = custom_cover(&g.id).exists(); // seulement quand le menu est ouvert
            let mut item = |ui: &mut egui::Ui, label: &str, a: Act| {
                if ui.button(label).clicked() {
                    act = Some(a);
                    ui.close();
                }
            };
            item(ui, verb, Act::Play);
            item(ui, if fav { "Retirer des favoris" } else { "Ajouter aux favoris" }, Act::Fav);
            ui.separator();
            if g.dir.is_some() {
                item(ui, "Ouvrir le dossier du jeu", Act::Folder);
            }
            if g.store != Store::Other {
                item(ui, "Page du magasin", Act::StorePage);
            }
            item(ui, "Changer la pochette…", Act::Cover);
            if has_custom {
                item(ui, "Remettre la pochette d'origine", Act::CoverReset);
            }
            ui.separator();
            item(ui, if hidden { "Réafficher" } else { "Masquer ce jeu" }, Act::Hide);
            if g.installed && g.store == Store::Steam {
                item(ui, "Désinstaller…", Act::Uninstall);
            }
        });
        if star.clicked() {
            act = Some(Act::Fav);
        } else if resp.clicked() {
            act = Some(Act::Play);
        }
        if let Some(a) = act {
            let ctx = ui.ctx().clone();
            self.act(&ctx, i, a);
        }
    }

    /// Onglet Profil : chiffres tirés des données locales de Steam, puis les réglages.
    fn profile(&mut self, ui: &mut egui::Ui, accent: Color32, rescan: &mut bool) {
        let gray = Color32::from_gray(150);
        let last = |g: &Game| g.last_played.max(*self.st.last.get(&g.id).unwrap_or(&0));
        let total: u32 = self.games.iter().map(|g| g.playtime).sum();
        let fam = self.games.iter().filter(|g| g.family).count();
        let installed = self.games.iter().filter(|g| g.installed).count();
        let disk: u64 = self.games.iter().map(|g| g.size).sum();
        let steam_mine = self.games.iter().filter(|g| g.store == Store::Steam && !g.family).count();
        let never = self.games.iter().filter(|g| g.store == Store::Steam && !g.family && g.playtime == 0 && last(g) == 0).count();
        let mut top: Vec<(String, u32)> = self.games.iter().filter(|g| g.playtime > 0).map(|g| (g.name.clone(), g.playtime)).collect();
        top.sort_by_key(|t| std::cmp::Reverse(t.1));
        let played = top.len();
        top.truncate(5);
        let mut recent: Vec<(String, u64)> = self.games.iter().map(|g| (g.name.clone(), last(g))).filter(|r| r.1 > 0).collect();
        recent.sort_by_key(|r| std::cmp::Reverse(r.1));
        recent.truncate(5);
        let heavy = self.games.iter().max_by_key(|g| g.size).filter(|g| g.size > 0).map(|g| (g.name.clone(), g.size));
        let tiles = [
            (fmt_time(total), "de jeu sur Steam".to_string()),
            (format!("{:.1}", total as f32 / 1440.0).replace('.', ","), "jours complets, nuits comprises".to_string()),
            ((self.games.len() - fam).to_string(), "jeux à moi".to_string()),
            (fam.to_string(), "jeux prêtés par la famille".to_string()),
            (installed.to_string(), format!("installés · {:.0} Go", disk as f64 / 1e9)),
            (never.to_string(), format!("jamais lancés sur {steam_mine} jeux Steam")),
            (self.st.favs.len().to_string(), "favoris".to_string()),
        ];
        let title = |ui: &mut egui::Ui, t: &str| {
            ui.add_space(18.0);
            ui.label(RichText::new(t).size(17.0).strong().color(accent));
            ui.add_space(4.0);
        };

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(a) = &self.avatar {
                    ui.add(egui::Image::new(a).fit_to_exact_size(Vec2::splat(72.0)).corner_radius(36.0));
                    ui.add_space(8.0);
                }
                ui.vertical(|ui| {
                    ui.add_space(10.0);
                    ui.label(RichText::new(if self.persona.is_empty() { "Mon profil" } else { &self.persona }).size(26.0).strong());
                    ui.label(RichText::new(format!("{} jeux dans le lanceur · {played} déjà lancés sur Steam", self.games.len())).color(gray));
                });
            });
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(10.0);
                for (big, small) in &tiles {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(200.0, 86.0), Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(r, 10.0, Color32::from_rgb(0x16, 0x19, 0x1f));
                    p.text(r.min + Vec2::new(14.0, 10.0), egui::Align2::LEFT_TOP, big, FontId::proportional(28.0), Color32::WHITE);
                    p.text(r.min + Vec2::new(14.0, 56.0), egui::Align2::LEFT_TOP, small, FontId::proportional(12.5), gray);
                }
            });

            if let Some(max) = top.first().map(|t| t.1) {
                title(ui, "Les plus joués");
                for (i, (name, min)) in top.iter().enumerate() {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width().min(760.0), 26.0), Sense::hover());
                    let p = ui.painter();
                    p.text(r.left_center(), egui::Align2::LEFT_CENTER, format!("{}. {name}", i + 1), FontId::proportional(14.0), Color32::WHITE);
                    let bar = egui::Rect::from_min_max(egui::pos2(r.min.x + 250.0, r.min.y + 7.0), egui::pos2(r.max.x - 80.0, r.max.y - 7.0));
                    p.rect_filled(bar, 6.0, Color32::from_white_alpha(10));
                    let mut fill = bar;
                    fill.max.x = bar.min.x + (bar.width() * *min as f32 / max as f32).max(6.0);
                    p.rect_filled(fill, 6.0, accent);
                    p.text(r.right_center(), egui::Align2::RIGHT_CENTER, fmt_time(*min), FontId::proportional(14.0), gray);
                }
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{} pèse {} % de tout ton temps de jeu.", top[0].0, top[0].1 as u64 * 100 / total.max(1) as u64)).color(gray));
            }
            if !recent.is_empty() {
                title(ui, "Derniers lancés");
                for (name, t) in &recent {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).size(14.0));
                        ui.label(RichText::new(format!("· {}", fmt_ago(*t))).color(gray));
                    });
                }
            }
            title(ui, "En vrac");
            if let Some((name, size)) = &heavy {
                ui.label(format!("Le plus lourd sur le disque : {name}, {:.0} Go.", *size as f64 / 1e9));
            }
            if never > 0 {
                ui.label(format!("{never} jeux Steam à toi n'ont jamais été lancés sur ce PC."));
            }
            ui.label(RichText::new("Epic ne garde pas le temps de jeu sur le PC : seuls les jeux Steam sont comptés.").color(gray));

            title(ui, "Réglages");
            ui.horizontal(|ui| {
                ui.label("Quand je lance un jeu :");
                let mut ch = false;
                for (i, l) in ["rester ouvert", "se réduire", "se fermer"].into_iter().enumerate() {
                    ch |= ui.radio_value(&mut self.st.on_launch, i, l).changed();
                }
                if ch {
                    save_state(&self.st);
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("Jeux masqués : {}", self.st.hidden.len()));
                if !self.st.hidden.is_empty() && ui.button("Tout réafficher").clicked() {
                    self.st.hidden.clear();
                    save_state(&self.st);
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Ajouter un jeu…").on_hover_text("Un .exe ou un raccourci : pour les jeux hors Steam et Epic").clicked() {
                    pick_file("Jeux et raccourcis|*.exe;*.lnk;*.url", Pick::AddGame, self.pick_tx.clone(), ui.ctx().clone());
                }
                ui.label(RichText::new("Roblox et Minecraft sont trouvés tout seuls.").color(gray));
            });
            let mut remove = None;
            for (i, (name, path)) in self.st.custom.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("Retirer").clicked() {
                        remove = Some(i);
                    }
                    ui.label(name).on_hover_text(path);
                });
            }
            if let Some(i) = remove {
                self.st.custom.remove(i);
                save_state(&self.st);
                *rescan = true;
            }

            title(ui, "Mes jeux et famille Steam");
            ui.horizontal(|ui| {
                ui.label("Collection Steam qui contient mes jeux :");
                let cur = match self.st.collection.as_str() {
                    "" => "Mes jeux",
                    "-" => "Aucune",
                    n => n,
                }
                .to_string();
                let mut ch = false;
                egui::ComboBox::from_id_salt("coll").selected_text(cur).show_ui(ui, |ui| {
                    ch |= ui.selectable_value(&mut self.st.collection, "-".to_string(), "Aucune").changed();
                    for n in &self.colls {
                        ch |= ui.selectable_value(&mut self.st.collection, n.clone(), n).changed();
                    }
                });
                if ch {
                    save_state(&self.st);
                    *rescan = true;
                }
            });
            ui.label(RichText::new(match &self.owned {
                Some(o) => format!("{} jeux comptés comme à moi ; le reste de la bibliothèque Steam va dans « Famille Steam ».", o.len()),
                None => "Aucune collection trouvée : tous tes jeux Steam sont dans « Steam ». Pour séparer ceux de ta famille, range les tiens dans une collection Steam, puis choisis-la ici.".to_string(),
            })
            .color(gray));

            title(ui, "Diagnostic");
            for (ok, t) in &self.diag {
                ui.label(RichText::new(format!("{}  {t}", if *ok { "OK " } else { "Non" })).color(if *ok { Color32::from_rgb(0x4a, 0xde, 0x80) } else { accent }));
            }
            ui.label(RichText::new("Epic : la liste « à installer » vient du cache du lanceur Epic, pas de ta vraie bibliothèque : elle peut contenir des jeux que tu n'as pas.").color(gray));
            ui.add_space(10.0);
            ui.label(RichText::new("Clavier : flèches pour choisir, Entrée pour lancer, tape du texte pour chercher, Échap pour effacer, F5 pour rescanner.").color(gray));
            ui.label(RichText::new(format!("Lanceur v{}", env!("CARGO_PKG_VERSION"))).color(gray));
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        while let Ok((id, ci)) = self.rx.try_recv() {
            self.tex.insert(id.clone(), ctx.load_texture(id, ci, egui::TextureOptions::LINEAR));
        }
        if let Ok(set) = self.owned_rx.try_recv() {
            self.owned = Some(set);
            apply_family(&mut self.games, &self.owned);
        }
        // résultat d'un rescan fait en arrière-plan : on garde les pochettes déjà chargées (pas de clignotement)
        let scanned = self.scan_rx.as_ref().and_then(|r| r.try_recv().ok());
        if let Some((games, owned)) = scanned {
            self.games = games;
            self.owned = owned;
            self.refresh_diag();
            self.scan_rx = None;
            self.tex.retain(|id, _| self.games.iter().any(|g| &g.id == id));
            self.requested.retain(|id| self.tex.contains_key(id));
        }
        let accent = ACCENTS[self.st.accent.min(ACCENTS.len() - 1)];
        let (time, focused) = ctx.input(|i| (i.time, i.focused));
        let mut rescan = ctx.input(|i| i.key_pressed(egui::Key::F5));
        // retour sur la fenêtre (après une installation, un jeu…) : on rescanne tout seul, au plus toutes les 10 s
        rescan |= focused && !self.was_focused && time - self.last_scan > 10.0;
        self.was_focused = focused;
        let mut dirty = false;
        // fichier choisi dans la boîte « Ouvrir » (pochette personnalisée, jeu ajouté à la main)
        while let Ok((what, path)) = self.pick_rx.try_recv() {
            match what {
                Pick::Cover(id) => {
                    if fs::copy(&path, custom_cover(&id)).is_ok() {
                        self.tex.remove(&id);
                        self.requested.remove(&id);
                    }
                }
                Pick::AddGame => {
                    if !self.st.custom.iter().any(|c| c.1 == path) {
                        let name = Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        self.st.custom.push((name, path));
                        dirty = true;
                        rescan = true;
                    }
                }
            }
        }
        // taille de la fenêtre : retenue en continu, écrite à la fermeture
        let (inner, maxi, mini, closing) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().maximized.unwrap_or(false), i.viewport().minimized.unwrap_or(false), i.viewport().close_requested()));
        if !mini {
            self.st.maximized = maxi;
            if let Some(r) = inner.filter(|r| !maxi && r.width() > 300.0) {
                self.st.win = Some([r.width(), r.height()]);
            }
        }
        dirty |= closing;
        let (mut search_focused, mut search_enter) = (false, false);

        egui::Panel::top("top").frame(egui::Frame::new().fill(Color32::from_rgb(0x12, 0x14, 0x19)).inner_margin(12.0)).show(root, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Lanceur").size(21.0).strong().color(accent));
                ui.add_space(10.0);
                let search = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Rechercher… (tape directement)").desired_width(200.0));
                if std::mem::take(&mut self.focus_search) {
                    search.request_focus();
                }
                if search.changed() {
                    self.sel = None;
                }
                search_focused = search.has_focus();
                search_enter = search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                    draw_refresh(ui.painter(), r, resp.hovered(), accent);
                    if resp.on_hover_text("Rescanner les bibliothèques (F5)").clicked() {
                        rescan = true;
                    }
                    for i in (0..ACCENTS.len()).rev() {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                        ui.painter().circle_filled(r.center(), 7.0, ACCENTS[i]);
                        if i == self.st.accent {
                            ui.painter().circle_stroke(r.center(), 9.0, egui::Stroke::new(1.5, Color32::WHITE));
                        }
                        if resp.clicked() {
                            self.st.accent = i;
                            apply_theme(ctx, ACCENTS[i]);
                            dirty = true;
                        }
                    }
                    dirty |= ui.add(egui::Slider::new(&mut self.st.card_w, 120.0..=260.0).show_value(false)).on_hover_text("Taille des pochettes").changed();
                    let sorts = ["Nom", "Récents", "Temps de jeu"];
                    egui::ComboBox::from_id_salt("tri").selected_text(format!("Tri : {}", sorts[self.st.sort])).show_ui(ui, |ui| {
                        for (i, s) in sorts.into_iter().enumerate() {
                            dirty |= ui.selectable_value(&mut self.st.sort, i, s).changed();
                        }
                    });
                });
            });
            ui.add_space(8.0);
            // 2e ligne : à qui sont les jeux (onglets) + profil, puis les filtres à droite
            ui.horizontal(|ui| {
                let mut count = [0usize; 6];
                for g in self.games.iter().filter(|g| !self.st.hidden.contains(&g.id)) {
                    count[g.group()] += 1;
                    count[2] += 1;
                }
                let optional = |i: usize| matches!(i, 1 | 4 | 5); // Famille, Epic, Autres : cachés quand ils n'ont aucun jeu
                if optional(self.st.scope) && count[self.st.scope] == 0 {
                    self.st.scope = 0; // onglet vidé (plus aucun jeu) : retour au premier
                }
                // ordre d'affichage ≠ numéros : les numéros 0 à 3 sont ceux déjà enregistrés dans state.json
                for (i, name) in [(0, "Steam"), (1, "Famille Steam"), (4, "Epic"), (5, "Autres"), (2, "Tout"), (3, "Profil")] {
                    if optional(i) && count[i] == 0 {
                        continue;
                    }
                    let label = if i == 3 { name.to_string() } else { format!("{name}   {}", count[i]) };
                    if ui.selectable_label(self.st.scope == i, RichText::new(label).size(15.0).strong()).clicked() {
                        self.st.scope = i;
                        self.sel = None;
                        dirty = true;
                    }
                }
                if self.st.scope == 3 {
                    return;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let mut filters = vec![("Tous", Filter::All), ("Installés", Filter::Installed), ("Favoris", Filter::Fav)];
                    if self.st.hidden.is_empty() && self.filter == Filter::Hidden {
                        self.filter = Filter::All;
                    } else if !self.st.hidden.is_empty() {
                        filters.push(("Masqués", Filter::Hidden));
                    }
                    for (label, f) in filters.into_iter().rev() {
                        if ui.selectable_label(self.filter == f, label).clicked() {
                            self.filter = f;
                            self.sel = None;
                        }
                    }
                });
            });
        });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::from_rgb(0x0e, 0x10, 0x14)).inner_margin(16.0)).show(root, |ui| {
            let full = ui.max_rect();
            if let Some((text, until)) = &self.toast {
                if ui.input(|i| i.time) < *until {
                    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("toast")));
                    let galley = p.layout_no_wrap(text.clone(), FontId::proportional(14.0), Color32::WHITE);
                    let r = egui::Rect::from_center_size(egui::pos2(full.center().x, full.max.y - 34.0), galley.size() + Vec2::new(28.0, 16.0));
                    p.rect_filled(r, 8.0, Color32::from_rgb(0x1c, 0x20, 0x28));
                    p.rect_stroke(r, 8.0, egui::Stroke::new(1.0, accent), egui::StrokeKind::Inside);
                    p.galley(r.min + Vec2::new(14.0, 8.0), galley, Color32::WHITE);
                    ctx.request_repaint_after(std::time::Duration::from_millis(300));
                } else {
                    self.toast = None;
                }
            }
            if self.st.scope == 3 {
                self.profile(ui, accent, &mut rescan);
                return;
            }
            if self.games.is_empty() {
                // premier lancement sur un PC sans rien de reconnu : on dit ce qui a été cherché
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    ui.label(RichText::new("Aucun jeu trouvé sur ce PC").size(24.0).strong());
                    ui.add_space(12.0);
                    for (ok, t) in &self.diag {
                        ui.label(RichText::new(format!("{}  {t}", if *ok { "OK" } else { "Non" })).size(15.0).color(if *ok { Color32::from_rgb(0x4a, 0xde, 0x80) } else { accent }));
                    }
                    ui.add_space(12.0);
                    ui.label(RichText::new("Installe Steam ou Epic Games, lance-les une fois, puis appuie sur F5.\nOu ajoute un jeu à la main.").color(Color32::GRAY));
                    ui.add_space(10.0);
                    if ui.button("Ouvrir le profil").clicked() {
                        self.st.scope = 3;
                        dirty = true;
                    }
                });
                return;
            }
            if LIBRARY_UNREADABLE.load(Relaxed) && self.games.iter().any(|g| g.store == Store::Steam) {
                egui::Frame::new().fill(accent.gamma_multiply(0.22)).corner_radius(8.0).inner_margin(10.0).show(ui, |ui| {
                    ui.label("Attention : la bibliothèque complète de Steam n'a pas pu être lue (Steam a changé le format de son cache, ou n'a pas fini de démarrer). Seuls les jeux installés sont affichés. Lance Steam, attends qu'il soit prêt, puis F5.");
                });
                ui.add_space(8.0);
            }
            let q = self.query.to_lowercase();
            let scope = self.st.scope;
            let show_hidden = self.filter == Filter::Hidden;
            let mut idx: Vec<usize> = (0..self.games.len())
                .filter(|&i| {
                    let g = &self.games[i];
                    self.st.hidden.contains(&g.id) == show_hidden
                        && (scope == 2 || g.group() == scope)
                        && g.name.to_lowercase().contains(&q)
                        && match self.filter {
                            Filter::All | Filter::Hidden => true,
                            Filter::Installed => g.installed,
                            Filter::Fav => self.st.favs.contains(&g.id),
                        }
                })
                .collect();
            let last = |g: &Game| g.last_played.max(*self.st.last.get(&g.id).unwrap_or(&0));
            idx.sort_by_key(|&i| {
                let g = &self.games[i];
                let first = match self.st.sort {
                    1 => last(g),
                    2 => g.playtime as u64,
                    _ => 0,
                };
                (std::cmp::Reverse(first), g.name.to_lowercase())
            });
            if idx.is_empty() {
                ui.centered_and_justified(|ui| ui.label(RichText::new("Aucun jeu à afficher").color(Color32::GRAY).size(16.0)));
                return;
            }
            // blocs : installés puis à installer (et, dans « Tout », plateforme par plateforme)
            let mut blocks: Vec<(String, Vec<usize>)> = vec![];
            for (grp, who) in [(0, "Steam"), (1, "Famille Steam"), (4, "Epic"), (5, "Autres")] {
                for (inst, what) in [(true, "Installés"), (false, "À installer")] {
                    let list: Vec<usize> = idx.iter().copied().filter(|&i| self.games[i].group() == grp && self.games[i].installed == inst).collect();
                    if !list.is_empty() {
                        blocks.push((if scope == 2 { format!("{who} · {}", what.to_lowercase()) } else { what.to_string() }, list));
                    }
                }
            }
            let order: Vec<usize> = blocks.iter().flat_map(|b| b.1.iter().copied()).collect();
            let n = order.len();
            self.sel = self.sel.filter(|&s| s < n);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let gap = 14.0;
                ui.spacing_mut().item_spacing = Vec2::splat(gap);
                // les cartes s'élargissent pour remplir la ligne : pas de bande vide à droite
                let cols = ((ui.available_width() + gap) / (self.st.card_w + gap)).floor().max(1.0);
                let w = ((ui.available_width() - (cols - 1.0) * gap) / cols).floor();

                // clavier : flèches + Entrée ; taper du texte part dans la recherche ; Échap efface
                // ponytail: haut/bas avancent d'une ligne « à plat », sans tenir compte des titres de blocs (décalage de colonne possible entre deux blocs)
                let c = cols as usize;
                let (typed, esc, enter) = ctx.input(|i| {
                    let t: String = i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.as_str()) } else { None }).collect();
                    (t, i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Enter))
                });
                let key = |k| ctx.input(|i| i.key_pressed(k));
                let mut launch = None;
                if esc {
                    self.query.clear();
                    self.sel = None;
                } else if search_enter {
                    if !q.is_empty() {
                        launch = Some(order[self.sel.unwrap_or(0)]);
                    }
                } else if search_focused {
                    if key(egui::Key::ArrowDown) {
                        ctx.memory_mut(|m| m.focused().map(|id| m.surrender_focus(id)));
                        (self.sel, self.sel_moved) = (Some(0), true);
                    }
                } else if ctx.memory(|m| m.focused().is_none()) {
                    if !typed.trim().is_empty() {
                        self.query += &typed;
                        self.focus_search = true;
                        self.sel = None;
                        ctx.request_repaint();
                    } else if enter {
                        launch = self.sel.map(|s| order[s]);
                    } else {
                        let s = self.sel;
                        let new = if key(egui::Key::ArrowRight) {
                            Some(s.map_or(0, |s| (s + 1).min(n - 1)))
                        } else if key(egui::Key::ArrowLeft) {
                            Some(s.map_or(0, |s| s.saturating_sub(1)))
                        } else if key(egui::Key::ArrowDown) {
                            Some(s.map_or(0, |s| (s + c).min(n - 1)))
                        } else if key(egui::Key::ArrowUp) {
                            Some(s.map_or(0, |s| s.saturating_sub(c)))
                        } else {
                            None
                        };
                        if new.is_some() {
                            (self.sel, self.sel_moved) = (new, true);
                        }
                    }
                }

                let mut k = 0;
                for (title, list) in &blocks {
                    ui.label(RichText::new(format!("{title}  ·  {}", list.len())).size(16.0).strong().color(accent));
                    for row in list.chunks(c) {
                        ui.horizontal(|ui| {
                            for &i in row {
                                let selected = self.sel == Some(k);
                                self.card(ui, i, w, accent, selected);
                                k += 1;
                            }
                        });
                    }
                    ui.add_space(12.0);
                }
                if let Some(i) = launch {
                    self.act(ctx, i, Act::Play);
                }
            });
        });

        if dirty {
            save_state(&self.st);
        }
        if rescan {
            self.last_scan = time;
            self.rescan(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    // Essai sur le vrai PC (Steam, Epic, collection « Mes jeux » de Pierre) : `cargo test --release -- --ignored --nocapture`
    #[test]
    #[ignore = "dépend de l'installation Steam/Epic de ce PC"]
    fn scan_timing() {
        let t = Instant::now();
        let root = steam_root().unwrap();
        let o = steam_owned(&root, &collection_owned("").unwrap_or_default());
        println!("owned {} en {:?}", o.len(), t.elapsed());
        let t = Instant::now();
        let s = scan_steam("");
        println!("steam {} ({} installés) en {:?}", s.len(), s.iter().filter(|g| g.installed).count(), t.elapsed());
        let t = Instant::now();
        let e = scan_epic();
        println!("epic {} en {:?}", e.len(), t.elapsed());
        let titles: HashSet<_> = e.iter().map(|g| g.name.to_lowercase()).collect();
        assert_eq!(titles.len(), e.len(), "doublons Epic");
        println!("famille installés: {:?}", s.iter().filter(|g| g.family).map(|g| &g.name).collect::<Vec<_>>());
        println!("à mettre à jour: {:?}", s.iter().filter(|g| g.update).map(|g| &g.name).collect::<Vec<_>>());
        assert!(s.iter().all(|g| g.installed || !g.update), "mise à jour sur un jeu non installé");
        let c = collection_owned("").expect("collection Mes jeux");
        let mut s2 = s.clone();
        apply_family(&mut s2, &Some(c.clone()));
        println!("collection {} jeux ; famille au total {} / {}", c.len(), s2.iter().filter(|g| g.family).count(), s2.len());
        let have: HashSet<u32> = s2.iter().map(|g| g.id.trim_start_matches("steam:").parse().unwrap_or(0)).collect();
        println!("collection absente de la liste: {:?}", c.iter().filter(|i| !have.contains(i)).collect::<Vec<_>>());
        assert!(o.len() > 100 && !e.is_empty());
        println!("avec temps de jeu: {} ; total {}", s.iter().filter(|g| g.playtime > 0).count(), fmt_time(s.iter().map(|g| g.playtime).sum()));
        println!("autres: {:?}", scan_other(&[]).iter().map(|g| (&g.name, &g.launch_url)).collect::<Vec<_>>());
        println!("profil: {:?}", steam_profile());
        println!("compte: {:?} ; collections: {:?}", steam_user(&root), collections().iter().map(|c| (&c.0, c.1.len())).collect::<Vec<_>>());
        assert!(steam_user(&root).is_some_and(|u| u.0.len() == 17), "compte connecté introuvable");
        println!("lignes: {:?} ; illisible: {}", setup_lines(), LIBRARY_UNREADABLE.load(Relaxed));
        assert!(!LIBRARY_UNREADABLE.load(Relaxed));
    }

    #[test]
    fn two_accounts_take_most_recent() {
        let t = "\"users\"\n{\n\t\"76561190000000001\"\n\t{\n\t\t\"AccountName\"\t\t\"ami\"\n\t\t\"PersonaName\"\t\t\"Ami\"\n\t\t\"MostRecent\"\t\t\"0\"\n\t}\n\t\"76561190000000002\"\n\t{\n\t\t\"PersonaName\"\t\t\"Moi\"\n\t\t\"MostRecent\"\t\t\"1\"\n\t}\n}\n";
        assert_eq!(parse_users(t), Some(("76561190000000002".into(), "Moi".into())));
        assert_eq!(parse_users(&t.replace("\"MostRecent\"\t\t\"1\"", "\"MostRecent\"\t\t\"0\"")), Some(("76561190000000001".into(), "Ami".into())));
        assert_eq!(parse_users("rien"), None);
    }

    #[test]
    fn coll_settings() {
        assert_eq!(coll_name(""), Some("Mes jeux"));
        assert_eq!(coll_name("-"), None);
        assert_eq!(coll_name("Favoris"), Some("Favoris"));
        assert!(collection_owned("-").is_none());
    }

    #[test]
    fn playtime_and_format() {
        let vdf = "\"UserLocalConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"apps\"\n\t\t\t\t{\n\t\t\t\t\t\"730\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"LastPlayed\"\t\t\"1700000000\"\n\t\t\t\t\t\t\"Playtime\"\t\t\"1537\"\n\t\t\t\t\t}\n\t\t\t\t\t\"10\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"cloud\"\n\t\t\t\t\t\t{\n\t\t\t\t\t\t\t\"Playtime\"\t\t\"9\"\n\t\t\t\t\t\t}\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n\t\"apps\"\n\t{\n\t\t\"99\"\n\t\t{\n\t\t\t\"Playtime\"\t\t\"5\"\n\t\t}\n\t}\n}\n";
        let mut m = HashMap::new();
        parse_playtime(vdf, &mut m);
        assert_eq!(m.get("730"), Some(&(1537, 1700000000)));
        assert!(!m.contains_key("10"), "Playtime d'un sous-bloc pris pour celui du jeu");
        assert_eq!(fmt_time(45), "45 min");
        assert_eq!(fmt_time(90), "1,5 h");
        assert_eq!(fmt_time(1537), "26 h");
    }
}

fn main() -> eframe::Result {
    // taille retenue d'une fois sur l'autre (state.json), bornée pour ne jamais rouvrir une fenêtre minuscule
    let st = load_state();
    let [w, h] = st.win.unwrap_or([1100.0, 720.0]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([w.clamp(860.0, 6000.0), h.clamp(420.0, 4000.0)])
        .with_min_inner_size([860.0, 420.0])
        .with_maximized(st.maximized)
        .with_title("Lanceur");
    // icône de la fenêtre et de la barre des tâches (le même logo que dans l'exe)
    if let Ok(img) = image::load_from_memory(include_bytes!("../assets/icon256.png")) {
        let rgba = img.to_rgba8();
        viewport = viewport.with_icon(egui::IconData { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() });
    }
    let opts = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Lanceur", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
