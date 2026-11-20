//! The quick shade (swipe down from the status bar) and the settings behind
//! the held mark: what they show and what their taps do. Values that take a
//! moment to read (Wi-Fi link, storage) are read in a thread, so opening the
//! shade never holds up a frame.

use std::sync::atomic::Ordering;

use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{hw, texts, with_state, AppWindow, SetBar, SetRow, SetSection, SetSeg, Theme};

/// Rows of the settings list, in this order.
const ROW_WIFI: i32 = 0;
const ROW_HA: i32 = 1;
const ROW_DISPLAY: i32 = 2;
const ROW_VOICE: i32 = 3;
const ROW_SOUND: i32 = 4;
const ROW_STORAGE: i32 = 5;
const ROW_TOUCH: i32 = 6;
const ROW_ABOUT: i32 = 7;
const ROW_RESTART: i32 = 8;

/// The settings pages (SettingsLayer views).
const VIEW_DISPLAY: i32 = 6;
const VIEW_VOICE: i32 = 7;
const VIEW_SOUND: i32 = 8;
const VIEW_ABOUT: i32 = 9;

/// The project's pages on the About page (a QR code each).
const LINKS: [(&str, &str); 2] =
    [("Project page", "https://github.com/lauden-io/PanelOS"), ("Report a problem", "https://github.com/lauden-io/PanelOS/issues")];

pub fn open(ui: &AppWindow) {
    fill(ui);
    ui.set_set_view(1);
}

/// Every value the shade and the settings show, from the current state.
pub fn fill(ui: &AppWindow) {
    let states = texts::list("set.states");
    let state = |i: usize, fallback: &str| -> SharedString { states.get(i).map(String::as_str).unwrap_or(fallback).into() };
    with_state(|s| {
        ui.set_set_shade_title(texts::nth("set.shade", 0).into());
        ui.set_set_lock_title(texts::nth("set.lock", 0).into());
        ui.set_set_title(texts::nth("set.title", 0).into());
        ui.set_set_tagline(texts::nth("set.tagline", 0).into());
        ui.set_set_confirm_title(texts::nth("set.confirm", 0).into());
        ui.set_set_confirm_line(texts::nth("set.confirm.line", 0).into());
        ui.set_set_page_title("Storage".into());
        ui.set_set_page_sub(texts::nth("set.storage.sub", 0).into());
        let level = if s.settings.auto_brightness {
            (s.brightness_target / 1000.0).clamp(0.0, 1.0).powf(1.0 / hw::BACKLIGHT_GAMMA) as f32
        } else {
            s.settings.brightness as f32 / 100.0
        };
        ui.set_set_brightness(level);
        ui.set_set_auto(s.settings.auto_brightness);
        ui.set_set_volume(s.settings.volume as f32 / 100.0);
        let mic_on = !s.settings.mic_muted;
        ui.set_set_mic_on(mic_on);
        ui.set_set_mic_state(if mic_on { state(0, "Listening") } else { state(1, "Muted") });
        let dnd = crate::dnd_active(s);
        ui.set_set_dnd(dnd);
        ui.set_set_dnd_state(if dnd { state(3, "On") } else { state(2, "Off") });
        ui.set_set_sleep_state(state(4, "Now"));
        ui.set_set_theme_state(
            match s.settings.theme.as_str() {
                "dark" => "Dark",
                "light" => "Light",
                _ => states.get(5).map(String::as_str).unwrap_or("Auto"),
            }
            .into(),
        );
        // the display page
        let savers: Vec<SharedString> = crate::settings::SCREENSAVER_CHOICES.iter().map(|(_, l)| SharedString::from(*l)).collect();
        ui.set_set_saver_options(ModelRc::new(VecModel::from(savers)));
        ui.set_set_saver_index(s.settings.screensaver_index());
        ui.set_set_saver_style(if s.settings.screensaver_style == "black" { 1 } else { 0 });
        // the about page
        let host = s.config.as_ref().map(|c| c.ha_host_port()).unwrap_or_default();
        let ip = crate::pairing::local_ip_towards(if host.is_empty() { "8.8.8.8:53" } else { &host }).map(|ip| ip.to_string());
        let ha = s.config.as_ref().map(|c| c.ha_url.trim_end_matches('/').to_string()).unwrap_or_default();
        let about = vec![
            info_row("PanelOS", format!("Version {}", env!("CARGO_PKG_VERSION"))),
            info_row("Model", crate::panel_device::MODEL.to_string()),
            info_row("Address", ip.unwrap_or_else(|| "–".into())),
            info_row("Home Assistant", if ha.is_empty() { "–".into() } else { ha }),
        ];
        ui.set_set_about_rows(ModelRc::new(VecModel::from(about)));
        let links: Vec<SetRow> = LINKS.iter().map(|(l, url)| info_row(l, url.trim_start_matches("https://").to_string())).collect();
        ui.set_set_about_links(ModelRc::new(VecModel::from(links)));
    });
    rows(ui, &Slow::default());
    let weak = ui.as_weak();
    let words = with_state(|s| s.settings.wake_words.clone());
    std::thread::spawn(move || {
        let known = crate::voice::available_wake_words();
        let slow = Slow {
            wifi: wifi_link(),
            free: storage_free(),
            voice: Some(words.iter().filter_map(|id| known.iter().find(|w| w.id == *id).map(|w| w.phrase.clone())).collect()),
            known: known.iter().map(|w| (w.id.clone(), w.phrase.clone())).collect(),
        };
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                rows(&ui, &slow);
            }
        });
    });
}

/// What the list shows once read off the UI thread.
#[derive(Default)]
struct Slow {
    wifi: Option<(String, Option<f64>)>,
    free: Option<u64>,
    voice: Option<Vec<String>>,
    /// The voice satellite's wake words: id, phrase.
    known: Vec<(String, String)>,
}

/// A row of the About page: a name and its value.
fn info_row(label: &str, value: String) -> SetRow {
    SetRow { icon: SharedString::new(), label: label.into(), value: value.into(), danger: false }
}

/// The voice page's two wake word choices: the satellite's wake words (by phrase), and "None" + the same for the second.
fn wake_choices(ui: &AppWindow, known: &[(String, String)], active: &[String]) {
    let phrases: Vec<SharedString> = known.iter().map(|(_, p)| SharedString::from(p.as_str())).collect();
    let pos = |i: usize| active.get(i).and_then(|id| known.iter().position(|(k, _)| k == id));
    ui.set_set_wake1(pos(0).map_or(-1, |p| p as i32));
    ui.set_set_wake2(pos(1).map_or(0, |p| p as i32 + 1));
    let mut second = vec![SharedString::from("None")];
    second.extend(phrases.iter().cloned());
    ui.set_set_wake_options(ModelRc::new(VecModel::from(phrases)));
    ui.set_set_wake_options2(ModelRc::new(VecModel::from(second)));
}

/// The active wake words after a pick on the voice page (`which` 0 the first, 1 the second; for the second, 0 = none).
fn picked_wake_words(known: &[String], active: &[String], which: usize, index: usize) -> Vec<String> {
    let first = active.first().cloned();
    let second = active.get(1).cloned();
    match which {
        0 => {
            let Some(new) = known.get(index).cloned() else { return active.to_vec() };
            // the second one stays unless it is the new first one
            std::iter::once(new.clone()).chain(second.filter(|w| *w != new)).collect()
        }
        _ => {
            let new = index.checked_sub(1).and_then(|i| known.get(i)).cloned();
            match (first, new) {
                (Some(f), Some(n)) if f == n => active.to_vec(),
                (Some(f), Some(n)) => vec![f, n],
                (Some(f), None) => vec![f],
                (None, Some(n)) => vec![n],
                (None, None) => Vec::new(),
            }
        }
    }
}

/// The About page's QR code: the chosen link.
fn about_qr(ui: &AppWindow) {
    let i = (ui.get_set_about_link().max(0) as usize).min(LINKS.len() - 1);
    if let Some(img) = crate::pairing::qr_image(LINKS[i].1, 208) {
        ui.set_set_about_qr(img);
    }
}

/// The settings list (Wi-Fi and storage once read).
fn rows(ui: &AppWindow, slow: &Slow) {
    if !slow.known.is_empty() {
        let active = with_state(|s| s.settings.wake_words.clone());
        wake_choices(ui, &slow.known, &active);
    }
    let list = with_state(|s| {
        let wifi = match &slow.wifi {
            Some((ssid, Some(dbm))) => format!("{ssid} · {dbm:.0} dBm"),
            Some((ssid, None)) => ssid.clone(),
            None => "…".into(),
        };
        let host = s
            .config
            .as_ref()
            .map(|c| c.ha_url.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_string())
            .unwrap_or_default();
        let ha = format!("{host} · {}", if s.connected { "connected" } else { "not connected" });
        // Short enough for the value column (about 210 px at 13 px).
        let theme = match s.settings.theme.as_str() {
            "dark" => "Dark",
            "light" => "Light",
            _ => "Auto",
        };
        let saver = match s.settings.screensaver_seconds {
            0 => "no screensaver".to_string(),
            n if n < 60 => format!("screensaver {n} s"),
            n if n % 60 == 0 && n < 3600 => format!("screensaver {} min", n / 60),
            n => format!("screensaver {} h", n / 3600),
        };
        let voice = match &slow.voice {
            None => "…".to_string(),
            Some(v) if v.is_empty() => "no wake word".into(),
            Some(v) => v.join(" · "),
        };
        let row = |icon: &str, label: &str, value: String, danger: bool| SetRow {
            icon: icon.into(),
            label: label.into(),
            value: value.into(),
            danger,
        };
        vec![
            row("\u{F05A9}", "Wi-Fi", wifi, false),
            row("\u{F07D0}", "Home Assistant", ha, false),
            row("\u{F0379}", "Display", format!("{theme} · {saver}"), false),
            row("\u{F036C}", "Voice", voice, false),
            row("\u{F057E}", "Sound", format!("Volume {} %", s.settings.volume), false),
            row("\u{F02CA}", "Storage", slow.free.map(|b| format!("{} free", size_text(b))).unwrap_or_else(|| "…".into()), false),
            row("\u{F0741}", "Touch", if s.settings.touch_offset == (0.0, 0.0) { "Calibrate".into() } else { "Calibrated".into() }, false),
            row("\u{F02FD}", "About", format!("PanelOS {}", env!("CARGO_PKG_VERSION")), false),
            row("\u{F0709}", "Restart", String::new(), true),
        ]
    });
    ui.set_set_rows(ModelRc::new(VecModel::from(list)));
}

/// Taps in the shade and the settings (`set.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    let v: f32 = value.parse().unwrap_or(0.0);
    match what {
        "set.view" => {
            let view: i32 = value.parse().unwrap_or(0);
            if view == 4 {
                storage(ui);
            }
            if view == VIEW_ABOUT {
                about_qr(ui);
            }
            ui.set_set_view(view);
        }
        // the display page: theme (0 light, 1 dark, 2 auto), time format (0 24-hour, 1 12-hour), screensaver
        "set.theme.mode" => {
            crate::on_setting(ui, "theme", match value { "0" => "light", "1" => "dark", _ => "auto" });
            with_state(|s| crate::update_theme(ui, s));
            fill(ui);
        }
        "set.timefmt" => {
            crate::on_setting(ui, "time_format", if value == "1" { "12" } else { "24" });
            fill(ui);
        }
        "set.saver" => {
            crate::on_setting(ui, "screensaver_index", value);
            fill(ui);
        }
        "set.saver.style" => {
            crate::on_setting(ui, "screensaver_style", if value == "1" { "black" } else { "clock" });
            fill(ui);
        }
        // the voice page: the first or the second wake word (the second's 0 = none)
        "set.wake1" | "set.wake2" => {
            let known: Vec<String> = crate::voice::available_wake_words().into_iter().map(|w| w.id).collect();
            let active = with_state(|s| s.settings.wake_words.clone());
            let index = value.parse::<usize>().unwrap_or(usize::MAX);
            let words = picked_wake_words(&known, &active, if what == "set.wake1" { 0 } else { 1 }, index);
            if words != active {
                crate::on_setting(ui, "wake_words", &words.join(","));
            }
            fill(ui);
        }
        // the about page: which link the QR code shows
        "set.link" => {
            ui.set_set_about_link(value.parse::<i32>().unwrap_or(0).clamp(0, LINKS.len() as i32 - 1));
            about_qr(ui);
        }
        "set.brightness" | "set.brightness.done" => {
            let percent = (v.clamp(0.01, 1.0) * 100.0).round() as u8;
            with_state(|s| {
                s.settings.auto_brightness = false;
                s.settings.brightness = percent.max(1);
                s.brightness_target = hw::manual_brightness_permille(s.settings.brightness) as f64;
                if what == "set.brightness.done" {
                    s.settings.save();
                    let _ = s.device_notify.send(());
                }
            });
            ui.set_set_brightness(percent as f32 / 100.0);
            ui.set_set_auto(false);
        }
        // the shade's Auto box (29.5): the light sensor sets the level; moving the slider unticks it
        "set.auto" => {
            with_state(|s| {
                s.settings.auto_brightness = !s.settings.auto_brightness;
                if !s.settings.auto_brightness {
                    s.brightness_target = hw::manual_brightness_permille(s.settings.brightness) as f64;
                }
                s.settings.save();
                let _ = s.device_notify.send(());
            });
            fill(ui);
        }
        "set.volume" => {
            let percent = (v.clamp(0.0, 1.0) * 100.0).round() as u8;
            with_state(|s| {
                // muted by the slider: the level the finger started from is where the speaker mark goes back to
                if percent == 0 && s.settings.volume > 0 {
                    s.vol_before_mute = s.settings.volume;
                }
                s.settings.volume = percent;
                hw::set_volume(percent);
                s.player.play(crate::voice::Sound::Chime);
                s.settings.save();
                let _ = s.device_notify.send(());
            });
            ui.set_speaker_muted(percent == 0);
            fill(ui);
        }
        "set.mic" => {
            with_state(|s| {
                s.settings.mic_muted = !s.settings.mic_muted;
                s.mic_muted.store(s.settings.mic_muted, Ordering::SeqCst);
                ui.set_mic_muted(s.settings.mic_muted);
                s.settings.save();
                let _ = s.device_notify.send(());
            });
            // the hint line follows at once (not with the next refresh)
            crate::refresh_hint(ui);
            fill(ui);
        }
        "set.theme" => {
            with_state(|s| {
                s.settings.theme = match s.settings.theme.as_str() {
                    "auto" => "dark",
                    "dark" => "light",
                    _ => "auto",
                }
                .into();
                s.settings.save();
                crate::update_theme(ui, s);
                let _ = s.device_notify.send(());
            });
            fill(ui);
        }
        "set.dnd" => {
            with_state(|s| {
                let on = !crate::dnd_active(s);
                s.dnd = Some((on, crate::dnd_setting(s)));
                ui.set_dnd_on(on);
            });
            fill(ui);
        }
        "set.sleep" => {
            ui.set_set_view(0);
            with_state(|s| {
                s.screen_off = true;
                crate::start_screensaver(ui, s);
            });
        }
        "set.row" => match value.parse::<i32>().unwrap_or(-1) {
            ROW_WIFI => crate::wifi::open(ui),
            ROW_HA => crate::open_ha_setup(ui),
            ROW_DISPLAY => ui.set_set_view(VIEW_DISPLAY),
            ROW_VOICE => ui.set_set_view(VIEW_VOICE),
            ROW_SOUND => ui.set_set_view(VIEW_SOUND),
            ROW_STORAGE => {
                storage(ui);
                ui.set_set_view(4);
            }
            ROW_TOUCH => crate::start_touch_test(ui),
            ROW_ABOUT => {
                about_qr(ui);
                ui.set_set_view(VIEW_ABOUT);
            }
            ROW_RESTART => ui.set_set_view(5),
            _ => {}
        },
        "set.restart" => {
            eprintln!("nspanel-app: restart requested (panel settings)");
            std::process::exit(0)
        }
        _ => eprintln!("nspanel-app: act {what} {value} (not handled)"),
    }
}

/// The storage page: PanelOS's own slot (and its spare copy), one section
/// per data partition with its biggest folders, and what is not used yet.
/// Every value is read on the panel when the page opens.
fn storage(ui: &AppWindow) {
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let parts: Vec<Partition> =
            [("Data", "/mnt/persist"), ("Extra space", "/mnt/data")].iter().filter_map(|(n, p)| partition(n, p)).collect();
        let blocks = block_partitions();
        let slot = |name: &str| blocks.iter().find(|(n, _, _)| n == name).and_then(|(_, dev, size)| boot_image(dev).map(|img| (*size, img)));
        let system = slot("recovery");
        let spare = slot("boot");
        let unused: u64 = blocks
            .iter()
            .filter(|(n, _, _)| matches!(n.as_str(), "system" | "vendor" | "oem" | "stback" | "backup"))
            .map(|(_, _, size)| size)
            .sum();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let dark = ui.global::<Theme>().get_dark();
            let text = if dark { Color::from_rgb_u8(0xF5, 0xF3, 0xEE) } else { Color::from_rgb_u8(0x10, 0x11, 0x14) };
            let free_tint = if dark { Color::from_rgb_u8(0x3a, 0x3d, 0x41) } else { Color::from_rgb_u8(0xcf, 0xca, 0xc0) };
            let tints = [text, Color::from_rgb_u8(0x8a, 0x8f, 0x94), Color::from_rgb_u8(0x5c, 0x5f, 0x63)];
            let signal = Color::from_rgb_u8(0xFF, 0x4A, 0x1C);
            let (mut total, mut free) = (0u64, 0u64);
            let mut sections: Vec<SetSection> = Vec::new();
            // PanelOS's slot: the image in it (kernel, system files, device tree) and the rest.
            for (title, kind, s) in [("PanelOS", "own partition", &system), ("Spare copy", "boot partition", &spare)] {
                let Some((size, img)) = s else { continue };
                let used = img.total.min(*size);
                sections.push(SetSection {
                    name: title.into(),
                    size: format!("{kind} · {}", size_text(*size)).into(),
                    segs: ModelRc::new(VecModel::from(vec![SetSeg { share: used as f32 / (*size).max(1) as f32, tint: signal }])),
                    rows: ModelRc::new(VecModel::from(vec![
                        SetBar { label: "PanelOS system".into(), value: 0.0, tint: signal, text: size_text(used).into() },
                        SetBar { label: "Free".into(), value: 0.0, tint: free_tint, text: size_text(size - used).into() },
                    ])),
                    note: format!(
                        "Kernel {} · system files {} · device tree {}",
                        size_text(img.kernel),
                        size_text(img.ramdisk),
                        size_text(img.second)
                    )
                    .into(),
                });
            }
            sections.extend(parts.iter()
                .map(|p| {
                    total += p.total;
                    free += p.free;
                    let mut segs = Vec::new();
                    let mut rows = Vec::new();
                    let mut grey = 0;
                    for (name, bytes) in &p.dirs {
                        // Settings and logs are the panel's own: Signal; the rest in quiet tones.
                        let tint = if name == "Settings and logs" {
                            signal
                        } else {
                            grey += 1;
                            tints[(grey - 1).min(tints.len() - 1)]
                        };
                        segs.push(SetSeg { share: *bytes as f32 / p.total.max(1) as f32, tint });
                        rows.push(SetBar { label: name.as_str().into(), value: 0.0, tint, text: size_text(*bytes).into() });
                    }
                    rows.push(SetBar { label: "Free".into(), value: 0.0, tint: free_tint, text: size_text(p.free).into() });
                    SetSection {
                        name: p.name.into(),
                        size: format!("partition · {}", size_text(p.total)).into(),
                        segs: ModelRc::new(VecModel::from(segs)),
                        rows: ModelRc::new(VecModel::from(rows)),
                        note: SharedString::new(),
                    }
                }));
            if unused > 0 {
                sections.push(SetSection {
                    name: "Not used yet".into(),
                    size: size_text(unused).into(),
                    segs: ModelRc::new(VecModel::from(Vec::<SetSeg>::new())),
                    rows: ModelRc::new(VecModel::from(Vec::<SetBar>::new())),
                    note: "Old Android partitions: system, vendor, oem, backups".into(),
                });
            }
            ui.set_set_used(if total > 0 { 1.0 - free as f32 / total as f32 } else { 0.0 });
            ui.set_set_used_text(format!("{} free of {}", size_text(free), size_text(total)).into());
            ui.set_set_sections(ModelRc::new(VecModel::from(sections)));
        });
    });
}

/// The eMMC partitions: name, device, size in bytes (from sysfs).
fn block_partitions() -> Vec<(String, String, u64)> {
    let Ok(dir) = std::fs::read_dir("/sys/class/block") else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| {
            let dev = e.file_name().to_string_lossy().to_string();
            let uevent = std::fs::read_to_string(e.path().join("uevent")).ok()?;
            let name = uevent.lines().find_map(|l| l.strip_prefix("PARTNAME="))?.to_string();
            let sectors: u64 = std::fs::read_to_string(e.path().join("size")).ok()?.trim().parse().ok()?;
            Some((name, format!("/dev/{dev}"), sectors * 512))
        })
        .collect()
}

/// A PanelOS image in a slot (Android boot image header, version 0).
struct BootImage {
    kernel: u64,
    ramdisk: u64,
    second: u64,
    /// Header plus the three parts, each rounded up to whole pages.
    total: u64,
}

fn boot_image(dev: &str) -> Option<BootImage> {
    use std::io::Read;
    let mut head = [0u8; 48];
    std::fs::File::open(dev).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != b"ANDROID!" {
        return None;
    }
    let word = |at: usize| u32::from_le_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]) as u64;
    let (kernel, ramdisk, second, page) = (word(8), word(16), word(24), word(36).max(1));
    let pages = |n: u64| n.div_ceil(page) * page;
    Some(BootImage { kernel, ramdisk, second, total: page + pages(kernel) + pages(ramdisk) + pages(second) })
}

struct Partition {
    name: &'static str,
    total: u64,
    free: u64,
    /// Its biggest folders (up to three, the rest as "Other"), largest first.
    dirs: Vec<(String, u64)>,
}

fn partition(name: &'static str, path: &str) -> Option<Partition> {
    let (total, free) = statvfs(path)?;
    let mut dirs: Vec<(String, u64)> = std::fs::read_dir(path)
        .ok()?
        .flatten()
        .filter(|e| e.file_name() != "lost+found")
        .map(|e| (folder_label(&e.file_name().to_string_lossy()), tree_size(&e.path())))
        .filter(|(_, b)| *b > 0)
        .collect();
    dirs.sort_by(|a, b| b.1.cmp(&a.1));
    if dirs.len() > 4 {
        let other: u64 = dirs[3..].iter().map(|d| d.1).sum();
        dirs.truncate(3);
        dirs.push(("Other".into(), other));
    }
    Some(Partition { name, total, free, dirs })
}

fn folder_label(name: &str) -> String {
    match name {
        "nspanel" => "Settings and logs".into(),
        "proxlog" => "Sensor logs".into(),
        "panelos-web" | "panelos-web.squashfs" => "Web mode: the program".into(),
        "web-data" | "panelos-web-data" => "Web mode: browser cache and storage".into(),
        "panelos-ui" | "panelos-ui.squashfs" => "PanelOS interface".into(),
        "panelos-ui-dev" => "PanelOS interface (test copy)".into(),
        other => other.into(),
    }
}

fn tree_size(path: &std::path::Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        use std::os::unix::fs::MetadataExt;
        return meta.blocks() * 512;
    }
    std::fs::read_dir(path).map(|it| it.flatten().map(|e| tree_size(&e.path())).sum()).unwrap_or(0)
}

fn statvfs(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    // SAFETY: statvfs(3) into a zeroed struct for a valid C string.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    // Only real mounts: the folder of an absent partition sits on the root file system.
    let mut root: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c"/".as_ptr(), &mut root) } == 0 && root.f_fsid == st.f_fsid {
        return None;
    }
    let frsize = st.f_frsize as u64;
    Some((st.f_blocks as u64 * frsize, st.f_bavail as u64 * frsize))
}

fn storage_free() -> Option<u64> {
    let free: u64 = ["/mnt/persist", "/mnt/data"].iter().filter_map(|p| statvfs(p)).map(|(_, f)| f).sum();
    (free > 0).then_some(free)
}

fn size_text(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else if mb >= 100.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

/// The network and its signal (`iw dev wlan0 link`).
pub fn wifi_link() -> Option<(String, Option<f64>)> {
    let out = std::process::Command::new("iw").args(["dev", "wlan0", "link"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let ssid = text.lines().find_map(|l| l.trim_start().strip_prefix("SSID: ").map(str::to_string))?;
    let dbm = text
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("signal: "))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok());
    Some((ssid, dbm))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn wake_word_picks() {
        let known = ids(&["alexa", "hey_jarvis", "okay_nabu"]);
        let active = ids(&["okay_nabu", "hey_jarvis"]);
        // a new first one keeps the second
        assert_eq!(picked_wake_words(&known, &active, 0, 0), ids(&["alexa", "hey_jarvis"]));
        // the second one as the first: the second goes
        assert_eq!(picked_wake_words(&known, &active, 0, 1), ids(&["hey_jarvis"]));
        // the second: none, another one, the first one again (no change)
        assert_eq!(picked_wake_words(&known, &active, 1, 0), ids(&["okay_nabu"]));
        assert_eq!(picked_wake_words(&known, &active, 1, 1), ids(&["okay_nabu", "alexa"]));
        assert_eq!(picked_wake_words(&known, &active, 1, 3), active);
        // without a first one the second becomes it; out of range changes nothing
        assert_eq!(picked_wake_words(&known, &[], 1, 2), ids(&["hey_jarvis"]));
        assert_eq!(picked_wake_words(&known, &active, 0, 9), active);
    }
}
