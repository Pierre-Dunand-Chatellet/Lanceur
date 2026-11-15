//! Tout ce qui change d'un système à l'autre : dossiers de l'appli, où se trouve Steam, lancer un programme ou une adresse,
//! boîte de choix de fichier, fuseau horaire. Le reste du programme (lecture des fichiers de Steam, interface) est le même partout.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub const OS: &str = if cfg!(windows) {
    "Windows"
} else if cfg!(target_os = "macos") {
    "macOS"
} else {
    "Linux"
};

/// Ce qu'on peut ajouter à la main (texte du bouton « Ajouter un jeu »).
pub const PROGRAMME: &str = if cfg!(windows) {
    "Un .exe ou un raccourci"
} else if cfg!(target_os = "macos") {
    "Une application (.app) ou un programme"
} else {
    "Un programme ou un raccourci .desktop"
};
/// Phrase du Profil sur ce que Lanceur trouve sans aide (hors Steam et Epic), vide si rien.
pub const TROUVES_SEULS: &str = if cfg!(windows) {
    "Roblox et Minecraft sont trouvés tout seuls."
} else if cfg!(target_os = "macos") {
    "Roblox et Minecraft sont trouvés tout seuls s'ils sont dans Applications."
} else {
    ""
};

/// Commande sans fenêtre de console (Windows seulement : ailleurs, rien à faire).
pub fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        const NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(NO_WINDOW);
    }
    cmd
}

pub fn curl() -> &'static str {
    if cfg!(windows) {
        "curl.exe"
    } else {
        "curl"
    }
}

fn var_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

pub fn home() -> PathBuf {
    var_dir(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default()
}

/// Réglages (state.json) : %APPDATA%\Lanceur, ~/Library/Application Support/Lanceur ou ~/.config/Lanceur.
pub fn config_dir() -> PathBuf {
    if cfg!(windows) {
        var_dir("APPDATA").unwrap_or_default().join("Lanceur")
    } else if cfg!(target_os = "macos") {
        home().join("Library/Application Support/Lanceur")
    } else {
        var_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config")).join("Lanceur")
    }
}

/// Pochettes téléchargées et listes en cache : %LOCALAPPDATA%\Lanceur, ~/Library/Caches/Lanceur ou ~/.cache/Lanceur.
pub fn cache_dir() -> PathBuf {
    if cfg!(windows) {
        var_dir("LOCALAPPDATA").unwrap_or_default().join("Lanceur")
    } else if cfg!(target_os = "macos") {
        home().join("Library/Caches/Lanceur")
    } else {
        var_dir("XDG_CACHE_HOME").unwrap_or_else(|| home().join(".cache")).join("Lanceur")
    }
}

/// Chemin écrit comme le système le veut (Windows : le registre donne `c:/program files/steam`, l'Explorateur veut des antislashs).
pub fn native_path(p: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(p.to_string_lossy().replace('/', "\\"))
    } else {
        p.to_path_buf()
    }
}

/// Clé de comparaison de deux chemins (même dossier écrit de deux façons).
pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        s.trim_end_matches(['\\', '/']).to_lowercase()
    } else {
        s.trim_end_matches('/').to_string()
    }
}

/// Dossier de Steam : celui du registre (Windows), ou le premier emplacement habituel qui existe.
pub fn steam_default() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Valve\\Steam")
            .ok()
            .and_then(|k| k.get_value::<String, _>("SteamPath").ok())
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(r"C:\Program Files (x86)\Steam")))
            .filter(|p| p.exists())
    }
    #[cfg(not(windows))]
    {
        let h = home();
        let candidats: Vec<PathBuf> = if cfg!(target_os = "macos") {
            vec![h.join("Library/Application Support/Steam")]
        } else {
            // ~/.steam/steam est un lien vers le vrai dossier ; Flatpak et Snap ont chacun le leur
            vec![
                h.join(".steam/steam"),
                h.join(".local/share/Steam"),
                h.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
                h.join("snap/steam/common/.local/share/Steam"),
            ]
        };
        candidats.into_iter().find(|c| c.join("steamapps").is_dir() || c.join("config").is_dir())
    }
}

/// Où le lanceur Epic range ses données (manifestes, catalogue) : les emplacements à essayer, dans l'ordre.
/// Windows : ProgramData. Mac : emplacements relevés dans des forums, non vérifiés ici. Linux : Epic n'y a pas de lanceur.
pub fn epic_candidates() -> Vec<PathBuf> {
    if cfg!(windows) {
        let pd = var_dir("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        vec![pd.join(r"Epic\EpicGamesLauncher\Data")]
    } else if cfg!(target_os = "macos") {
        let a = home().join("Library/Application Support/Epic");
        vec![a.join("EpicGamesLauncher/Data"), a.join("Epic Games Launcher/Data"), PathBuf::from("/Users/Shared/Epic Games/Launcher/Data")]
    } else {
        vec![]
    }
}

/// Décalage de l'heure locale par rapport à UTC, en secondes (pour « hier » / « aujourd'hui »).
pub fn local_offset() -> i64 {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct Tzi {
            bias: i32,
            _std_name: [u16; 32],
            _std_date: [u16; 8],
            std_bias: i32,
            _dl_name: [u16; 32],
            _dl_date: [u16; 8],
            dl_bias: i32,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetTimeZoneInformation(tzi: *mut Tzi) -> u32;
        }
        let mut t = std::mem::MaybeUninit::<Tzi>::zeroed();
        // SAFETY : la structure a la disposition de TIME_ZONE_INFORMATION (172 octets) ; Windows la remplit, et tout-à-zéro est valide.
        let (r, t) = unsafe { (GetTimeZoneInformation(t.as_mut_ptr()), t.assume_init()) };
        let saison = match r {
            1 => t.std_bias,
            2 => t.dl_bias,
            _ => 0,
        };
        -60 * (t.bias + saison) as i64
    }
    #[cfg(not(windows))]
    {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as libc::time_t).unwrap_or(0);
        // SAFETY : `tm` est une structure C de nombres entiers, valide une fois remplie à zéro ; localtime_r écrit dedans sans garder de pointeur.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return 0;
            }
            tm.tm_gmtoff as i64
        }
    }
}

/// Ouvre une adresse (steam://…), un dossier, un document ou un programme avec ce que le système prévoit.
pub fn open(target: &str) {
    #[cfg(windows)]
    {
        let _ = quiet(Command::new("explorer.exe").arg(target)).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(target).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let p = Path::new(target);
        if !target.contains("://") && p.is_file() && target.ends_with(".desktop") {
            if Command::new("gio").args(["launch", target]).spawn().is_ok() {
                return;
            }
        } else if !target.contains("://") && p.is_file() && is_executable(p) {
            // un programme ajouté à la main : on le lance directement, depuis son dossier
            let mut c = Command::new(p);
            if let Some(d) = p.parent() {
                c.current_dir(d);
            }
            let _ = c.spawn();
            return;
        }
        let _ = Command::new("xdg-open").arg(target).spawn();
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// Boîte « Ouvrir » du système : `images` = pochette (jpg, png), sinon un programme. Renvoie le chemin choisi.
/// Windows : boîte de PowerShell. Mac : osascript. Linux : zenity ou kdialog (à installer si absents).
pub fn choose_file(images: bool) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        let filtre = if images { "Images|*.jpg;*.jpeg;*.png" } else { "Jeux et raccourcis|*.exe;*.lnk;*.url" };
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; [Console]::OutputEncoding=[Text.Encoding]::UTF8; \
             $f=New-Object Windows.Forms.Form -Property @{{TopMost=$true}}; $d=New-Object Windows.Forms.OpenFileDialog; \
             $d.Filter='{filtre}'; $d.DereferenceLinks=$false; if($d.ShowDialog($f) -eq 'OK'){{$d.FileName}}"
        );
        let out = quiet(Command::new("powershell.exe").args(["-NoProfile", "-STA", "-Command", &script])).output().map_err(|e| e.to_string())?;
        Ok(texte(&out.stdout))
    }
    #[cfg(target_os = "macos")]
    {
        let script = if images {
            "POSIX path of (choose file of type {\"public.image\"} with prompt \"Choisis une image\")"
        } else {
            "POSIX path of (choose file with prompt \"Choisis une application ou un programme\")"
        };
        let out = Command::new("osascript").args(["-e", script]).output().map_err(|e| e.to_string())?;
        Ok(texte(&out.stdout)) // annulé : osascript sort avec une erreur et rien sur la sortie
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let essais: [(&str, Vec<&str>); 2] = [
            ("zenity", if images { vec!["--file-selection", "--title=Choisis une image", "--file-filter=Images | *.jpg *.jpeg *.png"] } else { vec!["--file-selection", "--title=Choisis un programme"] }),
            ("kdialog", if images { vec!["--getopenfilename", ".", "*.jpg *.jpeg *.png|Images"] } else { vec!["--getopenfilename", "."] }),
        ];
        for (prog, args) in essais {
            if let Ok(out) = Command::new(prog).args(args).output() {
                return Ok(texte(&out.stdout));
            }
        }
        Err("Pour choisir un fichier, installe zenity ou kdialog.".to_string())
    }
}

fn texte(octets: &[u8]) -> Option<String> {
    Some(String::from_utf8_lossy(octets).trim().to_string()).filter(|s| !s.is_empty())
}

/// Roblox et Minecraft, quand ils sont installés de la façon habituelle : (identifiant, nom, ce qu'il faut ouvrir pour les lancer).
pub fn autres_jeux() -> Vec<(&'static str, &'static str, String)> {
    #[cfg(windows)]
    {
        const MINECRAFT: &str = "Microsoft.4297127D64EC6_8wekyb3d8bbwe";
        let local = var_dir("LOCALAPPDATA").unwrap_or_default();
        let mut v = vec![];
        // Roblox : un dossier par version, on prend la plus récente
        let roblox = std::fs::read_dir(local.join("Roblox/Versions")).into_iter().flatten().flatten().map(|e| e.path().join("RobloxPlayerBeta.exe")).filter(|p| p.exists());
        // antislashs partout : avec un « / » dans le chemin, l'Explorateur ouvrait Documents au lieu de lancer Roblox
        if let Some(exe) = roblox.max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()).map(|p| native_path(&p)) {
            v.push(("other:roblox", "Roblox", exe.to_string_lossy().into_owned()));
        }
        if local.join("Packages").join(MINECRAFT).exists() {
            v.push(("other:minecraft", "Minecraft", format!("shell:AppsFolder\\{MINECRAFT}!Minecraft")));
        }
        v
    }
    #[cfg(target_os = "macos")]
    {
        let mut v = vec![];
        for (id, nom, app) in [("other:roblox", "Roblox", "Roblox.app"), ("other:minecraft", "Minecraft", "Minecraft.app")] {
            for dossier in [PathBuf::from("/Applications"), home().join("Applications")] {
                let p = dossier.join(app);
                if p.exists() {
                    v.push((id, nom, p.to_string_lossy().into_owned()));
                    break;
                }
            }
        }
        v
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        vec![]
    }
}
