use std::fmt::Write as _;

#[allow(dead_code)]
#[path = "src/warp_pics.rs"]
mod warp_pics;

fn main() {
    // The pages' pictures and fonts are not embedded: they are read at run time
    // from the UI bundle, by the absolute path they have in it. PANELOS_UI_DIR is
    // that path, where tools/build_app.sh puts a copy of ui/ for the build.
    match std::env::var("PANELOS_UI_DIR") {
        Ok(dir) => {
            let config = slint_build::CompilerConfiguration::new()
                .embed_resources(slint_build::EmbedResourcesKind::AsAbsolutePath);
            slint_build::compile_with_config(format!("{dir}/app.slint"), config).expect("Slint UI compile failed");
            println!("cargo:rerun-if-changed={dir}");
            // Pictures the app loads itself (page assets set as properties).
            println!("cargo:rustc-env=PANELOS_UI_PICTURES={dir}");
        }
        Err(_) => {
            slint_build::compile("ui/app.slint").expect("Slint UI compile failed");
            let here = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
            println!("cargo:rustc-env=PANELOS_UI_PICTURES={here}/ui");
        }
    }
    println!("cargo:rerun-if-env-changed=PANELOS_UI_DIR");

    // Material Design Icons name -> codepoint table (from ui/fonts/mdi-codepoints.txt).
    let table = std::fs::read_to_string("ui/fonts/mdi-codepoints.txt").expect("mdi-codepoints.txt");
    let mut out = String::from("pub static MDI: &[(&str, u32)] = &[\n");
    let mut entries: Vec<(String, u32)> = table
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let (name, hex) = l.split_once('\t')?;
            Some((name.to_string(), u32::from_str_radix(hex.trim(), 16).ok()?))
        })
        .collect();
    entries.sort();
    for (name, cp) in entries {
        writeln!(out, "    ({name:?}, 0x{cp:X}),").unwrap();
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("mdi_table.rs");
    std::fs::write(dest, out).unwrap();
    println!("cargo:rerun-if-changed=ui/fonts/mdi-codepoints.txt");
    println!("cargo:rerun-if-changed=ui");

    // The pictures of the weather scenes' moving rows the app draws (src/warp.rs): what the scene's own picture
    // functions pick, so both always load the same files.
    let sky = std::fs::read_to_string("ui/photo_sky.slint").expect("ui/photo_sky.slint");
    let pics = warp_pics::generate(&sky).unwrap_or_else(|e| panic!("ui/photo_sky.slint pictures: {e}"));
    std::fs::write(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("warp_pics.rs"), pics).unwrap();
    // The night stars the app draws (src/stars.rs): the star lists and cloud cover maps next to the night sky photos the
    // scene's `sky` function picks (tools/split_night_stars.py wrote them; ui/ is watched above).
    let ui = std::path::Path::new("ui");
    let stars = warp_pics::night_stars(&sky, &|p| std::fs::read_to_string(ui.join(p)).ok(), &|p| ui.join(p).exists())
        .unwrap_or_else(|e| panic!("ui/photo_sky.slint night stars: {e}"));
    std::fs::write(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("night_stars.rs"), stars).unwrap();
    println!("cargo:rerun-if-changed=ui/photo_sky.slint");
    println!("cargo:rerun-if-changed=src/warp_pics.rs");

    // Build time, reported to Home Assistant as the "compilation time".
    let built = std::process::Command::new("date")
        .arg("+%b %e %Y, %H:%M:%S")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    println!("cargo:rustc-env=NSPANEL_BUILD_TIME={}", built.trim());

    // Wake word engine (C++, TensorFlow Lite + micro frontend), only for the
    // panel (the TFLite library comes from the Buildroot sysroot).
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        let fe = "native/microfrontend/tensorflow/lite/experimental/microfrontend/lib";
        let c_files = [
            "filterbank.c", "filterbank_util.c", "frontend.c", "frontend_util.c", "log_lut.c", "log_scale.c",
            "log_scale_util.c", "noise_reduction.c", "noise_reduction_util.c", "pcan_gain_control.c",
            "pcan_gain_control_util.c", "window.c", "window_util.c",
        ];
        let mut c = cc::Build::new();
        c.include("native/microfrontend").include("native/kissfft").include("native/kissfft/tools")
            .define("FIXED_POINT", "16").flag("-mcpu=cortex-a35").warnings(false);
        for f in c_files {
            c.file(format!("{fe}/{f}"));
        }
        c.file("native/kissfft/kiss_fft.c").file("native/kissfft/tools/kiss_fftr.c");
        // C objects go into the same static library as the C++ part (they
        // reference each other, so two libraries would depend on link order).
        let c_objects = c.compile_intermediates();
        cc::Build::new()
            .cpp(true)
            .objects(c_objects)
            .std("c++17")
            .include("native/microfrontend")
            .include("native/kissfft")
            .include("native/kissfft/tools")
            .define("FIXED_POINT", "16")
            .flag("-mcpu=cortex-a35")
            .warnings(false)
            .file(format!("{fe}/fft.cc"))
            .file(format!("{fe}/fft_util.cc"))
            .file("native/mww.cc")
            .compile("mww");
        println!("cargo:rustc-link-lib=tensorflow-lite");

        // Echo cancellation (WebRTC AudioProcessing from the Buildroot sysroot;
        // include flags from its pkg-config file through Buildroot's wrapper).
        let pkg_config = std::env::var("PKG_CONFIG").unwrap_or_else(|_| "pkg-config".into());
        let cflags = std::process::Command::new(&pkg_config)
            .args(["--cflags", "webrtc-audio-processing-1"])
            .output()
            .expect("pkg-config webrtc-audio-processing-1");
        assert!(cflags.status.success(), "pkg-config: webrtc-audio-processing-1 not found");
        let mut aec = cc::Build::new();
        aec.cpp(true).std("c++17").flag("-mcpu=cortex-a35").warnings(false).file("native/aec.cc");
        for flag in String::from_utf8_lossy(&cflags.stdout).split_whitespace() {
            if let Some(dir) = flag.strip_prefix("-I") {
                aec.include(dir);
            } else if let Some(def) = flag.strip_prefix("-D") {
                let (k, v) = def.split_once('=').map(|(k, v)| (k, Some(v))).unwrap_or((def, None));
                aec.define(k, v);
            }
        }
        aec.compile("aec");
        println!("cargo:rustc-link-lib=webrtc-audio-processing-1");
        println!("cargo:rerun-if-changed=native");
    }

    // ESPHome native API messages (proto/api.proto) -> Rust types.
    let fds = protox::compile(["proto/api.proto"], ["proto"]).expect("api.proto");
    prost_build::Config::new().compile_fds(fds).expect("prost codegen");
    println!("cargo:rerun-if-changed=proto");
}
