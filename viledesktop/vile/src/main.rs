use std::fs;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use vile_protocol::{
    color_cache_dir, load_config, read_json_line, sanitize_recipe_name, save_wallpaper_state,
    socket_path, write_json_line, BarToDaemon, DaemonToBar, OverlayKind, WallpaperMode,
    WallpaperState,
};

enum Cmd {
    Vol(VolCmd),
    Refresh,
    Apps,
    Notifications,
    Tray,
    Menu,
    Overlay(OverlayKind),
    Shot(ShotMode),
    ColorLab {
        image: Option<String>,
        load: Option<Option<String>>,
    },
    Colors {
        recipe: Option<String>,
        image: Option<String>,
        apply_wall: bool,
        wall_mode: Option<WallpaperMode>,
    },
    Wallpaper {
        image: Option<String>,
        mode: Option<WallpaperMode>,
    },
}

enum ShotMode {
    Full,
    Crop,
}

enum VolCmd {
    Delta(i32),
    Mute,
}

fn main() -> Result<()> {
    match parse_args(std::env::args().skip(1))? {
        Cmd::Refresh => refresh(),
        Cmd::Vol(cmd) => {
            if send_to_daemon(&cmd).is_err() {
                fallback_wpctl(&cmd);
            }
            Ok(())
        }
        Cmd::ColorLab { image, load } => run_colorlab(image, load),
        Cmd::Colors {
            recipe,
            image,
            apply_wall,
            wall_mode,
        } => match (recipe, image) {
            (Some(recipe), Some(image)) => run_colors(recipe, image, apply_wall, wall_mode),
            (recipe, image) => run_pick(PickKind::Apply, image, recipe, wall_mode),
        },
        Cmd::Wallpaper { image, mode } => match image {
            Some(image) => run_wallpaper(image, mode),
            None => run_pick(PickKind::Wall, None, None, mode),
        },
        Cmd::Apps => send_apps(),
        Cmd::Notifications => send_notifications(),
        Cmd::Tray => send_tray(),
        Cmd::Menu => send_menu(),
        Cmd::Overlay(kind) => send_overlay(kind),
        Cmd::Shot(mode) => run_shot(mode),
    }
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Cmd> {
    let args: Vec<String> = args.into_iter().collect();
    let mut recipe = None;
    let mut image = None;
    let mut vol = None;
    let mut refresh = false;
    let mut wall_only = false;
    let mut apply_wall = false;
    let mut wall_mode = None;
    let mut apps = false;
    let mut notifications = false;
    let mut tray = false;
    let mut menu = false;
    let mut overlay = None;
    let mut shot = None;
    let mut colorlab = false;
    let mut colors = false;
    let mut load: Option<Option<String>> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--refresh" {
            refresh = true;
            i += 1;
            continue;
        }
        if arg == "--apps" {
            apps = true;
            i += 1;
            continue;
        }
        if arg == "--notifications" {
            notifications = true;
            i += 1;
            continue;
        }
        if arg == "--tray" {
            tray = true;
            i += 1;
            continue;
        }
        if arg == "--menu" {
            menu = true;
            i += 1;
            continue;
        }
        if arg == "--bat" {
            overlay = Some(OverlayKind::Battery);
            i += 1;
            continue;
        }
        if arg == "--mem" {
            overlay = Some(OverlayKind::Mem);
            i += 1;
            continue;
        }
        if arg == "--cpu" {
            overlay = Some(OverlayKind::Cpu);
            i += 1;
            continue;
        }
        if arg == "--gpu" {
            overlay = Some(OverlayKind::Gpu);
            i += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--shot=") {
            shot = Some(parse_shot(rest)?);
            i += 1;
            continue;
        }
        if arg == "--shot" {
            let spec = args.get(i + 1).map(String::as_str).unwrap_or("");
            if spec.is_empty() {
                bail!("{}", usage());
            }
            shot = Some(parse_shot(spec)?);
            i += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--vol=") {
            vol = Some(parse_vol(rest)?);
            i += 1;
            continue;
        }
        if arg == "--vol" {
            let spec = args.get(i + 1).map(String::as_str).unwrap_or("");
            if spec.is_empty() {
                bail!("{}", usage());
            }
            vol = Some(parse_vol(spec)?);
            i += 2;
            continue;
        }
        if arg == "--colorlab" {
            colorlab = true;
            i += 1;
            continue;
        }
        if arg == "--load" {
            if let Some(next) = args.get(i + 1) {
                if !next.starts_with('-') {
                    load = Some(Some(next.clone()));
                    i += 2;
                    continue;
                }
            }
            load = Some(None);
            i += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--load=") {
            load = if rest.is_empty() {
                Some(None)
            } else {
                Some(Some(rest.to_string()))
            };
            i += 1;
            continue;
        }
        if arg == "--colors" {
            colors = true;
            if let Some(next) = args.get(i + 1) {
                if !next.is_empty() && !next.starts_with('-') {
                    recipe = Some(next.clone());
                    i += 2;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--colors=") {
            colors = true;
            if !rest.is_empty() {
                recipe = Some(rest.to_string());
            }
            i += 1;
            continue;
        }
        if arg == "--wall" {
            wall_only = true;
            i += 1;
            continue;
        }
        if arg == "-w" {
            apply_wall = true;
            if let Some(next) = args.get(i + 1) {
                if let Some(mode) = WallpaperMode::parse(next) {
                    wall_mode = Some(mode);
                    i += 2;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-w=") {
            apply_wall = true;
            wall_mode =
                Some(WallpaperMode::parse(rest).ok_or_else(|| anyhow::anyhow!("{}", usage()))?);
            i += 1;
            continue;
        }
        if arg == "--img" {
            image = Some(
                args.get(i + 1)
                    .cloned()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("{}", usage()))?,
            );
            i += 2;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("--img=") {
            image = Some(rest.to_string());
            i += 1;
            continue;
        }
        if arg == "-h" || arg == "--help" {
            bail!("{}", usage());
        }
        bail!("{}", usage());
    }
    if refresh {
        return Ok(Cmd::Refresh);
    }
    if apps {
        return Ok(Cmd::Apps);
    }
    if notifications {
        return Ok(Cmd::Notifications);
    }
    if tray {
        return Ok(Cmd::Tray);
    }
    if menu {
        return Ok(Cmd::Menu);
    }
    if let Some(kind) = overlay {
        return Ok(Cmd::Overlay(kind));
    }
    if let Some(mode) = shot {
        return Ok(Cmd::Shot(mode));
    }
    if let Some(cmd) = vol {
        return Ok(Cmd::Vol(cmd));
    }
    if colorlab {
        if colors || wall_only {
            bail!("{}", usage());
        }
        return Ok(Cmd::ColorLab { image, load });
    }
    if wall_only {
        if recipe.is_some() {
            bail!("{}", usage());
        }
        return Ok(Cmd::Wallpaper {
            image,
            mode: wall_mode,
        });
    }
    if colors || recipe.is_some() {
        return Ok(Cmd::Colors {
            recipe,
            image,
            apply_wall,
            wall_mode,
        });
    }
    bail!("{}", usage());
}

fn usage() -> &'static str {
    "usage: vile --vol +5 | vile --vol -5 | vile --vol mute | vile --apps | vile --notifications | vile --tray | vile --menu | vile --bat | vile --mem | vile --cpu | vile --gpu | vile --refresh | vile --shot full | vile --shot crop | vile --colorlab [--img <path>] [--load [name]] | vile --colors [name] [--img <path>] [-w [mode]] | vile --wall [--img <path>] [-w [mode]]"
}

fn parse_shot(spec: &str) -> Result<ShotMode> {
    match spec.trim().trim_start_matches('-') {
        "full" => Ok(ShotMode::Full),
        "crop" => Ok(ShotMode::Crop),
        _ => bail!("{}", usage()),
    }
}

fn parse_vol(spec: &str) -> Result<VolCmd> {
    if spec.eq_ignore_ascii_case("mute") {
        return Ok(VolCmd::Mute);
    }
    spec.parse::<i32>()
        .map(VolCmd::Delta)
        .with_context(|| format!("invalid volume command {spec:?}"))
}

enum PickKind {
    Apply,
    Wall,
}

fn sibling(name: &str) -> Result<PathBuf> {
    let sibling = bin_dir()?.join(name);
    Ok(if sibling.is_file() {
        sibling
    } else {
        PathBuf::from(name)
    })
}

fn run_colorlab(image: Option<String>, load: Option<Option<String>>) -> Result<()> {
    run_lab_bin("--lab", image, load, None, None, PickKind::Apply)
}

fn run_pick(
    kind: PickKind,
    image: Option<String>,
    recipe: Option<String>,
    wall_mode: Option<WallpaperMode>,
) -> Result<()> {
    let flag = match kind {
        PickKind::Apply => "--pick",
        PickKind::Wall => "--pick-wall",
    };
    run_lab_bin(flag, image, None, recipe, wall_mode, kind)
}

fn run_lab_bin(
    flag: &str,
    image: Option<String>,
    load: Option<Option<String>>,
    recipe: Option<String>,
    wall_mode: Option<WallpaperMode>,
    kind: PickKind,
) -> Result<()> {
    let bin = sibling("vilecolors")?;
    let out = std::env::temp_dir().join(format!("vile-lab-pick-{}.json", std::process::id()));
    let _ = fs::remove_file(&out);
    let mut cmd = Command::new(&bin);
    cmd.arg(flag).arg("--out").arg(&out);
    if let Some(image) = &image {
        cmd.arg("--img").arg(image);
    }
    match load {
        Some(None) => {
            cmd.arg("--load");
        }
        Some(Some(name)) => {
            cmd.arg("--load").arg(name);
        }
        None => {}
    }
    if let Some(recipe) = &recipe {
        cmd.arg("--recipe").arg(recipe);
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = cmd
        .status()
        .with_context(|| format!("spawn {}", bin.display()))?;
    if !status.success() {
        let _ = fs::remove_file(&out);
        bail!("vilecolors exited {}", status.code().unwrap_or(-1));
    }
    let text = fs::read_to_string(&out).unwrap_or_default();
    let _ = fs::remove_file(&out);
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let value: serde_json::Value =
        serde_json::from_str(text).context("reading color lab selection")?;
    let image = value
        .get("image")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let recipe = value
        .get("recipe")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if image.is_empty() {
        bail!("no image selected");
    }
    match kind {
        PickKind::Wall => run_wallpaper(image, wall_mode),
        PickKind::Apply => {
            if recipe.is_empty() {
                bail!("no recipe selected");
            }
            run_colors(recipe, image, true, wall_mode)
        }
    }
}

fn run_colors(
    recipe: String,
    image: String,
    apply_wall: bool,
    wall_mode: Option<WallpaperMode>,
) -> Result<()> {
    let recipe = sanitize_recipe_name(&recipe);
    if recipe.is_empty() {
        bail!("invalid recipe name");
    }
    match UnixStream::connect(socket_path()) {
        Ok(mut stream) => {
            stream.set_read_timeout(Some(Duration::from_secs(120)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            write_json_line(
                &mut stream,
                &BarToDaemon::Colors {
                    recipe,
                    image,
                    apply_wall,
                    wall_mode,
                },
            )?;
            match read_json_line::<DaemonToBar>(&mut stream)? {
                DaemonToBar::Ok => {
                    apply_cached_sequences();
                    Ok(())
                }
                DaemonToBar::Error { message } => bail!("{message}"),
                _ => bail!("unexpected reply from viledaemon"),
            }
        }
        Err(_) => {
            if apply_wall {
                run_vilecolors_then_wall(&recipe, &image, wall_mode)
            } else {
                exec_vilecolors(&recipe, &image)
            }
        }
    }
}

fn run_wallpaper(image: String, mode: Option<WallpaperMode>) -> Result<()> {
    match UnixStream::connect(socket_path()) {
        Ok(mut stream) => {
            stream.set_read_timeout(Some(Duration::from_secs(30)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            write_json_line(&mut stream, &BarToDaemon::Wallpaper { image, mode })?;
            match read_json_line::<DaemonToBar>(&mut stream)? {
                DaemonToBar::Ok => Ok(()),
                DaemonToBar::Error { message } => bail!("{message}"),
                _ => bail!("unexpected reply from viledaemon"),
            }
        }
        Err(_) => spawn_vilewall(&image, mode),
    }
}

fn apply_cached_sequences() {
    use std::io::{self, IsTerminal, Write};
    let Ok(seq) = fs::read(color_cache_dir().join("sequences")) else {
        return;
    };
    if seq.is_empty() {
        return;
    }
    let stdout = io::stdout();
    if stdout.is_terminal() {
        let mut out = stdout.lock();
        let _ = out.write_all(&seq);
        let _ = out.flush();
    }
    if let Ok(mut tty) = fs::OpenOptions::new().write(true).open("/dev/tty") {
        let _ = tty.write_all(&seq);
        let _ = tty.flush();
    }
}

fn run_shot(mode: ShotMode) -> Result<()> {
    let sibling = bin_dir()?.join("vileshot");
    let bin = if sibling.is_file() {
        sibling
    } else {
        PathBuf::from("vileshot")
    };
    let arg = match mode {
        ShotMode::Full => "full",
        ShotMode::Crop => "crop",
    };
    let err = Command::new(&bin).arg(arg).exec();
    Err(err).with_context(|| format!("exec {}", bin.display()))
}

fn exec_vilecolors(recipe: &str, image: &str) -> Result<()> {
    let sibling = bin_dir()?.join("vilecolors");
    let bin = if sibling.is_file() {
        sibling
    } else {
        PathBuf::from("vilecolors")
    };
    let err = Command::new(&bin)
        .args(["--recipe", recipe, "--img", image])
        .exec();
    Err(err).with_context(|| format!("exec {}", bin.display()))
}

fn run_vilecolors_then_wall(recipe: &str, image: &str, mode: Option<WallpaperMode>) -> Result<()> {
    let sibling = bin_dir()?.join("vilecolors");
    let bin = if sibling.is_file() {
        sibling
    } else {
        PathBuf::from("vilecolors")
    };
    let status = Command::new(&bin)
        .args(["--recipe", recipe, "--img", image])
        .status()
        .with_context(|| format!("spawn {}", bin.display()))?;
    if !status.success() {
        bail!("vilecolors exited {}", status.code().unwrap_or(-1));
    }
    spawn_vilewall(image, mode)
}

fn spawn_vilewall(image: &str, mode: Option<WallpaperMode>) -> Result<()> {
    let image_path = PathBuf::from(image)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(image));
    let mode = mode.unwrap_or_else(|| {
        load_config()
            .map(|cfg| cfg.wallpaper.mode)
            .unwrap_or_default()
    });
    let state = WallpaperState {
        image: image_path.to_string_lossy().into_owned(),
        mode,
    };
    let _ = save_wallpaper_state(&state);
    let sibling = bin_dir()?.join("vilewall");
    let bin = if sibling.is_file() {
        sibling
    } else {
        PathBuf::from("vilewall")
    };
    let mut cmd = Command::new(&bin);
    cmd.arg("--img").arg(&state.image);
    cmd.arg("--mode").arg(mode.as_str());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            if libc_setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
        .with_context(|| format!("starting {}", bin.display()))?;
    Ok(())
}

fn send_apps() -> Result<()> {
    send_toggle(BarToDaemon::AppsToggle)
}

fn send_notifications() -> Result<()> {
    send_toggle(BarToDaemon::NotificationsToggle)
}

fn send_tray() -> Result<()> {
    send_toggle(BarToDaemon::TrayToggle)
}

fn send_menu() -> Result<()> {
    send_toggle(BarToDaemon::MenuToggle)
}

fn send_overlay(kind: OverlayKind) -> Result<()> {
    send_toggle(BarToDaemon::Overlay { kind })
}

fn send_toggle(msg: BarToDaemon) -> Result<()> {
    let path = socket_path();
    let mut stream =
        UnixStream::connect(&path).with_context(|| format!("connecting to {}", path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    write_json_line(&mut stream, &msg)?;
    match read_json_line::<DaemonToBar>(&mut stream) {
        Ok(DaemonToBar::Ok) | Err(_) => Ok(()),
        Ok(DaemonToBar::Error { message }) => bail!("{message}"),
        Ok(_) => Ok(()),
    }
}

fn send_to_daemon(cmd: &VolCmd) -> Result<()> {
    let path = socket_path();
    let mut stream =
        UnixStream::connect(&path).with_context(|| format!("connecting to {}", path.display()))?;
    let msg = match cmd {
        VolCmd::Delta(delta) => BarToDaemon::Volume { delta: *delta },
        VolCmd::Mute => BarToDaemon::Mute,
    };
    write_json_line(&mut stream, &msg)?;
    Ok(())
}

fn fallback_wpctl(cmd: &VolCmd) {
    match cmd {
        VolCmd::Mute => {
            let _ = Command::new("wpctl")
                .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
                .status();
        }
        VolCmd::Delta(0) => {}
        VolCmd::Delta(delta) => {
            let bump = delta.unsigned_abs();
            let arg = if *delta > 0 {
                format!("{bump}%+")
            } else {
                format!("{bump}%-")
            };
            let _ = Command::new("wpctl")
                .args(["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", &arg])
                .status();
        }
    }
}

fn refresh() -> Result<()> {
    let dir = bin_dir()?;
    let daemon = dir.join("viledaemon");
    let bar = dir.join("vilebar");
    let wall = dir.join("vilewall");
    if !daemon.is_file() {
        bail!("missing {}", daemon.display());
    }
    if !bar.is_file() {
        bail!("missing {}", bar.display());
    }
    if !wall.is_file() {
        bail!("missing {}", wall.display());
    }
    stop_vile();
    thread::sleep(Duration::from_millis(300));
    spawn_detached(&daemon)?;
    thread::sleep(Duration::from_millis(150));
    spawn_detached(&bar)?;
    thread::sleep(Duration::from_millis(150));
    spawn_detached(&wall)?;
    Ok(())
}

fn bin_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("current_exe")?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent()
        .map(Path::to_path_buf)
        .context("vile has no parent directory")
}

fn stop_vile() {
    let self_pid = std::process::id();
    for pid in vile_pids() {
        if pid != self_pid {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
    }
    thread::sleep(Duration::from_millis(150));
    for pid in vile_pids() {
        if pid != self_pid {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
        }
    }
}

fn vile_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(proc) = fs::read_dir("/proc") else {
        return pids;
    };
    for entry in proc.flatten() {
        let pid: u32 = match entry.file_name().to_str().and_then(|s| s.parse().ok()) {
            Some(pid) => pid,
            None => continue,
        };
        let comm = fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        let comm = comm.trim();
        let cmdline = fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let exe = String::from_utf8_lossy(&cmdline);
        let exe = exe.split('\0').next().unwrap_or("");
        if is_vile_process(comm, exe) {
            pids.push(pid);
        }
    }
    pids
}

fn is_vile_process(comm: &str, exe: &str) -> bool {
    let base = Path::new(exe)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    matches!(comm, "viledaemon" | "vilebar" | "vilewall")
        || matches!(base, "viledaemon" | "vilebar" | "vilewall")
}

fn spawn_detached(bin: &Path) -> Result<()> {
    let mut cmd = Command::new(bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        );
    unsafe {
        cmd.pre_exec(|| {
            if libc_setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
        .with_context(|| format!("starting {}", bin.display()))?;
    Ok(())
}

fn libc_setsid() -> i32 {
    unsafe { setsid() }
}

extern "C" {
    fn setsid() -> i32;
}

#[cfg(test)]
mod tests {
    use super::{parse_args, parse_vol, Cmd, OverlayKind, ShotMode, VolCmd};

    #[test]
    fn parses_signed_percent_and_mute() {
        match parse_vol("+5").unwrap() {
            VolCmd::Delta(5) => {}
            _ => panic!("expected +5"),
        }
        match parse_vol("-5").unwrap() {
            VolCmd::Delta(-5) => {}
            _ => panic!("expected -5"),
        }
        match parse_vol("mute").unwrap() {
            VolCmd::Mute => {}
            _ => panic!("expected mute"),
        }
    }

    #[test]
    fn parses_refresh() {
        match parse_args(["--refresh".into()]).unwrap() {
            Cmd::Refresh => {}
            _ => panic!("expected refresh"),
        }
    }

    #[test]
    fn parses_apps() {
        match parse_args(["--apps".into()]).unwrap() {
            Cmd::Apps => {}
            _ => panic!("expected apps"),
        }
    }

    #[test]
    fn parses_notifications() {
        match parse_args(["--notifications".into()]).unwrap() {
            Cmd::Notifications => {}
            _ => panic!("expected notifications"),
        }
    }

    #[test]
    fn parses_tray() {
        match parse_args(["--tray".into()]).unwrap() {
            Cmd::Tray => {}
            _ => panic!("expected tray"),
        }
    }

    #[test]
    fn parses_menu() {
        match parse_args(["--menu".into()]).unwrap() {
            Cmd::Menu => {}
            _ => panic!("expected menu"),
        }
    }

    #[test]
    fn parses_info_overlays() {
        match parse_args(["--bat".into()]).unwrap() {
            Cmd::Overlay(OverlayKind::Battery) => {}
            _ => panic!("expected bat"),
        }
        match parse_args(["--mem".into()]).unwrap() {
            Cmd::Overlay(OverlayKind::Mem) => {}
            _ => panic!("expected mem"),
        }
        match parse_args(["--cpu".into()]).unwrap() {
            Cmd::Overlay(OverlayKind::Cpu) => {}
            _ => panic!("expected cpu"),
        }
        match parse_args(["--gpu".into()]).unwrap() {
            Cmd::Overlay(OverlayKind::Gpu) => {}
            _ => panic!("expected gpu"),
        }
    }

    #[test]
    fn parses_shot() {
        match parse_args(["--shot".into(), "full".into()]).unwrap() {
            Cmd::Shot(ShotMode::Full) => {}
            _ => panic!("expected shot full"),
        }
        match parse_args(["--shot=crop".into()]).unwrap() {
            Cmd::Shot(ShotMode::Crop) => {}
            _ => panic!("expected shot crop"),
        }
        match parse_args(["--shot".into(), "--full".into()]).unwrap() {
            Cmd::Shot(ShotMode::Full) => {}
            _ => panic!("expected shot --full"),
        }
        assert!(parse_args(["--shot".into()]).is_err());
        assert!(parse_args(["--shot".into(), "window".into()]).is_err());
    }

    #[test]
    fn parses_colors() {
        match parse_args([
            "--colors".into(),
            "exported".into(),
            "--img".into(),
            "wall.png".into(),
        ])
        .unwrap()
        {
            Cmd::Colors {
                recipe,
                image,
                apply_wall,
                wall_mode,
            } => {
                assert_eq!(recipe.as_deref(), Some("exported"));
                assert_eq!(image.as_deref(), Some("wall.png"));
                assert!(!apply_wall);
                assert!(wall_mode.is_none());
            }
            _ => panic!("expected colors"),
        }
        match parse_args(["--img=/tmp/a.png".into(), "--colors=cutty".into()]).unwrap() {
            Cmd::Colors {
                recipe,
                image,
                apply_wall,
                ..
            } => {
                assert_eq!(recipe.as_deref(), Some("cutty"));
                assert_eq!(image.as_deref(), Some("/tmp/a.png"));
                assert!(!apply_wall);
            }
            _ => panic!("expected colors"),
        }
        match parse_args(["--colors".into()]).unwrap() {
            Cmd::Colors { recipe, image, .. } => {
                assert!(recipe.is_none());
                assert!(image.is_none());
            }
            _ => panic!("expected colors picker"),
        }
        match parse_args(["--colors".into(), "exported".into()]).unwrap() {
            Cmd::Colors { recipe, image, .. } => {
                assert_eq!(recipe.as_deref(), Some("exported"));
                assert!(image.is_none());
            }
            _ => panic!("expected colors name only"),
        }
    }

    #[test]
    fn parses_colorlab() {
        match parse_args(["--colorlab".into()]).unwrap() {
            Cmd::ColorLab { image, load } => {
                assert!(image.is_none());
                assert!(load.is_none());
            }
            _ => panic!("expected colorlab"),
        }
        match parse_args([
            "--colorlab".into(),
            "--img".into(),
            "a.png".into(),
            "--load".into(),
        ])
        .unwrap()
        {
            Cmd::ColorLab { image, load } => {
                assert_eq!(image.as_deref(), Some("a.png"));
                assert_eq!(load, Some(None));
            }
            _ => panic!("expected colorlab load list"),
        }
        match parse_args(["--colorlab".into(), "--load".into(), "cutty".into()]).unwrap() {
            Cmd::ColorLab { load, .. } => {
                assert_eq!(load, Some(Some("cutty".into())));
            }
            _ => panic!("expected colorlab load name"),
        }
    }

    #[test]
    fn parses_colors_with_wall_flag() {
        match parse_args([
            "--colors".into(),
            "exported".into(),
            "--img".into(),
            "wall.png".into(),
            "-w".into(),
        ])
        .unwrap()
        {
            Cmd::Colors {
                apply_wall,
                wall_mode,
                ..
            } => {
                assert!(apply_wall);
                assert!(wall_mode.is_none());
            }
            _ => panic!("expected colors -w"),
        }
        match parse_args([
            "--colors".into(),
            "exported".into(),
            "--img".into(),
            "wall.png".into(),
            "-w".into(),
            "fill".into(),
        ])
        .unwrap()
        {
            Cmd::Colors { wall_mode, .. } => {
                assert_eq!(wall_mode, Some(super::WallpaperMode::Fill));
            }
            _ => panic!("expected -w fill"),
        }
        match parse_args([
            "--colors".into(),
            "exported".into(),
            "--img".into(),
            "a.jpg".into(),
            "-w=scale".into(),
        ])
        .unwrap()
        {
            Cmd::Colors {
                wall_mode,
                apply_wall,
                ..
            } => {
                assert!(apply_wall);
                assert_eq!(wall_mode, Some(super::WallpaperMode::Fit));
            }
            _ => panic!("expected scale alias"),
        }
    }

    #[test]
    fn parses_wall_command() {
        match parse_args(["--wall".into(), "--img".into(), "bg.png".into()]).unwrap() {
            Cmd::Wallpaper { image, mode } => {
                assert_eq!(image.as_deref(), Some("bg.png"));
                assert!(mode.is_none());
            }
            _ => panic!("expected wall"),
        }
        match parse_args([
            "--wall".into(),
            "--img=bg.png".into(),
            "-w".into(),
            "span".into(),
        ])
        .unwrap()
        {
            Cmd::Wallpaper { mode, .. } => assert_eq!(mode, Some(super::WallpaperMode::Span)),
            _ => panic!("expected wall span"),
        }
        match parse_args(["--wall".into()]).unwrap() {
            Cmd::Wallpaper { image, .. } => assert!(image.is_none()),
            _ => panic!("expected wall picker"),
        }
        assert!(parse_args([
            "--wall".into(),
            "--colors".into(),
            "x".into(),
            "--img".into(),
            "a.png".into()
        ])
        .is_err());
    }
}
