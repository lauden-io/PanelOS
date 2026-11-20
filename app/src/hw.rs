//! Panel hardware the app drives directly: backlight, ambient light and
//! proximity (STK3x3x through IIO), SoC temperature.

use std::fs;
use std::path::{Path, PathBuf};

const BACKLIGHT: &str = "/sys/class/backlight/backlight";

pub struct Backlight {
    max: u32,
    current: u32,
}

impl Backlight {
    pub fn open() -> Self {
        let max = fs::read_to_string(format!("{BACKLIGHT}/max_brightness"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(255);
        let current = fs::read_to_string(format!("{BACKLIGHT}/brightness"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(max);
        Self { max, current }
    }

    /// Sets the level in 0..=1000 (per mille); writes only on change.
    /// Development aid: `NSPANEL_DEV_DARK` keeps the backlight off (tests at night).
    pub fn set_permille(&mut self, permille: u32) {
        let permille = if std::env::var_os("NSPANEL_DEV_DARK").is_some() { 0 } else { permille };
        let level = (self.max * permille.min(1000) + 500) / 1000;
        if level != self.current {
            if fs::write(format!("{BACKLIGHT}/brightness"), level.to_string()).is_ok() {
                self.current = level;
            }
        }
    }
}

pub struct LightSensor {
    dir: Option<PathBuf>,
    /// Available scales (lux per count), most sensitive first.
    scales: Vec<f64>,
    index: usize,
    last_lux: Option<f64>,
}

fn read_f64(p: PathBuf) -> Option<f64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

impl LightSensor {
    pub fn open() -> Self {
        let dir = fs::read_dir("/sys/bus/iio/devices").ok().and_then(|entries| {
            entries.filter_map(Result::ok).map(|e| e.path()).find(|p| {
                fs::read_to_string(p.join("name")).map(|n| n.trim().starts_with("stk3")).unwrap_or(false)
            })
        });
        let mut scales: Vec<f64> = dir
            .as_ref()
            .and_then(|d| fs::read_to_string(d.join("in_illuminance_scale_available")).ok())
            .map(|s| s.split_whitespace().filter_map(|v| v.parse().ok()).collect())
            .unwrap_or_default();
        scales.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let current = dir.as_ref().and_then(|d| read_f64(d.join("in_illuminance_scale"))).unwrap_or(0.1);
        let index = scales.iter().position(|s| (s - current).abs() < 1e-6).unwrap_or(0);
        if scales.is_empty() {
            scales.push(current);
        }
        // Proximity: the driver starts at its least sensitive setting (gain
        // 1, 0.185 ms), where it reads 0 even with a hand on the glass. Most
        // sensitive gain, 11.84 ms and the IR LED at full power: about 7000
        // counts at rest (sd about 5 in a calm room); a hand in front of the
        // panel: PROXIMITY_CURVE.
        if let Some(d) = dir.as_ref() {
            let _ = fs::write(d.join("in_proximity_scale"), PROXIMITY_SCALE);
            let _ = fs::write(d.join("in_proximity_integration_time"), PROXIMITY_INTEGRATION);
            set_proximity_led(d);
        }
        Self { dir, scales, index, last_lux: None }
    }

    /// Ambient light in lux. The sensor saturates at 65535 counts (about
    /// 6500 lx at the most sensitive scale), so the scale is switched
    /// automatically: coarser when saturated, finer when the reading is low.
    pub fn lux(&mut self) -> Option<f64> {
        let dir = self.dir.clone()?;
        let raw = read_f64(dir.join("in_illuminance_raw"))?;
        let scale = self.scales[self.index];
        let mut new_index = self.index;
        if raw >= 60000.0 && self.index + 1 < self.scales.len() {
            new_index += 1;
        } else if raw < 400.0 && self.index > 0 {
            new_index -= 1;
        }
        if new_index != self.index {
            let value = format!("{}", self.scales[new_index]);
            if fs::write(dir.join("in_illuminance_scale"), value).is_ok() {
                self.index = new_index;
            }
        }
        let lux = raw * scale;
        // A saturated reading only says "at least this bright".
        self.last_lux = Some(lux);
        self.last_lux
    }

    /// Proximity counts (readings in quick succession repeat the last
    /// conversion, so one reading per call).
    pub fn proximity(&self) -> Option<f64> {
        read_f64(self.dir.as_ref()?.join("in_proximity_raw"))
    }

    /// Proximity integration time in seconds.
    pub fn proximity_integration(&self) -> Option<f64> {
        read_f64(self.dir.as_ref()?.join("in_proximity_integration_time"))
    }

    /// Sets the proximity integration time (seconds, one of
    /// `in_proximity_integration_time_available`).
    pub fn set_proximity_integration(&self, seconds: &str) {
        if let Some(d) = self.dir.as_ref() {
            if let Err(e) = fs::write(d.join("in_proximity_integration_time"), seconds) {
                eprintln!("nspanel-app: proximity: integration time {seconds}: {e}");
            }
        }
    }
}

/// A hand in front of the panel centre: distance in cm and proximity signal
/// (counts above the resting level) at PROXIMITY_CURVE_INTEGRATION with the
/// IR LED at full power. Measured with a ruler in three rounds (one at
/// 2.96 ms, scaled by its resting level ratio 3.87); means where rounds
/// overlap. Single rounds differed by up to 30 % at 45-50 cm.
const PROXIMITY_CURVE: [(f64, f64); 11] = [
    (10.0, 6629.0),
    (15.0, 2485.0),
    (20.0, 1406.0),
    (25.0, 771.0),
    (30.0, 433.0),
    (35.0, 271.0),
    (40.0, 183.0),
    (45.0, 114.0),
    (50.0, 66.0),
    (55.0, 38.0),
    (60.0, 26.0),
];
const PROXIMITY_CURVE_INTEGRATION: f64 = 0.01184;

/// Segment of PROXIMITY_CURVE as (distance, log signal) pairs.
fn curve_segment(i: usize) -> ((f64, f64), (f64, f64)) {
    let ((d0, s0), (d1, s1)) = (PROXIMITY_CURVE[i], PROXIMITY_CURVE[i + 1]);
    ((d0, s0.ln()), (d1, s1.ln()))
}

/// Rough distance in cm of a hand in front of the panel centre for a
/// proximity signal. The signal is taken to grow in proportion to the
/// integration time; between the points the logarithm of the signal is
/// interpolated linearly, beyond them the nearest segment continues.
pub fn proximity_distance_cm(signal: f64, integration: f64) -> Option<f64> {
    if signal <= 0.0 || integration <= 0.0 {
        return None;
    }
    let s = (signal * PROXIMITY_CURVE_INTEGRATION / integration).ln();
    let last = PROXIMITY_CURVE.len() - 2;
    let i = (0..last).find(|&i| s >= PROXIMITY_CURVE[i + 1].1.ln()).unwrap_or(last);
    let ((d0, l0), (d1, l1)) = curve_segment(i);
    Some((d0 + (s - l0) * (d1 - d0) / (l1 - l0)).clamp(1.0, 99.0))
}

/// Proximity signal of a hand at `cm` in front of the panel centre (the
/// inverse of proximity_distance_cm).
pub fn proximity_signal(cm: f64, integration: f64) -> f64 {
    let last = PROXIMITY_CURVE.len() - 2;
    let i = (0..last).find(|&i| cm <= PROXIMITY_CURVE[i + 1].0).unwrap_or(last);
    let ((d0, l0), (d1, l1)) = curve_segment(i);
    (l0 + (cm - d0) * (l1 - l0) / (d1 - d0)).exp() * integration / PROXIMITY_CURVE_INTEGRATION
}

/// Proximity sensor setting (IIO `in_proximity_scale`, most sensitive gain).
const PROXIMITY_SCALE: &str = "0.1";
/// Proximity integration time in seconds (IIO `in_proximity_integration_time`):
/// the longest that makes a difference, 23.68 ms and above read the same as
/// 11.84 ms on this sensor. From 2.96 ms the resting level and a hand's
/// signal grow about 3.9 times, the noise only about 1.3 times.
pub const PROXIMITY_INTEGRATION: &str = "0.011840";
/// STK3310 LEDCTRL (register 0x03): IR LED current in bits 7:6 (12.5, 25, 50
/// or 100 mA) and pulse duty in bits 5:0, as in Sensortek's STK331x register
/// map. A running panel reads 0xAC (50 mA); 0xFF gives about 1.65 times the
/// reflection from a hand at 10-20 cm with the same noise. The IIO driver has
/// no control for it and never writes it, so the value stays until power off.
const PROXIMITY_LED: &str = "0xff";

/// Sets the proximity sensor's IR LED through i2c-dev (busybox i2cset). Bus
/// and address come from the IIO device's place in sysfs
/// (.../i2c-2/2-0047/iio:device0).
fn set_proximity_led(iio: &Path) {
    let client = fs::canonicalize(iio)
        .ok()
        .and_then(|p| p.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()));
    let Some((bus, address)) = client.as_deref().and_then(|c| c.split_once('-')) else {
        eprintln!("nspanel-app: proximity: no I2C address for {}", iio.display());
        return;
    };
    let address = format!("0x{address}");
    match std::process::Command::new("i2cset").args(["-f", "-y", bus, &address, "0x03", PROXIMITY_LED]).status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("nspanel-app: proximity: LED setting failed ({status})"),
        Err(e) => eprintln!("nspanel-app: proximity: LED setting failed ({e})"),
    }
}

/// Wake distance in cm: "something is close" above the resting level plus
/// the signal of a hand at this distance (PROXIMITY_CURVE)...
pub const PROXIMITY_WAKE_CM: f64 = 40.0;
/// The wake distances the integration offers (near, medium, far), cm. Far
/// stays at 45 cm: beyond it single measuring rounds differed by up to 30 %
/// and the signal comes close to the noise.
pub const PROXIMITY_WAKE_CHOICES: [(&str, f64); 3] = [("near", 30.0), ("medium", PROXIMITY_WAKE_CM), ("far", 45.0)];
/// ...or plus this many times the jitter (mean change between consecutive
/// resting readings), if that is more: noisy readings shorten the range
/// instead of waking the panel. (A fixed margin woke the panel by itself
/// while single readings strayed up to ±200, sd 80.)
const PROXIMITY_JITTER_FACTOR: f64 = 5.0;
/// Jitter assumed at the start, until readings have shown the real one.
const PROXIMITY_START_JITTER: f64 = 30.0;
/// Readings in a row above the threshold (half a second apart) after which
/// whatever is in front of the sensor counts as the new resting level.
const PROXIMITY_RELEARN: u32 = 120;
/// Between half the margin and the threshold the resting level follows the
/// readings only this much per reading (about 4 minutes to settle).
const PROXIMITY_HOLD_FOLLOW: f64 = 0.002;

/// Learns the resting proximity level and its jitter and reports an approach
/// after two consecutive readings (half a second apart) above the threshold.
///
/// The jitter comes from the change between consecutive readings, not from
/// their distance to the resting level: a slow approach changes each reading
/// only a little, so it does not raise the threshold in front of the hand.
/// Readings above half the margin teach nothing either: a hand held or moved
/// just below the threshold would otherwise pull it up.
pub struct ProximityWake {
    rest: Option<f64>,
    jitter: f64,
    /// The last reading, if it was a resting one.
    previous: Option<f64>,
    above: u32,
    /// Signal of a hand at the wake distance.
    wake_signal: f64,
    /// The integration time the signals are for (seconds).
    integration: f64,
}

impl Default for ProximityWake {
    fn default() -> Self {
        Self::new(PROXIMITY_INTEGRATION.parse().unwrap_or(PROXIMITY_CURVE_INTEGRATION))
    }
}

impl ProximityWake {
    /// For the given proximity integration time (seconds).
    pub fn new(integration: f64) -> Self {
        Self {
            rest: None,
            jitter: PROXIMITY_START_JITTER,
            previous: None,
            above: 0,
            wake_signal: proximity_signal(PROXIMITY_WAKE_CM, integration),
            integration,
        }
    }

    /// The wake distance (cm, on the measured curve).
    pub fn set_wake_cm(&mut self, cm: f64) {
        let signal = proximity_signal(cm, self.integration);
        if (signal - self.wake_signal).abs() > 0.01 {
            eprintln!("nspanel-app: proximity: wake distance {cm:.0} cm (signal {signal:.0})");
            self.wake_signal = signal;
        }
    }

    /// The wake distance calibration: the resting level and the jitter measured from readings taken with nothing in
    /// front, instead of the learned ones. The resting level is their median; the jitter is where the learning in
    /// `update` settles for their consecutive changes (the largest tenth left out, so one stray reading does not
    /// decide). False, and nothing changes, with fewer than 8 readings or when a reading lies above half the
    /// threshold that results (something moved in front meanwhile).
    pub fn calibrate(&mut self, readings: &[f64]) -> bool {
        if readings.len() < 8 {
            return false;
        }
        let mut sorted = readings.to_vec();
        sorted.sort_by(f64::total_cmp);
        let rest = sorted[sorted.len() / 2];
        let mut steps: Vec<f64> = readings.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        steps.sort_by(f64::total_cmp);
        steps.truncate(steps.len() - steps.len() / 10);
        let jitter = settled_jitter(&steps);
        let margin = (jitter * PROXIMITY_JITTER_FACTOR).max(self.wake_signal);
        if readings.iter().any(|&r| r > rest + margin / 2.0) {
            return false;
        }
        eprintln!("nspanel-app: proximity: calibrated, rest {rest:.0}, jitter {jitter:.1} (were {:.0}, {:.1})", self.rest.unwrap_or(0.0), self.jitter);
        self.rest = Some(rest);
        self.jitter = jitter;
        self.previous = None;
        self.above = 0;
        true
    }

    /// How far a hand wakes the panel now (cm): the wake distance, or less when the jitter sets the threshold.
    pub fn reach_cm(&self) -> Option<f64> {
        proximity_distance_cm(self.margin(), self.integration)
    }

    /// Someone (or something) is close to the panel right now.
    pub fn near(&self) -> bool {
        self.above >= 2
    }

    /// Counts above the resting level that count as "close".
    pub fn margin(&self) -> f64 {
        (self.jitter * PROXIMITY_JITTER_FACTOR).max(self.wake_signal)
    }

    pub fn rest(&self) -> Option<f64> {
        self.rest
    }

    pub fn jitter(&self) -> f64 {
        self.jitter
    }

    pub fn update(&mut self, value: f64) -> bool {
        let rest = *self.rest.get_or_insert(value);
        let margin = self.margin();
        if value > rest + margin {
            self.above = self.above.saturating_add(1);
            self.previous = None;
            if self.above == 2 {
                eprintln!("nspanel-app: proximity: approach ({value:.0} counts, rest {rest:.0}, margin {margin:.0})");
            }
            if self.above > PROXIMITY_RELEARN {
                eprintln!("nspanel-app: proximity: new resting level {value:.0} (was {rest:.0})");
                self.rest = Some(value);
                self.above = 0;
            }
        } else if value > rest + margin / 2.0 {
            // Maybe something in front (a hand held still or moving below
            // the threshold): no learning from it, or the threshold would
            // run away from the hand. Only a very slow follow, for a resting
            // level that really changed.
            self.above = 0;
            self.previous = None;
            self.rest = Some(rest + (value - rest) * PROXIMITY_HOLD_FOLLOW);
        } else {
            self.above = 0;
            // The jitter estimate rises quickly (a noisier sensor at once
            // raises the threshold) and falls slowly; the resting level
            // follows slow changes (daylight, dust, temperature).
            if let Some(previous) = self.previous {
                let step = (value - previous).abs();
                let k = if step > self.jitter { 0.1 } else { 0.01 };
                self.jitter += (step - self.jitter) * k;
            }
            self.previous = Some(value);
            self.rest = Some(rest * 0.98 + value * 0.02);
        }
        self.above == 2
    }
}

/// The jitter the learning in `ProximityWake::update` settles at for these changes between consecutive readings: it
/// rises by a tenth of a larger change and falls by a hundredth of a smaller one, so it settles where both balance.
fn settled_jitter(steps: &[f64]) -> f64 {
    let pull = |j: f64| steps.iter().map(|&s| if s > j { 0.1 * (s - j) } else { -0.01 * (j - s) }).sum::<f64>();
    let (mut lo, mut hi) = (0.0, steps.iter().copied().fold(0.0, f64::max));
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if pull(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

/// The darkest backlight level used, per mille: 0.8 %,
/// still readable at night and without flicker.
pub const BACKLIGHT_MIN_PERMILLE: f64 = 8.0;
/// Perceived brightness follows the PWM duty roughly with this exponent.
pub const BACKLIGHT_GAMMA: f64 = 2.2;

/// Backlight level (per mille) for a perceived brightness 0..1, where 0 is
/// the darkest level used: equal steps look equal.
pub fn perceived_permille(p: f64) -> u32 {
    let p_min = (BACKLIGHT_MIN_PERMILLE / 1000.0).powf(1.0 / BACKLIGHT_GAMMA);
    let q = p_min + p.clamp(0.0, 1.0) * (1.0 - p_min);
    (q.powf(BACKLIGHT_GAMMA) * 1000.0).round() as u32
}

/// Backlight level (per mille) for an ambient light level: the darkest level
/// at 1 lx and below, full from 500 lx, in between evenly in perceived
/// brightness over the logarithm of the light (10 lx ≈ 16 %, 50 lx ≈ 41 %).
pub fn auto_brightness_permille(lux: f64) -> u32 {
    let t = (lux.max(1.0).ln() / 500f64.ln()).clamp(0.0, 1.0);
    perceived_permille(t)
}

/// Backlight level (per mille) for the manual setting 1..100 %.
pub fn manual_brightness_permille(percent: u8) -> u32 {
    perceived_permille((percent.clamp(1, 100) as f64 - 1.0) / 99.0)
}

/// Wi-Fi signal level in dBm (`iw dev wlan0 link`; rtw88 does not fill
/// /proc/net/wireless).
pub fn wifi_dbm() -> Option<f64> {
    let out = std::process::Command::new("iw").args(["dev", "wlan0", "link"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.trim_start().starts_with("signal:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Speaker volume: the RK809 codec's "Master Playback Volume" (0..255 =
/// -95..0 dB, 0.375 dB per step). 100 % = 0 dB, every percent 0.4 dB less,
/// 0 % = off. (The boot default is 200 = -20.6 dB, about 48 %.)
pub fn set_volume(percent: u8) {
    let p = percent.min(100) as f64;
    let value = if p == 0.0 { 0 } else { (255.0 - (100.0 - p) * 0.4 / 0.375).round().clamp(1.0, 255.0) as i64 };
    let result = (|| -> alsa::Result<()> {
        let mixer = alsa::mixer::Mixer::new("hw:CARD=rk809", false)?;
        let id = alsa::mixer::SelemId::new("Master", 0);
        if let Some(selem) = mixer.find_selem(&id) {
            selem.set_playback_volume_all(value)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("nspanel-app: volume: {e}");
    }
}

pub fn soc_temperature() -> Option<f64> {
    Some(read_f64(PathBuf::from("/sys/class/thermal/thermal_zone0/temp"))? / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Readings around `rest` that change by `step` from one to the next.
    fn readings(rest: f64, step: f64) -> Vec<f64> {
        (0..20).map(|i| rest + if i % 2 == 0 { step / 2.0 } else { -step / 2.0 }).collect()
    }

    #[test]
    fn wake_distance_sets_the_threshold() {
        let mut p = ProximityWake::new(PROXIMITY_CURVE_INTEGRATION);
        // a calm room: the threshold is the signal of a hand at the wake distance
        assert!(p.calibrate(&readings(7000.0, 5.0)));
        assert!((p.jitter() - 5.0).abs() < 0.01);
        assert!((p.margin() - 183.0).abs() < 1.0);
        assert!((p.reach_cm().unwrap() - 40.0).abs() < 0.5);
        p.set_wake_cm(30.0);
        assert!((p.margin() - 433.0).abs() < 1.0);
        p.set_wake_cm(55.0);
        assert!((p.reach_cm().unwrap() - 55.0).abs() < 0.5);
        // a noisy room shortens the reach
        assert!(p.calibrate(&readings(7000.0, 20.0)));
        assert!((p.margin() - 100.0).abs() < 0.1);
        let reach = p.reach_cm().unwrap();
        assert!(reach > 45.0 && reach < 47.0);
    }

    #[test]
    fn calibration_refuses_a_moving_hand() {
        let mut p = ProximityWake::new(PROXIMITY_CURVE_INTEGRATION);
        let mut r = readings(7000.0, 5.0);
        r[9] += 600.0;
        assert!(!p.calibrate(&r));
        assert_eq!(p.rest(), None);
        assert!(!p.calibrate(&r[..6]));
    }

    #[test]
    fn calibrated_rest_wakes_above_it() {
        let mut p = ProximityWake::new(PROXIMITY_CURVE_INTEGRATION);
        assert!(p.calibrate(&readings(7000.0, 5.0)));
        assert!(!p.update(7050.0));
        assert!(!p.update(7200.0));
        assert!(p.update(7200.0));
        assert!(p.near());
    }
}
