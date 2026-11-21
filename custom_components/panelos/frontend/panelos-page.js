// The PanelOS page in Home Assistant's sidebar: every panel's settings beside
// a live picture of its screen, in PanelOS's own look (brand guide: Signal
// red only for what is lit or active, Night / Graphite / Paper grounds, Ink
// text on light, Space Grotesk headings, Noto Sans text).
//
// Shell: left navigation (wide), top bar with section pills (medium), top bar
// with a bottom bar and a floating mini screen (narrow). Each PanelOS entry is
// one panel; what a panel can do (its reported "caps") decides which
// sections, page types, looks and options it is offered. Critical alerts and
// status screens are shared by every panel. Data and changes go over the
// websocket commands in page.py (panelos/page/*).

// the live preview: the next picture is asked for this long after the last one arrived (the panel's own
// pace sets the rate: about 5-10 a second); slower while the browser tab is hidden
const PICTURE_EVERY_MS = 60;
const PICTURE_IDLE_MS = 1500;
const PICTURE_AFTER_CHANGE_MS = 700;
const ALL_PICTURES_EVERY_MS = 10000;
// the live preview stops after this long without use of the page (the last picture stays) and goes on with the next use
const PREVIEW_IDLE_MS = 60000;
const USE_EVENTS = ["pointermove", "pointerdown", "wheel", "keydown", "touchstart"];
const SYNC_FALLBACK_MS = 3000;

const DEFAULT_CAPS = ["touch", "images", "web", "video", "mic", "speaker", "proximity", "light"];
const CAP_NAMES = {
  touch: "Touch screen", images: "Pictures and animation", web: "Web view", video: "Live video",
  mic: "Microphone", speaker: "Speaker", proximity: "Proximity sensor", light: "Light sensor",
  temp: "Temperature sensor", humidity: "Humidity sensor", rotate: "Turns its screen",
  relays: "Built-in relays", buttons: "Physical buttons", update: "Software updates", storage: "Storage overview",
};
// Why something is missing, in plain words ({p} = the panel's name).
const NEEDS_REASON = {
  images: "{p}'s screen shows text and simple shapes only.",
  web: "{p} has no web view.",
  video: "{p} cannot play live video.",
  mic: "{p} has no microphone.",
  speaker: "{p} has no speaker.",
  proximity: "{p} has no proximity sensor.",
  light: "{p} has no light sensor.",
  fixed: "{p}'s screen has a fixed set of pages.",
};

// The page types (PAGE_KINDS in const.py). "home" is shown as Weather.
const PAGE_TYPES = {
  home: { name: "Custom", icon: "weather-partly-cloudy", desc: "Clock, weather and your widgets on a living background.", needs: [] },
  quick: { name: "Quick controls", icon: "toggle-switch-outline", desc: "Lights, shutters and devices at a tap.", needs: [] },
  music: { name: "Music", icon: "music", desc: "What plays, with the controls.", needs: [] },
  timers: { name: "Timers and alarms", icon: "timer-outline", desc: "Timers, alarms and a stopwatch.", needs: ["speaker"] },
  thermostat: { name: "Thermostat", icon: "thermostat", desc: "Heating and cooling, room by room.", needs: [] },
  camera: { name: "Cameras", icon: "cctv", desc: "Live view and the visitor alert.", needs: ["video"] },
  todo: { name: "To-do list", icon: "format-list-checks", desc: "Shopping and task lists.", needs: ["images"] },
  calendar: { name: "Calendar", icon: "calendar-month", desc: "Day, week and month, with details.", needs: ["images"] },
  info: { name: "Notifications", icon: "bell-outline", desc: "Messages from Home Assistant.", needs: [] },
  clock: { name: "Clock", icon: "clock-outline", desc: "A big clock face.", needs: [] },
  weather: { name: "Weather details", icon: "weather-cloudy-clock", desc: "Today and the coming days in detail.", needs: [] },
};
const TYPE_ORDER = ["home", "quick", "music", "timers", "thermostat", "camera", "todo", "calendar", "info", "clock", "weather"];

// Looks per page type, in the panel's order (index = the page's variant).
const LOOKS = {
  weather: ["Cards", "Living photo"],
  clock: ["Digital", "Big digits", "Round", "Square", "Minimal", "Numerals", "Wandering", "Split-flap", "Nixie", "LCD quartz", "Twin-bell"],
  music: ["Plain", "Full cover", "Turntable", "Cassette", "Half cassette"],
  timers: ["Plain", "Kitchen timer", "Hourglass", "Synthwave", "Flip clock"],
  thermostat: ["Dial", "Remote control", "Wall thermostat", "Synthwave"],
  quick: ["Tiles", "Mixing desk", "Flight deck"],
  camera: ["Camera"],
  calendar: ["Plain", "Departure board", "Tear-off", "Wall calendar"],
  todo: ["Plain", "Receipt", "Terminal", "Label maker", "Notepad"],
  info: ["List", "Fridge notes", "Pager", "TV news"],
};
// Looks beyond the first that need pictures on the panel's screen.
const LOOK_NEEDS_IMAGES = new Set(["weather", "music", "thermostat", "quick", "info"]);

// Custom page backgrounds: options.theme 0..9, synthwave (8) with options.scene 0..3; plain (9) is one
// colour, the panel's own ground by its theme (Night #000000 dark, Paper #F5F3EE light), and needs no pictures.
const BACKGROUNDS = [
  { name: "Photo · nature", theme: 0 }, { name: "Photo · countryside", theme: 1 }, { name: "Photo · city", theme: 2 },
  { name: "Drawn · countryside", theme: 3 }, { name: "Drawn · city", theme: 4 }, { name: "Drawn · neutral", theme: 5 },
  { name: "Drawn · big city", theme: 6 }, { name: "Line drawing", theme: 7 },
  { name: "Synthwave · mountains", theme: 8, scene: 0 }, { name: "Synthwave · city", theme: 8, scene: 1 },
  { name: "Synthwave · beach", theme: 8, scene: 2 }, { name: "Synthwave · space", theme: 8, scene: 3 },
  { name: "Plain", theme: 9 },
];
const PLAIN_THEME = 9;
const SW_SCENES = ["Mountains", "City", "Beach", "Space"];
// Turntable wood (music options.wood): the panel's deck pictures.
const WOODS = [
  { id: "walnut", name: "Walnut", c: "#5b3a24" }, { id: "oak", name: "Oak", c: "#a87a46" }, { id: "cherry", name: "Cherry", c: "#8c4a2f" },
  { id: "beech", name: "Beech", c: "#c99a6b" }, { id: "pine", name: "Pine", c: "#d8b27a" }, { id: "pear", name: "Pear", c: "#b98560" },
  { id: "scandi-oak", name: "Scandi oak", c: "#dcc39b" },
];
// Pager look finish (info options.finish).
const PAGER_FINISH = [
  { id: "graphite", name: "Graphite", c: "#2e3033" }, { id: "purple", name: "Purple", c: "#6c3fb3" },
  { id: "clear", name: "Clear", c: "linear-gradient(135deg,#c9d6df,#8fa3b0)" }, { id: "offwhite", name: "Off-white", c: "#d5d0c3" },
];
const TIMER_FUNCTIONS = [["timer", "Timers"], ["alarm", "Alarms"], ["stopwatch", "Stopwatch"]];
// the stopwatch's and the alarms' own looks (spec 24)
const SW_LOOKS = [[0, "Plain"], [1, "Chronograph"], [2, "Timing tower"], [3, "LCD watch"], [4, "Nixie"]];
const ALARM_LOOKS = [[0, "Plain"], [1, "Twin-bell"], [2, "Sunrise"], [3, "Clock radio"], [4, "24-hour dial"]];
const VOICE_LOOKS = ["Plain", "Terminal", "Comic", "Oscilloscope"];
// Option ids of the selects Home Assistant makes for a voice satellite, by the names Home Assistant shows for them
const VOICE_OPTION_NAMES = { preferred: "Preferred", default: "Default", relaxed: "Relaxed", aggressive: "Aggressive", no_wake_word: "No wake word" };

// Sounds. Stored values: "auto" (the place's default), "lib:<id>" (a PanelOS sound, {STATIC}/sounds/<id>.mp3
// here), "own:<id>" (uploaded once for every panel, house.sounds). The last two PanelOS sounds are for
// critical alerts only.
const SOUND_LIB = [
  { id: "chimes", name: "Glass chimes" }, { id: "marimba", name: "Marimba" }, { id: "birds", name: "Birds" },
  { id: "bell", name: "Mechanical bell" }, { id: "digital", name: "Digital" }, { id: "synth", name: "Synth" },
  { id: "sos", name: "SOS", crit: true }, { id: "warning", name: "Warning tone", crit: true },
];
// The default sound of each timers look (index = the page's variant).
const TIMER_LOOK_SOUND = ["marimba", "bell", "chimes", "synth", "digital"];
const SOUND_UPLOAD_MAX = 10 * 1024 * 1024;
const SOUND_TYPES = /\.(mp3|wav|ogg|oga|flac)$/i;
const ALARMS_MAX = 20;
// The tiles a Quick controls page shows (the panel's grid, desk and deck take eight).
const TILES_MAX = 8;
const DAY_NAMES = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
// What can start an own critical alert.
const OWN_CRIT_DOMAINS = ["binary_sensor", "sensor", "lock", "input_boolean", "switch"];

// Widgets of a weather page (options.items, a 4 x 4 grid), as the panel
// draws them (app/ui/home.slint, HomeItem): {kind, col, row, w, h, style,
// accent "#rrggbb" ("" = the panel's accent), weight, tap, opts}.
// kind: 0 clock, 1 date, 2 weather now, 3 next event, 4 forecast curve,
//   5 hourly forecast, 6 daily forecast, 7 music, 8 a sensor's value (its
//   sensor in "entity"), 9 the running timer, 10 the people at home (the
//   ones it shows in "people", person ids; none: everyone), 11 weather details.
// style: clock 0 digital, 1 big digits + seconds line, 2 round, 3 square,
//   4 minimal, 5 numerals, 6 split-flap, 7 the time in words, 8 nixie, 9 LCD quartz, 10 twin-bell; music: the
//   scroll speed of long titles 1..5 (with opts bit 1); others 0.
// opts (bits): clock 1 seconds, 2 date, 4 weather, 8 next event; date 1 year,
//   2 week number; weather now 1 condition, 2 high / low, 4 rain chance,
//   8 feels like; next event 1 time, 2 calendar colour; curve 1 rain bars,
//   2 icons; hourly / daily 1 rain chance; music 1 long titles scroll;
//   sensor 1 name, 2 icon; timer 1 name; people 1 the ones away too, 2 initials
//   only (without it: the profile pictures);
//   weather details 1 wind, 2 humidity, 4 chance of rain, 8 sunrise and sunset.
// The weather, calendars and player of a weather page are its entities (the
// first weather.*, every calendar.*, the first media_player.*).
const WIDGETS = {
  0: { name: "Clock", icon: "clock-outline", min: [2, 1], def: [2, 1], style: 1, opts: 1, bits: [[1, "Seconds"], [2, "Date"], [4, "Weather"], [8, "Next event"]] },
  1: { name: "Date", icon: "calendar-today", min: [2, 1], def: [2, 1], opts: 0, bits: [[1, "Year"], [2, "Week number"]] },
  2: { name: "Weather now", icon: "weather-partly-cloudy", min: [1, 1], def: [2, 1], opts: 1, bits: [[1, "Condition"], [2, "High and low"], [4, "Chance of rain"], [8, "Feels like"]] },
  3: { name: "Next event", icon: "calendar-clock", min: [2, 1], def: [4, 1], opts: 3, bits: [[1, "Time"], [2, "Calendar colour"]] },
  4: { name: "Forecast curve", icon: "chart-bell-curve-cumulative", min: [3, 1], def: [4, 1], opts: 3, bits: [[1, "Rain bars"], [2, "Icons"]] },
  5: { name: "Hourly forecast", icon: "clock-time-four-outline", min: [3, 1], def: [4, 1], opts: 1, bits: [[1, "Chance of rain"]] },
  6: { name: "Daily forecast", icon: "calendar-week", min: [3, 1], def: [4, 1], opts: 1, bits: [[1, "Chance of rain"]] },
  7: { name: "Music", icon: "music", min: [1, 1], def: [4, 1], opts: 0, bits: [] },
  8: { name: "Sensor", icon: "gauge", min: [1, 1], def: [2, 1], opts: 3, bits: [[1, "Name"], [2, "Icon"]], notap: true },
  9: { name: "Timer", icon: "timer-outline", min: [1, 1], def: [2, 1], opts: 1, bits: [[1, "Name"]], nocolour: true },
  10: { name: "People", icon: "account-group-outline", min: [1, 1], def: [2, 1], opts: 1, bits: [[1, "The ones away too"]], notap: true, noweight: true, nocolour: true },
  11: { name: "Details", icon: "weather-windy", min: [2, 1], def: [4, 1], opts: 15, bits: [[1, "Wind"], [2, "Humidity"], [4, "Chance of rain"], [8, "Sunrise and sunset"]] },
};
// The widgets a Weather details page offers (the weather ones), in the order of its "Add a widget" sheet.
const WEATHER_WIDGETS = [2, 11, 4, 5, 6];
const CLOCK_STYLES = ["Digital", "Big digits", "Round", "Square", "Minimal", "Numerals", "Split-flap", "In words", "Nixie", "LCD quartz", "Twin-bell"];
// The smallest size of a widget in cells [w, h]: the nixie clock needs 3 x 1 (4 x 1 with seconds), the LCD 2 x 1 (its
// case shows from 3 x 2), the twin-bell 2 x 2; other widgets as WIDGETS says.
function widgetMin(wd) {
  if (wd.kind === 0) {
    if (wd.style === 8) return [((wd.opts || 0) & 1) ? 4 : 3, 1];
    if (wd.style === 9) return [2, 1];
    if (wd.style === 10) return [2, 2];
  }
  return (WIDGETS[wd.kind] || WIDGETS[0]).min;
}
// The letter weights the panel draws (clocks, widgets, the screensaver): one slider over these four
// everywhere, live on the panel while it moves.
const WEIGHTS = [[300, "Light"], [400, "Regular"], [600, "Semibold"], [800, "Bold"]];
// The screensaver's clock faces (the panel's ClockFace styles 0..10; the clock page's looks are the same faces).
const SAVER_FACES = ["Big clock", "Huge digits", "Round", "Square", "Minimal", "Numerals", "Wandering", "Split-flap", "Nixie", "LCD quartz", "Twin-bell"];
// The panel's own layout when a weather page has none (as the panel draws it).
const DEFAULT_ITEMS = [
  { kind: 0, col: 0, row: 0, w: 4, h: 1, style: 1, opts: 1 },
  { kind: 4, col: 0, row: 1, w: 4, h: 1, style: 0, opts: 3 },
  { kind: 7, col: 0, row: 3, w: 4, h: 1, style: 0, opts: 0 },
];
// … and a Weather details page's (the panel's weather_items): the weather now, the details, the hours, the days.
const WEATHER_DEFAULT_ITEMS = [
  { kind: 2, col: 0, row: 0, w: 4, h: 1, style: 0, opts: 11, tap: false },
  { kind: 11, col: 0, row: 1, w: 4, h: 1, style: 0, opts: 15, tap: false },
  { kind: 5, col: 0, row: 2, w: 4, h: 1, style: 0, opts: 1, tap: false },
  { kind: 6, col: 0, row: 3, w: 4, h: 1, style: 0, opts: 1, tap: false },
];
const ACCENTS = ["#FF4A1C", "#F5F3EE", "#FFB020", "#3CCB7F", "#2E9BFF", "#B06CFF", "#FF5C9A"];
// Timers: the panel's own colour for the 1st … 6th timer, and the colours to choose from.
const TIMER_TINTS = ["#ff5a36", "#ffb21e", "#3d9bff", "#2fd27a", "#c86bfa", "#ff4fa0"];
const TIMER_PALETTE = ["#ff5a36", "#FF4A1C", "#ffb21e", "#ffd319", "#2fd27a", "#01ffc3", "#00e5ff", "#3d9bff", "#8a5cff", "#c86bfa", "#ff4fa0", "#F5F3EE"];

// Status screen kinds (the panel has its own texts and pictures for each).
const STATUS_CATS = [
  { id: "occupied", name: "Occupied", c: "#d9480f", pictos: ["toilet-sitter", "stop-sign"], mdi: "toilet" },
  { id: "aftermath", name: "Air recovery", c: "#5c7f0a", pictos: ["gas-mask", "toilet-fumes"], mdi: "fan", countdown: true },
  { id: "bathroom", name: "Bathroom in use", c: "#1971c2", pictos: ["shower-person", "wet-floor"], mdi: "shower" },
  { id: "meeting", name: "On a call", c: "#3b5bdb", pictos: ["on-air", "headset"], mdi: "headset" },
  { id: "focus", name: "Focus time", c: "#6741d9", pictos: ["door-hanger", "quiet-face"], mdi: "headset" },
  { id: "sleeping", name: "Someone is sleeping", c: "#1e2b4d", pictos: ["moon-zzz", "crib"], mdi: "sleep" },
  { id: "movie", name: "Movie night", c: "#a61e4d", pictos: ["popcorn", "clapper"], mdi: "popcorn" },
  { id: "gaming", name: "Gaming", c: "#087f5b", pictos: ["gamepad", "pixel-skull", "gta-vi"], mdi: "controller" },
  { id: "vacuum", name: "Robot vacuum at work", c: "#495057", pictos: ["robot-vacuum", "vacuum-sock"], mdi: "robot-vacuum" },
  { id: "cooking", name: "Cooking", c: "#b45309", pictos: ["chef-hat", "pan-flames"], mdi: "chef-hat" },
  { id: "workout", name: "Workout", c: "#c2255c", pictos: ["dumbbell", "kettlebell"], mdi: "dumbbell" },
  { id: "keepout", name: "Keep out", c: "#343a40", pictos: ["stop-hand", "skull-bones", "eye"], mdi: "hand-back-right" },
  { id: "custom", name: "Custom", c: "#0b7285", pictos: ["zip-face", "eye", "printer-3d"], mdi: "alert-outline" },
];
const STATUS_CAT = Object.fromEntries(STATUS_CATS.map((c) => [c.id, c]));
const STATUS_COLOURS = ["#d9480f", "#5c7f0a", "#1971c2", "#3b5bdb", "#6741d9", "#1e2b4d", "#a61e4d", "#087f5b", "#495057", "#b45309", "#c2255c", "#343a40", "#0b7285"];

// Critical alerts: suggested by device class.
const SAFETY = {
  smoke: { name: "Smoke", icon: "smoke-detector" }, carbon_monoxide: { name: "Carbon monoxide", icon: "molecule-co" },
  gas: { name: "Gas", icon: "gas-cylinder" }, moisture: { name: "Water leak", icon: "water-alert" },
  problem: { name: "Problem", icon: "alert-outline" }, tamper: { name: "Tamper", icon: "shield-alert-outline" },
  safety: { name: "Safety", icon: "alert-octagon-outline" },
};

const DND_ITEMS = [
  { id: "critical", name: "Critical alerts", locked: true },
  { id: "alarms", name: "Alarms" }, { id: "timers", name: "Timers" }, { id: "status", name: "Status screens" },
  { id: "ring", name: "Doorbell and camera ring" }, { id: "voice", name: "Voice answers when woken" },
  { id: "calls", name: "Calls and announcements" },
];
const DND_DEFAULT_ALLOW = { alarms: true, timers: true, status: true, ring: true, voice: true, calls: true };
const DEFAULT_SCREENSAVER = { when: "dark", lux: 3, from: "22:00", to: "06:00", light_entity: "", face: 1, seconds: true, date: true, weather: true, next: true, weight: 600, width: 100, speed: 20, colour: "" };
const DEFAULT_DND = { mode: "off", from: "22:00", to: "07:00", allow: DND_DEFAULT_ALLOW };
const DEFAULT_POPUPS = { kind: "card", sound: true, seconds: 10, until: false };
// what switches the Auto theme: the light in the room, the sun, or set times (dark from - to)
const DEFAULT_THEME_AUTO = { by: "light", from: "20:00", to: "07:00" };
const WAKE_DISTANCES = [["near", "Near"], ["medium", "Medium"], ["far", "Far"]];
// The cameras page's replies at the door (app/src/cam.rs DEFAULT_REPLIES): at most four, each one line on the panel,
// whose reply lines are 347 px wide in Noto Sans 500 15 px (app/ui/camera.slint).
const DOOR_REPLIES = ["We're coming, one moment", "Please leave it at the door", "We can't come to the door right now", "Who is it?"];
const DOOR_REPLIES_MAX = 4;
const DOOR_FONT = '500 15px "PanelOS Text", "Noto Sans", sans-serif';
const DOOR_LINE_PX = 347;

const SECTIONS = [
  { id: "overview", name: "Overview", icon: "monitor-dashboard" },
  { id: "pages", name: "Pages", icon: "view-grid-outline" },
  { id: "alerts", name: "Alerts", long: "Alerts and status", icon: "bell-outline" },
  { id: "voice", name: "Voice", icon: "account-voice", needs: "mic" },
  { id: "display", name: "Display", icon: "brightness-6" },
  { id: "device", name: "Device", icon: "cellphone-cog" },
];

const REPORTING = {
  illuminance_change: { label: "Light", now: "illuminance", unit: "lx" },
  soc_temperature_change: { label: "Chip temperature", now: "soc_temperature", unit: "°C" },
  wifi_signal_change: { label: "Wi-Fi signal", now: "wifi_signal", unit: "dBm" },
};

// Readable names of integrations, for entities with the same name.
const PLATFORMS = {
  met: "Met.no", open_meteo: "Open-Meteo", openweathermap: "OpenWeatherMap", accuweather: "AccuWeather",
  pirateweather: "Pirate Weather", music_assistant: "Music Assistant", cast: "Google Cast", esphome: "ESPHome",
  sonos: "Sonos", dlna_dmr: "DLNA", jellyfin: "Jellyfin", apple_tv: "Apple TV",
};
const platformName = (p) => p && (PLATFORMS[p] || p.charAt(0).toUpperCase() + p.slice(1).replace(/_/g, " "));

// Default icons per domain (when an entity has none of its own).
const DOMAIN_ICONS = {
  light: "lightbulb", switch: "toggle-switch-outline", cover: "window-shutter", fan: "fan", climate: "thermostat",
  media_player: "speaker", camera: "cctv", calendar: "calendar-month", todo: "format-list-checks", weather: "weather-partly-cloudy",
  sensor: "eye", binary_sensor: "checkbox-blank-circle-outline", input_boolean: "toggle-switch-outline", scene: "palette",
  script: "script-text-outline", vacuum: "robot-vacuum", lock: "lock-outline",
};
const SENSOR_ICONS = { temperature: "thermometer", humidity: "water-percent", illuminance: "brightness-6", carbon_dioxide: "molecule-co2", power: "flash", energy: "lightning-bolt", battery: "battery" };

// ---------------------------------------------------------------- DOM helpers

// Pictures shown once stay decoded in memory, and a new <img> of one is drawn with it at once: a page
// drawn again shows its pictures without a blank frame.
const KEPT_PICTURES = new Map();

function h(tag, attrs, ...children) {
  const node = document.createElement(tag);
  if (tag === "img") {
    node.decoding = "sync";
    const src = attrs?.src;
    // (the live screen pictures are blob: and data: URLs, new each time: not kept)
    if (src && !/^(data|blob):/.test(src) && !KEPT_PICTURES.has(src) && KEPT_PICTURES.size < 400) {
      const keep = new Image();
      keep.src = src;
      KEPT_PICTURES.set(src, keep);
    }
  }
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v == null || v === false) continue;
      if (k === "class") node.className = v;
      else if (k === "style" && typeof v === "object") {
        for (const [sk, sv] of Object.entries(v)) {
          if (sv == null) continue;
          if (sk.startsWith("--")) node.style.setProperty(sk, sv);
          else node.style[sk] = sv;
        }
      } else if (k.startsWith("on") && typeof v === "function") node.addEventListener(k.slice(2), v);
      else if (k === "value" && (tag === "input" || tag === "textarea")) node.value = v;
      else if (k === "checked" || k === "selected" || k === "disabled") node[k] = Boolean(v);
      else if (v === true) node.setAttribute(k, "");
      else node.setAttribute(k, v);
    }
  }
  append(node, children);
  return node;
}

function append(node, children) {
  for (const c of children) {
    if (c == null || c === false || c === "") continue;
    if (Array.isArray(c)) append(node, c);
    else node.append(c.nodeType ? c : document.createTextNode(String(c)));
  }
}

// An MDI icon (Home Assistant's own icon element).
function icon(name, cls) {
  const e = document.createElement("ha-icon");
  e.setAttribute("icon", name.includes(":") ? name : `mdi:${name}`);
  e.className = "mdi" + (cls ? ` ${cls}` : "");
  return e;
}

// An inline SVG from markup (look pictures).
function svgNode(markup, cls) {
  const wrap = document.createElement("div");
  wrap.className = cls || "art";
  wrap.innerHTML = markup;
  return wrap;
}

const clone = (x) => JSON.parse(JSON.stringify(x));

// ---------------------------------------------------------------- live widget previews (the widget picker)
// Home Assistant's weather conditions by the names the panel shows (app/src/pages.rs condition_text).
const WX_NAMES = { "clear-night": "Clear", "partlycloudy": "Partly cloudy", "lightning": "Thunder", "lightning-rainy": "Thunderstorm", "snowy-rainy": "Sleet", "windy-variant": "Windy", "pouring": "Pouring" };
const wxName = (c) => WX_NAMES[c] || (c ? c.charAt(0).toUpperCase() + c.slice(1) : "");
// The weather icons of the previews: line drawings on a 24-unit grid, in the text colour.
const WX_CLOUD = "M7.5 19h9.5a4 4 0 0 0 .4-7.98 5.5 5.5 0 0 0-10.6 1.2A3.4 3.4 0 0 0 7.5 19z";
const WX_SUN = '<circle cx="12" cy="12" r="4.2"/>' + [0, 45, 90, 135, 180, 225, 270, 315].map((a) => `<path d="M12 4.2V2" transform="rotate(${a} 12 12)"/>`).join("");
function wxIcon(cond, x, y, size, ink) {
  const up = `<path d="${WX_CLOUD}" transform="translate(0 -3.5)"/>`;
  const drops = (n) => [9, 13, 17, 11, 15].slice(0, n).map((dx, i) => `<path d="M${dx} ${17.5 + (i > 2 ? 2 : 0)}l-1 2.6"/>`).join("");
  const shapes = {
    "sunny": WX_SUN,
    "clear-night": '<path d="M15.5 3.6a8.4 8.4 0 1 0 4.9 13.3A7 7 0 0 1 15.5 3.6z"/>',
    "partlycloudy": `<g transform="translate(-3.2 -3.6) scale(.7)">${WX_SUN}</g><path d="${WX_CLOUD}" transform="translate(1.5 .5) scale(.92)"/>`,
    "cloudy": `<path d="${WX_CLOUD}" transform="translate(0 -1.5)"/>`,
    "rainy": up + drops(3),
    "pouring": up + drops(5),
    "snowy": up + [9, 13, 17].map((dx) => `<circle cx="${dx}" cy="19.5" r=".7"/>`).join(""),
    "snowy-rainy": up + '<circle cx="10" cy="19.5" r=".7"/><path d="M15 17.5l-1 2.6"/>',
    "hail": up + [9, 13, 17].map((dx) => `<circle cx="${dx}" cy="19" r="1.1"/>`).join(""),
    "lightning": up + '<path d="M13 15l-2 3.5h3l-2 3.5"/>',
    "lightning-rainy": up + '<path d="M12 15l-2 3.5h3l-2 3.5M16.5 16.5l-1 2.6"/>',
    "fog": '<path d="M4 9h16M6 13h14M4 17h14"/>',
    "windy": '<path d="M3 9h11a2.5 2.5 0 1 0-2.5-2.5M3 13h15a2.5 2.5 0 1 1-2.5 2.5M3 17h8"/>',
    "windy-variant": '<path d="M3 9h11a2.5 2.5 0 1 0-2.5-2.5M3 13h15a2.5 2.5 0 1 1-2.5 2.5M3 17h8"/>',
  };
  const body = shapes[cond] || shapes.cloudy;
  return `<g transform="translate(${x} ${y}) scale(${size / 24})" fill="none" stroke="${ink}" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">${body}</g>`;
}
// A text cut to a width at a font size (the panel elides its widget texts the same way).
function fitText(t, width, size) {
  const s = String(t || "");
  const most = Math.max(1, Math.floor(width / (size * 0.56)));
  return s.length <= most ? s : `${s.slice(0, Math.max(1, most - 1)).trimEnd()}…`;
}

// ---------------------------------------------------------------- jelly motion (spec 28.15)
// The switch, the segmented control, the weekday row and the checkbox move as on the panel. Each motion is a
// closed-form function of the time t since the change: J-edge(t; τ, T) = 1 − e^(−t/τ)·(cos ωt + sin ωt / (ωτ)),
// ω = 2π/T (it starts at rest, overshoots and rings out); a wobble that keeps the area, sx = 1 + A·e^(−t/τ)·cos(2πt/T),
// sy = 1/sx; an overshoot past a target softly capped at cap·tanh(x/cap).
const J_TAU = Math.PI * 2;
const jEdge = (t, tau, T) => (t <= 0 ? 0 : 1 - Math.exp(-t / tau) * (Math.cos((J_TAU * t) / T) + (T / (J_TAU * tau)) * Math.sin((J_TAU * t) / T)));
const jWob = (t, A, tau = 150, T = 170) => (t < 0 ? 1 : 1 + A * Math.exp(-t / tau) * Math.cos((J_TAU * t) / T));
const jSoft = (x, cap) => (x <= 0 ? x : cap * Math.tanh(x / cap));
const jEo = (u) => 1 - Math.pow(1 - Math.min(1, Math.max(0, u)), 3);
const J_LEAD = [90, 240];
const J_TRAIL = [130, 340];
const J_LAG = 30;
// reduced motion: no overshoot or wobble, 150 ms ease-out
const jReduce = () => Boolean(window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches);
const jScale = (el, sx, sy) => { if (el) el.style.transform = `scale(${sx.toFixed(4)}, ${sy.toFixed(4)})`; };
const J_TICK = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" pathLength="1"/></svg>';
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const mb = (bytes) => (bytes >= 1048576 ? `${(bytes / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`);
const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

// ---------------------------------------------------------------- look pictures
// Sheets that background updates leave alone (lists under the finger); and the picker's page of rows.
const STILL_SHEETS = new Set(["pick", "sound"]);
const PICK_PAGE = 80;

// Small drawings of the panel's looks (not screenshots): enough to tell them
// apart; the live preview shows the real thing.

let artId = 0;
const svgWrap = (body, bg) =>
  `<svg viewBox="0 0 120 120" preserveAspectRatio="xMidYMid slice" aria-hidden="true">${bg ? `<rect width="120" height="120" fill="${bg}"/>` : ""}${body}</svg>`;
function grad(stops, vertical = true) {
  const id = `pa${++artId}`;
  const s = stops.map(([o, c]) => `<stop offset="${o}" stop-color="${c}"/>`).join("");
  return [id, `<defs><linearGradient id="${id}" x1="0" y1="0" x2="${vertical ? 0 : 1}" y2="${vertical ? 1 : 0}">${s}</linearGradient></defs>`];
}
const txt = (x, y, size, fill, t, weight = 500, anchor = "middle") =>
  `<text x="${x}" y="${y}" font-size="${size}" fill="${fill}" font-family="PanelOS Heading, Space Grotesk, sans-serif" font-weight="${weight}" text-anchor="${anchor}">${t}</text>`;
const bars = (x, y, widths, fill, gap = 7, hgt = 3) => widths.map((w, i) => `<rect x="${x}" y="${y + i * gap}" width="${w}" height="${hgt}" rx="${hgt / 2}" fill="${fill}"/>`).join("");

function synthBase(scene) {
  const [g1, d1] = grad([[0, "#12002b"], [0.55, "#4b0f63"], [0.62, "#c2347a"], [0.63, "#1a0633"], [1, "#0b0018"]]);
  const [g2, d2] = grad([[0, "#ffd45a"], [1, "#ff3d9a"]]);
  let s = `${d1}${d2}<rect width="120" height="120" fill="url(#${g1})"/>`;
  if (scene === 3) {
    // space: a nebula, the ringed planet over cyan wireframe mountains, a violet grid
    const [g3, d3] = grad([[0, "#ffa8e2"], [0.5, "#ff6cc8"], [1, "#6a34d4"]]);
    s = `${d1}${d3}<rect width="120" height="120" fill="url(#${g1})"/><rect width="120" height="76" fill="#0b0634" opacity=".55"/>`;
    s += `<ellipse cx="30" cy="30" rx="36" ry="11" fill="#ff4fd8" opacity=".22" transform="rotate(12 30 30)"/><ellipse cx="88" cy="20" rx="30" ry="9" fill="#4a7bff" opacity=".2"/>`;
    s += [[12, 14], [30, 40], [52, 10], [88, 30], [104, 44], [70, 6], [18, 52], [96, 8], [40, 22], [112, 18]].map(([x, y]) => `<circle cx="${x}" cy="${y}" r="0.9" fill="#fff"/>`).join("");
    s += `<g transform="rotate(-11 60 64)"><ellipse cx="60" cy="64" rx="27" ry="8" fill="none" stroke="#ffe2f7" stroke-width="1.4"/></g><circle cx="60" cy="64" r="17" fill="url(#${g3})"/>`;
    s += `<g transform="rotate(-11 60 64)"><path d="M33 64 A27 8 0 0 0 87 64" fill="none" stroke="#ffe2f7" stroke-width="1.6"/></g>`;
    s += `<path d="M0 62 L14 56 L26 60 L40 66 L52 74 L60 76 L70 73 L84 70 L96 64 L108 66 L120 63 V76 H0Z" fill="#110730" stroke="#29f3ff" stroke-width="1"/>`;
  } else {
    s += `<circle cx="60" cy="58" r="22" fill="url(#${g2})"/>`;
    s += [56, 62, 67, 71].map((y, i) => `<rect x="36" y="${y}" width="48" height="${1 + i * 0.6}" fill="#4b0f63"/>`).join("");
  }
  if (scene === 0) s += `<path d="M0 75 L22 52 L38 66 L60 44 L80 64 L98 50 L120 72 V76 H0Z" fill="#240a40" stroke="#ff2bd6" stroke-width="1"/>`;
  if (scene === 1) s += `<path d="M0 76 V60 H10 V48 H20 V62 H28 V40 H40 V58 H48 V50 H60 V66 H66 V36 H78 V60 H86 V52 H98 V64 H108 V56 H120 V76Z" fill="#1a0633"/>` + [[14, 52], [32, 46], [34, 52], [70, 42], [72, 50], [90, 56], [112, 60]].map(([x, y]) => `<rect x="${x}" y="${y}" width="2" height="2" fill="#4ff0ff"/>`).join("");
  if (scene === 2) s += `<rect x="0" y="70" width="120" height="6" fill="#ff6fb5" opacity=".5"/><path d="M96 76 Q98 56 92 40" stroke="#14001f" stroke-width="3" fill="none"/><path d="M92 40 q-14 -2 -20 6 M92 40 q12 -6 22 0 M92 40 q-6 -10 -16 -10 M92 40 q4 -10 14 -10" stroke="#14001f" stroke-width="3" fill="none"/>`;
  s += `<g stroke="${scene === 3 ? "#b967ff" : "#ff2bd6"}" stroke-width="0.8" opacity=".75">${[78, 84, 92, 102, 116].map((y) => `<line x1="0" y1="${y}" x2="120" y2="${y}"/>`).join("")}${[-60, -20, 20, 60, 100, 140, 180].map((x) => `<line x1="60" y1="76" x2="${x}" y2="120"/>`).join("")}</g>`;
  return s;
}

// A Custom page background, by its BACKGROUNDS index. plain: "dark" / "light" = the panel's theme;
// otherwise (automatic or not known) both grounds, split.
function bgArt(i, plain = null) {
  const b = BACKGROUNDS[i] || BACKGROUNDS[1];
  if (b.theme === 8) return svgWrap(synthBase(b.scene));
  if (b.theme === PLAIN_THEME) {
    if (plain) return svgWrap("", plain === "dark" ? "#000000" : "#F5F3EE");
    return svgWrap(`<path d="M0 0 H120 L0 120 Z" fill="#000000"/><path d="M120 0 V120 H0 Z" fill="#F5F3EE"/>`);
  }
  switch (b.theme) {
    case 0: {
      const [g, d] = grad([[0, "#6fa8dc"], [1, "#d7e8f3"]]);
      return svgWrap(`${d}<rect width="120" height="120" fill="url(#${g})"/><circle cx="92" cy="28" r="9" fill="#fff6d5"/><path d="M0 78 Q30 58 60 72 T120 66 V120 H0Z" fill="#5d8a4a"/><path d="M0 92 Q40 76 80 90 T120 86 V120 H0Z" fill="#3f6b33"/><ellipse cx="44" cy="106" rx="30" ry="5" fill="#9cc2df" opacity=".85"/>`);
    }
    case 1: {
      const [g, d] = grad([[0, "#9cc3e8"], [1, "#eef2ee"]]);
      return svgWrap(`${d}<rect width="120" height="120" fill="url(#${g})"/><path d="M0 70 L120 64 V120 H0Z" fill="#9bb04f"/><path d="M0 84 L120 76 V120 H0Z" fill="#c6a94f"/><path d="M0 100 L120 92 V120 H0Z" fill="#7d9a3e"/><rect x="78" y="56" width="16" height="11" fill="#e9e1d2"/><path d="M76 57 L86 49 L96 57Z" fill="#a4442e"/>`);
    }
    case 2: {
      const [g, d] = grad([[0, "#36507a"], [1, "#e6a979"]]);
      const win = [[16, 66], [33, 56], [36, 64], [52, 64], [73, 52], [76, 60], [95, 68], [108, 82], [24, 90], [64, 92]].map(([x, y]) => `<rect x="${x}" y="${y}" width="2.5" height="3" fill="#ffd27a"/>`).join("");
      return svgWrap(`${d}<rect width="120" height="120" fill="url(#${g})"/><path d="M0 120 V74 H12 V60 H22 V80 H30 V50 H42 V70 H50 V58 H62 V84 H70 V46 H84 V72 H92 V62 H104 V78 H120 V120Z" fill="#1f2531"/>${win}`);
    }
    case 3:
      return svgWrap(`<circle cx="26" cy="28" r="8" fill="#ffd166"/><circle cx="30" cy="132" r="62" fill="#8cc084"/><circle cx="102" cy="138" r="58" fill="#6aa86a"/><circle cx="86" cy="74" r="7" fill="#3f7d4f"/><rect x="85" y="80" width="2" height="8" fill="#6b4a2f"/><circle cx="62" cy="86" r="5" fill="#3f7d4f"/>`, "#bfe3f2");
    case 4:
      return svgWrap(`<rect x="8" y="60" width="22" height="60" fill="#e07a5f"/><rect x="34" y="44" width="20" height="76" fill="#3d405b"/><rect x="58" y="66" width="24" height="54" fill="#81b29a"/><rect x="86" y="52" width="24" height="68" fill="#f2cc8f"/>${[[13, 68], [20, 68], [13, 80], [39, 52], [46, 52], [39, 64], [63, 74], [72, 74], [91, 60], [100, 60], [91, 72]].map(([x, y]) => `<rect x="${x}" y="${y}" width="4" height="5" fill="#fbf4e8" opacity=".85"/>`).join("")}`, "#f6dcb4");
    case 5:
      return svgWrap(`<circle cx="84" cy="34" r="10" fill="#f8f4ec"/><path d="M0 84 Q34 64 64 78 T120 72 V120 H0Z" fill="#d6cdbd"/><path d="M0 98 Q44 84 84 96 T120 92 V120 H0Z" fill="#bfb5a3"/>`, "#ece6da");
    case 6:
      return svgWrap(`<rect x="6" y="46" width="16" height="74" fill="#6c7fa3"/><rect x="24" y="24" width="18" height="96" fill="#4a5b7a"/><rect x="44" y="56" width="14" height="64" fill="#8597bb"/><rect x="60" y="14" width="20" height="106" fill="#3b4a66"/><rect x="82" y="38" width="16" height="82" fill="#6c7fa3"/><rect x="100" y="60" width="16" height="60" fill="#4a5b7a"/><rect x="69" y="6" width="2" height="8" fill="#3b4a66"/>`, "#a9c6e8");
    default:
      return svgWrap(`<g fill="none" stroke="#101114" stroke-width="1.6" stroke-linecap="round"><circle cx="86" cy="32" r="9"/><path d="M0 82 Q30 62 60 76 T120 70"/><path d="M0 98 Q40 82 80 96 T120 92"/><path d="M30 74 v-12 M24 66 q6 -10 12 0"/></g>`, "#f5f3ee");
  }
}

const ART = {
  weather: [
    () => svgWrap(`${[14, 46, 78].map((x) => `<rect x="${x}" y="36" width="28" height="48" rx="6" fill="#25282d"/><circle cx="${x + 14}" cy="52" r="6" fill="#ffc94d"/>${bars(x + 7, 66, [14, 10], "#6b6a67")}`).join("")}${bars(14, 18, [44, 26], "#f5f3ee", 8, 4)}`, "#15171a"),
    () => { const [g, d] = grad([[0, "#4f86c6"], [1, "#bcd7ee"]]); return svgWrap(`${d}<rect width="120" height="120" fill="url(#${g})"/><ellipse cx="38" cy="44" rx="22" ry="9" fill="#fff" opacity=".85"/><ellipse cx="80" cy="30" rx="16" ry="6" fill="#fff" opacity=".7"/><path d="M0 92 Q40 80 80 90 T120 86 V120 H0Z" fill="#4c7a3d"/>${txt(28, 84, 26, "#fff", "21°", 500, "start")}`); },
  ],
  clock: [
    () => svgWrap(`${txt(60, 66, 30, "#f5f3ee", "12:45")}${bars(36, 78, [48], "#6b6a67")}`, "#0e0f10"),
    () => svgWrap(`${txt(60, 56, 44, "#f5f3ee", "12", 600)}${txt(60, 100, 44, "#f5f3ee", "45", 600)}`, "#0e0f10"),
    () => svgWrap(`<circle cx="60" cy="60" r="40" fill="none" stroke="#f5f3ee" stroke-width="3"/><line x1="60" y1="60" x2="60" y2="32" stroke="#f5f3ee" stroke-width="4" stroke-linecap="round"/><line x1="60" y1="60" x2="82" y2="68" stroke="#f5f3ee" stroke-width="3" stroke-linecap="round"/><line x1="60" y1="60" x2="44" y2="88" stroke="#ff4a1c" stroke-width="1.5"/>`, "#0e0f10"),
    () => svgWrap(`<rect x="20" y="20" width="80" height="80" rx="14" fill="#1d1e21" stroke="#f5f3ee" stroke-width="2"/>${[0, 90, 180, 270].map((a) => `<line x1="60" y1="26" x2="60" y2="32" stroke="#f5f3ee" stroke-width="2" transform="rotate(${a} 60 60)"/>`).join("")}<line x1="60" y1="60" x2="60" y2="36" stroke="#f5f3ee" stroke-width="4" stroke-linecap="round"/><line x1="60" y1="60" x2="80" y2="60" stroke="#f5f3ee" stroke-width="3" stroke-linecap="round"/>`, "#0e0f10"),
    () => svgWrap(`<circle cx="60" cy="60" r="3" fill="#f5f3ee"/><line x1="60" y1="60" x2="60" y2="30" stroke="#f5f3ee" stroke-width="3" stroke-linecap="round"/><line x1="60" y1="60" x2="84" y2="74" stroke="#f5f3ee" stroke-width="2" stroke-linecap="round"/>`, "#0e0f10"),
    () => svgWrap(`<circle cx="60" cy="60" r="42" fill="none" stroke="#6b6a67" stroke-width="1.5"/>${txt(60, 32, 12, "#f5f3ee", "12")}${txt(91, 64, 12, "#f5f3ee", "3")}${txt(60, 96, 12, "#f5f3ee", "6")}${txt(29, 64, 12, "#f5f3ee", "9")}<line x1="60" y1="60" x2="60" y2="40" stroke="#f5f3ee" stroke-width="3" stroke-linecap="round"/><line x1="60" y1="60" x2="76" y2="66" stroke="#f5f3ee" stroke-width="2" stroke-linecap="round"/>`, "#0e0f10"),
    () => svgWrap(`${txt(40, 46, 18, "#f5f3ee", "12:45")}`, "#000"),
    () => svgWrap(`${[14, 62].map((x) => `<rect x="${x}" y="34" width="44" height="52" rx="6" fill="#2a2b2e"/>`).join("")}${txt(36, 74, 32, "#f5f3ee", "12", 600)}${txt(84, 74, 32, "#f5f3ee", "45", 600)}<line x1="14" y1="60" x2="106" y2="60" stroke="#0e0f10" stroke-width="2"/>`, "#0e0f10"),
    // 8 nixie: four tubes on a walnut plinth
    () => svgWrap(`${[14, 38, 66, 90].map((x, i) => `<rect x="${x}" y="30" width="18" height="50" rx="9" fill="rgba(24,14,8,.5)" stroke="rgba(255,255,255,.3)"/>${txt(x + 9, 66, 22, "#ff8a2a", "1245"[i])}`).join("")}<circle cx="60" cy="46" r="2" fill="#ffb35c"/><circle cx="60" cy="60" r="2" fill="#ffb35c"/><rect x="8" y="82" width="104" height="12" rx="3" fill="#6b3f22"/>`, "#0e0f10"),
    // 9 LCD quartz: the resin case, the green LCD
    () => svgWrap(`<rect x="10" y="26" width="100" height="68" rx="16" fill="#1b1c20"/><rect x="20" y="36" width="80" height="48" rx="6" fill="#a9b7a0"/>${txt(60, 72, 24, "#1d2620", "12:45", 700)}`, "#0e0f10"),
    // 10 twin-bell: the red clock with two bells
    () => svgWrap(`<circle cx="36" cy="30" r="12" fill="#ff4a1c"/><circle cx="84" cy="30" r="12" fill="#ff4a1c"/><circle cx="60" cy="64" r="38" fill="#ff4a1c"/><circle cx="60" cy="64" r="30" fill="#f5f3ee"/><line x1="60" y1="64" x2="60" y2="44" stroke="#101114" stroke-width="4" stroke-linecap="round"/><line x1="60" y1="64" x2="76" y2="70" stroke="#101114" stroke-width="3" stroke-linecap="round"/>`, "#0e0f10"),
  ],
  music: [
    () => { const [g, d] = grad([[0, "#8e5bd0"], [1, "#3fb6c5"]], false); return svgWrap(`${d}<rect x="16" y="16" width="56" height="56" rx="8" fill="url(#${g})"/>${bars(16, 82, [70, 44], "#f5f3ee", 8, 4)}<rect x="16" y="100" width="88" height="3" rx="1.5" fill="#3a3c40"/><rect x="16" y="100" width="40" height="3" rx="1.5" fill="#f5f3ee"/>`, "#121314"); },
    () => { const [g, d] = grad([[0, "#d05b8e"], [1, "#3f6bc5"]]); const [g2, d2] = grad([[0, "rgba(0,0,0,0)"], [1, "rgba(0,0,0,.75)"]]); return svgWrap(`${d}${d2}<rect width="120" height="120" fill="url(#${g})"/><rect y="50" width="120" height="70" fill="url(#${g2})"/>${bars(14, 88, [64, 40], "#fff", 9, 4)}`); },
    () => svgWrap(`<rect x="8" y="8" width="104" height="104" rx="6" fill="#4a3020"/><circle cx="54" cy="60" r="40" fill="#141414"/><circle cx="54" cy="60" r="28" fill="none" stroke="#2a2a2a"/><circle cx="54" cy="60" r="11" fill="#e8743b"/><circle cx="54" cy="60" r="1.6" fill="#141414"/><path d="M100 20 L98 70 L74 84" stroke="#c9c9c9" stroke-width="3" fill="none" stroke-linecap="round"/>`, "#2a1c12"),
    () => svgWrap(`<rect x="12" y="28" width="96" height="64" rx="6" fill="#e8e2d4"/><rect x="26" y="42" width="68" height="26" rx="4" fill="#3a3a3a"/><circle cx="42" cy="55" r="9" fill="#f5f3ee"/><circle cx="78" cy="55" r="9" fill="#f5f3ee"/><circle cx="42" cy="55" r="3" fill="#3a3a3a"/><circle cx="78" cy="55" r="3" fill="#3a3a3a"/><path d="M30 92 L36 80 H84 L90 92Z" fill="#d6cfbf"/>`, "#1d1e21"),
    // the half cassette as the panel draws it: a big cassette (label, one reel) cut by the dark player panel on the right
    () => { const [g, d] = grad([[0, "#b0662e"], [1, "#5a2410"]]); return svgWrap(`${d}<rect width="120" height="120" fill="url(#${g})"/><rect x="5" y="14" width="86" height="98" rx="6" fill="#1f1f22" stroke="#4a4b50" stroke-width="1.2"/><rect x="11" y="19" width="76" height="31" rx="3" fill="#efe7d6"/><circle cx="21" cy="30" r="6" fill="#ff4a1c"/><rect x="31" y="25" width="38" height="5" rx="2.5" fill="#2a2a2e"/><rect x="31" y="34" width="26" height="3" rx="1.5" fill="#7a7468"/><rect x="11" y="41" width="76" height="3" fill="#ff4a1c"/><rect x="11" y="46" width="76" height="1.6" fill="#ff4a1c"/><circle cx="45" cy="78" r="27" fill="#5a3a2a"/><circle cx="45" cy="78" r="12" fill="#f5f3ee"/><circle cx="45" cy="78" r="5" fill="#2a2a2e"/><rect x="84" y="0" width="36" height="120" fill="#202124"/><rect x="90" y="9" width="26" height="7" rx="3.5" fill="#3a3b40"/><rect x="93" y="26" width="20" height="5" rx="2" fill="#f5f3ee"/><circle cx="103" cy="50" r="10" fill="#ff4a1c"/><rect x="100" y="45.5" width="2.2" height="9" fill="#fff"/><rect x="104" y="45.5" width="2.2" height="9" fill="#fff"/><path d="M92 68 v6 l-4 -3z M114 68 v6 l4 -3z" fill="#e8e8ea"/><rect x="90" y="96" width="24" height="2" rx="1" fill="#55565c"/><rect x="90" y="96" width="14" height="2" rx="1" fill="#f5f3ee"/>`, "#1d1e21"); },
  ],
  timers: [
    () => svgWrap(`<circle cx="60" cy="60" r="38" fill="none" stroke="#2a2c30" stroke-width="8"/><circle cx="60" cy="60" r="38" fill="none" stroke="#ff4a1c" stroke-width="8" stroke-dasharray="160 240" transform="rotate(-90 60 60)" stroke-linecap="round"/>${txt(60, 68, 22, "#f5f3ee", "5:00")}`, "#101114"),
    () => svgWrap(`<circle cx="60" cy="62" r="40" fill="#fbfaf6" stroke="#d9d3c5" stroke-width="2"/><path d="M60 62 L60 22 A40 40 0 0 1 98 50 Z" fill="#e24a33"/><circle cx="60" cy="62" r="8" fill="#d9d3c5"/>${[0, 30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330].map((a) => `<line x1="60" y1="24" x2="60" y2="29" stroke="#8a857a" stroke-width="1.5" transform="rotate(${a} 60 62)"/>`).join("")}`, "#f1ede4"),
    () => svgWrap(`<path d="M38 18 H82 M38 102 H82" stroke="#d9c8a0" stroke-width="4" stroke-linecap="round"/><path d="M42 20 L60 60 L42 100 H78 L60 60 L78 20Z" fill="none" stroke="#d9c8a0" stroke-width="2.5"/><path d="M50 36 H70 L60 56Z" fill="#e3b85c"/><path d="M46 98 L60 80 L74 98Z" fill="#e3b85c"/>`, "#1b1c1f"),
    () => svgWrap(`${synthBase(0)}<circle cx="60" cy="60" r="26" fill="none" stroke="#4ff0ff" stroke-width="4" stroke-dasharray="110 200" transform="rotate(-90 60 60)"/>`),
    () => svgWrap(`${[14, 62].map((x) => `<rect x="${x}" y="34" width="44" height="52" rx="6" fill="#2a2b2e"/>`).join("")}${txt(36, 74, 30, "#f5f3ee", "05", 600)}${txt(84, 74, 30, "#f5f3ee", "00", 600)}<line x1="14" y1="60" x2="106" y2="60" stroke="#0e0f10" stroke-width="2"/>`, "#101114"),
  ],
  thermostat: [
    () => svgWrap(`<circle cx="60" cy="62" r="40" fill="none" stroke="#2a2c30" stroke-width="7" stroke-dasharray="188 252" transform="rotate(135 60 62)" stroke-linecap="round"/><circle cx="60" cy="62" r="40" fill="none" stroke="#ff8a3d" stroke-width="7" stroke-dasharray="110 252" transform="rotate(135 60 62)" stroke-linecap="round"/>${txt(60, 70, 24, "#f5f3ee", "21.5")}`, "#121314"),
    () => svgWrap(`<rect x="34" y="10" width="52" height="100" rx="14" fill="#f7f5f0" stroke="#cfc9bd"/><rect x="42" y="20" width="36" height="22" rx="3" fill="#c7d3c0"/>${txt(60, 37, 13, "#2a3a2a", "21.5")}${[[48, 56], [72, 56], [48, 74], [72, 74], [60, 92]].map(([x, y]) => `<circle cx="${x}" cy="${y}" r="6" fill="#e2ddd2"/>`).join("")}`, "#e8e4dc"),
    () => svgWrap(`<rect x="22" y="22" width="76" height="76" rx="10" fill="#efe7da" stroke="#cbbfa8"/><circle cx="60" cy="60" r="24" fill="#e2d6c2" stroke="#b9ab92" stroke-width="2"/>${txt(60, 66, 15, "#4a3f34", "21°")}`, "#d9cfc0"),
    () => svgWrap(`${synthBase(1)}<circle cx="60" cy="56" r="26" fill="rgba(0,0,0,.45)" stroke="#ff2bd6" stroke-width="3"/>${txt(60, 62, 16, "#fff", "21.5")}`),
  ],
  quick: [
    () => svgWrap([0, 1, 2, 3].flatMap((r) => [0, 1].map((c) => `<rect x="${14 + c * 48}" y="${14 + r * 24}" width="44" height="20" rx="6" fill="${(r + c) % 3 === 0 ? "#f5f3ee" : "#26292e"}"/>`)).join(""), "#121314"),
    () => svgWrap([0, 1, 2, 3].map((i) => `<rect x="${22 + i * 24}" y="16" width="4" height="76" rx="2" fill="#0c0c0d"/><rect x="${15 + i * 24}" y="${30 + (i * 17) % 44}" width="18" height="10" rx="2" fill="#d9d9d9"/><rect x="${13 + i * 24}" y="98" width="22" height="8" fill="#0c0c0c"/><rect x="${16 + i * 24}" y="101" width="14" height="2" fill="#f2f2f2"/>`).join(""), "#1c1d20"),
    () => svgWrap(`<rect x="8" y="22" width="104" height="76" rx="6" fill="#4a5058" stroke="#2c3036"/>${[0, 1, 2, 3].map((i) => `<circle cx="${24 + i * 24}" cy="60" r="7" fill="#2c3036"/><rect x="${22 + i * 24}" y="${i % 2 ? 40 : 60}" width="4" height="20" rx="2" fill="#dcdcdc"/>`).join("")}${[[14, 28], [106, 28], [14, 92], [106, 92]].map(([x, y]) => `<circle cx="${x}" cy="${y}" r="2" fill="#8a9099"/>`).join("")}`, "#2b2f35"),
  ],
  camera: [
    () => { const [g, d] = grad([[0, "#2d3a4a"], [1, "#56677a"]]); return svgWrap(`${d}<rect width="120" height="120" fill="url(#${g})"/><rect x="44" y="40" width="32" height="64" fill="#3b2f2a"/><circle cx="70" cy="74" r="2" fill="#c9a86b"/><rect x="0" y="104" width="120" height="16" fill="#3a434d"/><circle cx="14" cy="14" r="4" fill="#ff4a1c"/>`); },
  ],
  calendar: [
    () => svgWrap(`${[0, 1, 2, 3, 4].map((i) => `<line x1="${14 + i * 23}" y1="18" x2="${14 + i * 23}" y2="104" stroke="#2a2c30"/>`).join("")}<rect x="16" y="30" width="19" height="16" rx="3" fill="#3b8cff"/><rect x="39" y="52" width="19" height="22" rx="3" fill="#8d5bff"/><rect x="62" y="36" width="19" height="12" rx="3" fill="#3ccb7f"/><rect x="85" y="66" width="19" height="18" rx="3" fill="#3b8cff"/>`, "#121314"),
    () => svgWrap([0, 1, 2, 3, 4].map((i) => `<rect x="10" y="${16 + i * 19}" width="100" height="15" fill="#1d1d1d"/><rect x="14" y="${21 + i * 19}" width="22" height="5" fill="#f5c400"/><rect x="42" y="${21 + i * 19}" width="${40 + (i * 13) % 24}" height="5" fill="#f5c400"/>`).join(""), "#0b0b0b"),
    () => svgWrap(`<rect x="26" y="16" width="68" height="88" rx="4" fill="#f5f3ee"/><rect x="26" y="16" width="68" height="18" rx="4" fill="#d63a1e"/>${txt(60, 82, 40, "#101114", "15", 600)}`, "#2a2c30"),
    () => svgWrap(`<rect x="12" y="16" width="96" height="94" rx="3" fill="#fbf9f4"/>${[0, 1, 2, 3, 4, 5, 6].map((i) => `<circle cx="${22 + i * 13}" cy="16" r="2.5" fill="#6b6a67"/>`).join("")}${bars(20, 26, [40], "#101114", 0, 5)}${[0, 1, 2, 3].map((r) => [0, 1, 2, 3, 4, 5, 6].map((c) => `<rect x="${18 + c * 12.5}" y="${42 + r * 16}" width="10" height="12" fill="none" stroke="#d9d3c5"/>`).join("")).join("")}<path d="M44 62 q6 -4 12 0" stroke="#2e6bd6" fill="none"/>`, "#e9e4da"),
  ],
  todo: [
    () => svgWrap([0, 1, 2, 3, 4].map((i) => `<rect x="16" y="${20 + i * 18}" width="10" height="10" rx="2.5" fill="none" stroke="#9a978f" stroke-width="1.5"/><rect x="34" y="${23 + i * 18}" width="${60 - (i * 9) % 30}" height="4" rx="2" fill="#f5f3ee"/>`).join(""), "#121314"),
    () => svgWrap(`<path d="M30 8 H90 V104 l-5 6 -5 -6 -5 6 -5 -6 -5 6 -5 -6 -5 6 -5 -6 -5 6 -5 -6 -5 6 -5 -6Z" fill="#f7f5ef"/>${bars(38, 22, [44, 30, 38, 26, 40], "#9a978f", 12, 3)}`, "#2a2c30"),
    () => svgWrap(`${bars(14, 24, [52, 70, 40, 64], "#3ccb7f", 14, 4)}<rect x="14" y="82" width="8" height="10" fill="#3ccb7f"/>`, "#050805"),
    () => svgWrap([0, 1, 2, 3].map((i) => `<g transform="rotate(${[-3, 2, -1, 3][i]} 60 ${28 + i * 22})"><rect x="${20 + (i % 2) * 8}" y="${20 + i * 22}" width="${70 - (i % 3) * 10}" height="15" rx="2" fill="#0c0c0c"/><rect x="${27 + (i % 2) * 8}" y="${26 + i * 22}" width="${44 - (i % 3) * 8}" height="3" fill="#f2f2f2"/></g>`).join(""), "#26282c"),
    () => svgWrap(`${[0, 1, 2, 3, 4, 5].map((i) => `<line x1="0" y1="${30 + i * 15}" x2="120" y2="${30 + i * 15}" stroke="#9fbce0"/>`).join("")}<line x1="26" y1="0" x2="26" y2="120" stroke="#e07a7a"/>${bars(32, 24, [50, 38, 56], "#2c3e7a", 15, 3)}`, "#fff3a8"),
  ],
  info: [
    () => svgWrap([0, 1, 2, 3].map((i) => `<circle cx="22" cy="${26 + i * 22}" r="7" fill="#26292e"/><rect x="36" y="${21 + i * 22}" width="${62 - (i * 11) % 24}" height="4" rx="2" fill="#f5f3ee"/><rect x="36" y="${29 + i * 22}" width="40" height="3" rx="1.5" fill="#6b6a67"/>`).join(""), "#121314"),
    () => svgWrap(`<rect x="0" y="0" width="6" height="120" fill="#c9cfd4"/><g transform="rotate(-6 40 40)"><rect x="18" y="20" width="40" height="40" fill="#ffe066"/></g><g transform="rotate(5 82 52)"><rect x="62" y="32" width="40" height="40" fill="#ff9fb2"/></g><g transform="rotate(-3 52 92)"><rect x="32" y="72" width="40" height="36" fill="#a0e7a0"/></g><circle cx="38" cy="22" r="4" fill="#d63a1e"/><circle cx="82" cy="34" r="4" fill="#2e6bd6"/><circle cx="52" cy="74" r="4" fill="#3ccb7f"/>`, "#dfe3e6"),
    () => svgWrap(`<rect x="16" y="26" width="88" height="68" rx="12" fill="#2e3033"/><rect x="26" y="36" width="68" height="32" rx="3" fill="#9fb38a"/>${bars(32, 44, [46, 30], "#2a3a22", 9, 4)}<circle cx="38" cy="82" r="5" fill="#1d1e21"/><circle cx="60" cy="82" r="5" fill="#1d1e21"/><circle cx="82" cy="82" r="5" fill="#1d1e21"/><circle cx="96" cy="32" r="2.5" fill="#3ccb7f"/>`, "#1d1e21"),
    () => svgWrap(`<rect x="0" y="70" width="120" height="22" fill="#d63a1e"/><rect x="8" y="77" width="30" height="8" fill="#fff"/><rect x="44" y="78" width="60" height="6" fill="#ffd9d0"/><rect x="0" y="96" width="120" height="12" fill="#f5f3ee"/>${bars(6, 100, [80], "#101114", 0, 4)}<rect x="20" y="16" width="80" height="46" rx="3" fill="#2b3a66"/>`, "#1a2340"),
  ],
  voice: [
    () => svgWrap(`<circle cx="60" cy="60" r="38" fill="#ff4a1c" opacity=".12"/><circle cx="60" cy="60" r="26" fill="#ff4a1c" opacity=".22"/><circle cx="60" cy="60" r="14" fill="#ff4a1c"/>`, "#0e0f10"),
    () => svgWrap(`${txt(14, 34, 12, "#3ccb7f", "&gt;", 600, "start")}${bars(26, 26, [50, 66, 38], "#3ccb7f", 16, 4)}<rect x="14" y="78" width="8" height="10" fill="#3ccb7f"/>`, "#050805"),
    () => svgWrap(`<path d="M18 24 H102 a8 8 0 0 1 8 8 V72 a8 8 0 0 1 -8 8 H52 L36 98 L40 80 H18 a8 8 0 0 1 -8 -8 V32 a8 8 0 0 1 8 -8Z" fill="#fff" stroke="#101114" stroke-width="3"/><circle cx="44" cy="52" r="4" fill="#101114"/><circle cx="60" cy="52" r="4" fill="#101114"/><circle cx="76" cy="52" r="4" fill="#101114"/>`, "#ffd84a"),
    () => svgWrap(`${[30, 60, 90].map((v) => `<line x1="${v}" y1="0" x2="${v}" y2="120" stroke="#163016"/><line x1="0" y1="${v}" x2="120" y2="${v}" stroke="#163016"/>`).join("")}<path d="M0 60 C10 30 20 30 30 60 S50 90 60 60 S80 30 90 60 S110 90 120 60" fill="none" stroke="#3cff7a" stroke-width="2.5"/>`, "#081008"),
  ],
};

// The picture of a page's look (a Custom page: its background; plain = the panel's theme, see bgArt).
function lookArt(kind, variant, page, plain = null) {
  if (kind === "home") return svgNode(bgArt(bgIndex(page), plain));
  const list = ART[kind] || ART.info;
  return svgNode((list[variant] || list[0])());
}

// The looks as the panel itself draws them: pictures taken on a panel with demo content (looks/<name>-<dark|light>.webp,
// 224 px), and per browser the panel's own screen whenever it showed a look (newest first, see _captureLook).
const LIVE_LOOKS_KEY = "panelos-live-looks";
// Look pictures that do not exist (the drawing is shown instead, without asking for the file again).
const MISSING_LOOKS = new Set();
const LIVE_LOOKS_MAX = 120;
function readLiveLooks() {
  try {
    const v = JSON.parse(window.localStorage.getItem(LIVE_LOOKS_KEY) || "{}");
    return v && typeof v === "object" ? v : {};
  } catch (_e) {
    return {};
  }
}
function saveLiveLooks(all) {
  const keys = Object.keys(all).sort((a, b) => all[b].t - all[a].t);
  for (const k of keys.slice(LIVE_LOOKS_MAX)) delete all[k];
  try {
    window.localStorage.setItem(LIVE_LOOKS_KEY, JSON.stringify(all));
  } catch (_e) {
    // full or blocked: the pictures stay for this visit only
  }
}
// The picture name of a page's look.
function lookName(kind, variant, page) {
  return kind === "home" ? `bg-${bgIndex(page)}` : `${kind}-${variant || 0}`;
}

// The BACKGROUNDS index of a Custom page (options.theme / options.scene).
function bgIndex(page) {
  const o = (page && page.options) || {};
  const theme = Number.isInteger(o.theme) ? o.theme : 1;
  if (theme === 8) return 8 + Math.max(0, Math.min(3, Number.isInteger(o.scene) ? o.scene : 0));
  if (theme === PLAIN_THEME) return BACKGROUNDS.findIndex((b) => b.theme === PLAIN_THEME);
  return Math.max(0, Math.min(7, theme));
}


// ---------------------------------------------------------------- style

const FONTS = `
@font-face { font-family: "PanelOS Text"; src: url("{STATIC}/fonts/NotoSans-Latin.woff2") format("woff2"); font-weight: 100 900; font-display: swap; }
@font-face { font-family: "PanelOS Heading"; src: url("{STATIC}/fonts/SpaceGrotesk-Latin.woff2") format("woff2"); font-weight: 300 700; font-display: swap; }
`;

const STYLE = `
:host {
  --signal: #FF4A1C; --signal-soft: rgba(255, 74, 28, 0.13); --signal-line: rgba(255, 74, 28, 0.45);
  --ground: #F5F3EE; --surface: #FFFFFF; --raised: #EBE8E1; --sunken: #E4E0D7; --line: #DAD6CD; --nav: #EFECE5;
  --text: #101114; --muted: #5F5E5A; --faint: #8A877F; --ok: #1F9D55; --warn: #B7791F; --bad: #C7361B;
  --on-signal: #101114;
  --shadow: 0 1px 2px rgba(16,17,20,.06), 0 8px 24px rgba(16,17,20,.06);
  --shadow-lg: 0 24px 64px rgba(16,17,20,.22);
  --scrim: rgba(16,17,20,.38);
  --logo: url("{STATIC}/logo.svg");
  --bezel: #E9E6DF; --bezel-edge: #CFCAC0;
  display: block; height: 100vh; height: 100dvh; overflow: hidden; position: relative;
  background: var(--ground); color: var(--text); color-scheme: light;
  font-family: "PanelOS Text", "Noto Sans", system-ui, sans-serif; font-size: 15px; line-height: 1.45;
  -webkit-font-smoothing: antialiased;
}
:host([dark]) {
  --ground: #121314; --surface: #1A1B1E; --raised: #232528; --sunken: #0E0F10; --line: #2A2C30; --nav: #0E0F10;
  --text: #F5F3EE; --muted: #A3A19C; --faint: #7D7B76; --ok: #3CCB7F; --warn: #E0A840; --bad: #FF6A4D;
  --shadow: none; --shadow-lg: 0 24px 64px rgba(0,0,0,.6); --scrim: rgba(0,0,0,.6);
  --logo: url("{STATIC}/logo-dark.svg");
  --bezel: #1C1D20; --bezel-edge: #34363B;
  color-scheme: dark;
}
* { box-sizing: border-box; -webkit-tap-highlight-color: transparent; }
[hidden] { display: none !important; }
h1, h2, h3 { font-family: "PanelOS Heading", "Space Grotesk", "PanelOS Text", sans-serif; font-weight: 500; letter-spacing: -0.01em; margin: 0; }
button, input, select, textarea { font: inherit; color: inherit; }
button { cursor: pointer; }
.mdi { display: inline-flex; align-items: center; justify-content: center; flex: none; --mdc-icon-size: 20px; width: var(--mdc-icon-size); height: var(--mdc-icon-size); }
.muted { color: var(--muted); } .faint { color: var(--faint); }
.num { font-variant-numeric: tabular-nums; }
.wrap { overflow-wrap: anywhere; }

/* shell */
.app { display: grid; height: 100%; grid-template-columns: 248px minmax(0, 1fr) 392px; grid-template-rows: minmax(0, 1fr); grid-template-areas: "nav main pv"; position: relative; }
.app.no-preview { grid-template-columns: 248px minmax(0, 1fr); grid-template-areas: "nav main"; }
:host([pvbig]) .app:not(.no-preview) { grid-template-columns: 248px minmax(0, 1fr) 566px; }
.nav { grid-area: nav; background: var(--nav); border-right: 1px solid var(--line); display: flex; flex-direction: column; min-width: 0; min-height: 0; }
.top { grid-area: top; display: none; background: var(--ground); border-bottom: 1px solid var(--line); z-index: 5; min-width: 0; }
.main { grid-area: main; overflow-y: auto; min-width: 0; min-height: 0; overscroll-behavior: contain; }
.preview-col { grid-area: pv; overflow-y: auto; padding: 26px 24px 26px 4px; display: flex; flex-direction: column; align-items: center; gap: 14px; min-width: 0; min-height: 0; }
.bnav { grid-area: bnav; display: none; }
:host([size=medium]) .app { grid-template-columns: minmax(0, 1fr) 336px; grid-template-rows: auto minmax(0, 1fr); grid-template-areas: "top top" "main pv"; }
:host([size=medium]) .app.no-preview { grid-template-columns: minmax(0, 1fr); grid-template-areas: "top" "main"; }
:host([size=medium]) .nav, :host([size=narrow]) .nav { display: none; }
:host([size=medium]) .top, :host([size=narrow]) .top { display: block; }
:host([size=medium]) .preview-col { padding: 20px 20px 20px 0; }
:host([size=narrow]) .app, :host([size=narrow]) .app.no-preview { grid-template-columns: minmax(0, 1fr); grid-template-rows: auto minmax(0, 1fr) auto; grid-template-areas: "top" "main" "bnav"; }
:host([size=narrow]) .preview-col { display: none; }

.brand { height: 112px; flex: none; display: flex; align-items: flex-end; padding: 0 10px 8px; border-bottom: 1px solid var(--line); background-color: var(--ground); background-position: center; background-size: cover; }
/* Brand guide: the logo sits in a calm panel of the pattern's ground, 2u clear on every side
   (u = 5.9 px with a 168 px logo), corner 0.16 x the logo height, no border. */
.brand-box { display: block; background: var(--ground); padding: 12px; border-radius: 4px; }
.brand-box i { display: block; width: 168px; height: 24.4px; background: var(--logo) left center / contain no-repeat; }
.nav-scroll { overflow-y: auto; flex: 1; display: flex; flex-direction: column; padding: 14px 12px; gap: 14px; min-height: 0; }
.switcher { display: flex; align-items: center; gap: 12px; width: 100%; border: 1px solid var(--line); background: var(--surface); border-radius: 16px; padding: 8px 10px 8px 8px; text-align: left; min-width: 0; }
.switcher:hover { border-color: var(--signal-line); }
.switcher .txt { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.switcher .txt b { font-weight: 600; font-size: 15px; overflow-wrap: anywhere; }
.switcher .txt small { color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
.thumb { flex: none; width: 44px; height: 44px; border-radius: 8px; overflow: hidden; background: #000; position: relative; box-shadow: 0 0 0 1px var(--bezel-edge); display: grid; place-items: center; color: #6b6a67; }
.thumb img { width: 100%; height: 100%; object-fit: cover; display: block; }
.thumb .mdi { --mdc-icon-size: 22px; }
.links { display: flex; flex-direction: column; gap: 2px; }
.links button { display: flex; align-items: center; gap: 14px; border: 0; background: none; border-radius: 12px; padding: 10px 12px; text-align: left; color: var(--muted); font-size: 15px; min-width: 0; }
.links button .mdi { --mdc-icon-size: 21px; }
.links button:hover { background: var(--raised); color: var(--text); }
.links button.on { background: var(--signal-soft); color: var(--text); }
.links button.on .mdi { color: var(--signal); }
.links button .badge { margin-left: auto; }
.nav-foot { border-top: 1px solid var(--line); padding: 10px 12px; display: flex; flex-direction: column; gap: 2px; flex: none; }

.top-row { display: flex; align-items: center; gap: 6px; min-height: 58px; padding: 4px 8px 4px 4px; }
.top-row > i.logo { display: block; width: 112px; height: 16.3px; background: var(--logo) left center / contain no-repeat; flex: none; margin: 0 6px 0 8px; }
.top .switcher { border: 0; background: none; padding: 6px 8px; width: auto; flex: 1; border-radius: 12px; }
.top .switcher:hover { background: var(--raised); }
.tabs { display: flex; gap: 6px; padding: 0 14px 10px; overflow-x: auto; scrollbar-width: none; }
.tabs::-webkit-scrollbar { display: none; }
.tabs button { border: 1px solid var(--line); background: var(--surface); border-radius: 999px; padding: 6px 14px 6px 10px; white-space: nowrap; color: var(--muted); font-size: 14px; display: inline-flex; align-items: center; gap: 6px; flex: none; }
.tabs button .mdi { --mdc-icon-size: 17px; }
.tabs button.on { border-color: var(--signal); color: var(--text); background: var(--signal-soft); }
.tabs button.on .mdi { color: var(--signal); }
:host([size=narrow]) .tabs { display: none; }
.ib { width: 40px; height: 40px; border-radius: 20px; border: 0; background: none; display: grid; place-items: center; color: var(--muted); flex: none; position: relative; padding: 0; }
.ib .mdi { --mdc-icon-size: 22px; }
.ib:hover, .ib[aria-expanded=true] { background: var(--raised); color: var(--text); }
.ib:disabled { opacity: .4; cursor: default; }
.ib.info { width: 32px; height: 32px; color: var(--faint); }
.ib.info .mdi { --mdc-icon-size: 19px; }
.ib.info:hover, .ib.info[aria-expanded=true] { color: var(--signal); background: var(--signal-soft); }

.content { padding: 26px 34px 80px; display: flex; flex-direction: column; gap: 20px; max-width: 820px; width: 100%; }
.content.wide { max-width: 1100px; }
:host([size=medium]) .content { padding: 20px 24px 72px; }
:host([size=narrow]) .content { padding: 14px 16px 120px; gap: 14px; }

.bnav button { flex: 1 1 0; min-width: 0; border: 0; background: none; display: flex; flex-direction: column; align-items: center; gap: 2px; font-size: 11.5px; color: var(--muted); padding: 5px 0 3px; border-radius: 14px; text-align: center; }
.bnav button .mdi { --mdc-icon-size: 23px; width: 56px; height: 30px; border-radius: 15px; }
.bnav button.on { color: var(--text); }
.bnav button.on .mdi { background: var(--signal-soft); color: var(--signal); }
:host([size=narrow]) .bnav { display: flex; background: var(--surface); border-top: 1px solid var(--line); padding: 6px 6px calc(6px + env(safe-area-inset-bottom, 0px)); justify-content: space-around; z-index: 6; }

/* content blocks */
.hero { display: flex; align-items: flex-start; gap: 12px; padding: 2px 2px 4px; flex-wrap: wrap; }
.hero .t { flex: 1 1 220px; min-width: 0; display: flex; flex-direction: column; gap: 6px; }
.hero h1 { font-size: 30px; line-height: 1.15; overflow-wrap: anywhere; }
:host([size=narrow]) .hero h1 { font-size: 24px; }
:host([size=narrow]) .ov-hero h1 { display: none; }
.hero .meta { display: flex; flex-wrap: wrap; gap: 4px 14px; color: var(--muted); font-size: 14px; align-items: center; }
.hero .acts { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.back { display: inline-flex; align-items: center; gap: 4px; border: 0; background: none; color: var(--muted); padding: 4px 8px 4px 2px; border-radius: 10px; font-size: 14px; margin: -4px 0 -8px -2px; align-self: flex-start; }
.back:hover { color: var(--text); background: var(--raised); }
.state { display: inline-flex; align-items: center; gap: 7px; }
.state b { width: 8px; height: 8px; border-radius: 50%; background: var(--faint); flex: none; }
.state.on b { background: var(--ok); box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 22%, transparent); }

.card { background: var(--surface); border: 1px solid var(--line); border-radius: 18px; padding: 6px 22px 10px; box-shadow: var(--shadow); position: relative; min-width: 0; }
:host([size=narrow]) .card { padding: 4px 16px 8px; border-radius: 16px; }
.card-head { display: flex; align-items: center; gap: 8px; min-height: 56px; position: relative; flex-wrap: wrap; row-gap: 8px; padding: 6px 0; }
.card-head h2 { font-size: 19px; flex: 1 1 160px; min-width: 0; overflow-wrap: anywhere; }
.card-head .sub { color: var(--muted); font-size: 13.5px; font-family: "PanelOS Text", sans-serif; letter-spacing: 0; font-weight: 400; }
.card-head .acts { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; max-width: 100%; }
.pop { position: fixed; left: 0; top: 0; right: auto; z-index: 60; box-sizing: border-box; overflow-wrap: anywhere; width: min(360px, calc(100vw - 24px)); background: var(--raised); color: var(--text); border: 1px solid var(--line); border-radius: 14px; padding: 12px 14px; font-size: 14px; line-height: 1.5; box-shadow: var(--shadow-lg); font-family: "PanelOS Text", sans-serif; letter-spacing: 0; font-weight: 400; text-align: left; }
.row { display: flex; align-items: center; justify-content: space-between; gap: 16px; min-height: 60px; padding: 10px 0; border-top: 1px solid var(--line); min-width: 0; }
.row.first, .card-head + .row { border-top: 0; }
.row > .lw { flex: 1 1 auto; min-width: 0; display: flex; align-items: center; gap: 2px; }
.row > .lw > .label { min-width: 0; display: flex; flex-direction: column; overflow-wrap: anywhere; }
.row > .lw > .label small { color: var(--muted); font-size: 13px; }
.row.click { cursor: pointer; }
.row.click:hover > .lw > .label > span { text-decoration: underline; text-underline-offset: 3px; }
.row.dim > .lw > .label, .row.dim > .lead, .row.dim > .stlead { opacity: .55; }
.rinfo { position: relative; flex: none; }
/* bubbles are placed by _placePops() next to their button, in window coordinates, never sized by a parent */
.row > .control { flex: 0 1 auto; display: flex; align-items: center; gap: 10px; min-width: 0; justify-content: flex-end; flex-wrap: wrap; }
.row.stack { flex-direction: column; align-items: stretch; gap: 10px; padding: 14px 0 16px; }
/* rows with long texts (notifications): icon, text and button line up at the top, every row alike */
.row.atop { align-items: flex-start; }
.row.atop > .control { padding-top: 1px; flex: none; }
.row.stack > .control { justify-content: flex-start; width: 100%; }
:host([size=narrow]) .row:not(.stack):not(.keep) { flex-wrap: wrap; gap: 8px 16px; }
:host([size=narrow]) .row:not(.stack):not(.keep) > .control { margin-left: auto; }
.lead { width: 38px; height: 38px; border-radius: 12px; background: var(--raised); display: grid; place-items: center; flex: none; color: var(--muted); }
.lead .mdi { --mdc-icon-size: 21px; }
.lead.on { background: var(--signal-soft); color: var(--signal); }
.below { padding: 0 0 14px; margin-top: -4px; }

/* controls */
.seg { position: relative; display: inline-flex; flex-wrap: wrap; background: var(--raised); border-radius: 20px; padding: 3px; gap: 2px; max-width: 100%; }
.seg button { position: relative; z-index: 1; border: 0; background: none; font-size: 14px; font-weight: 500; padding: 7px 15px; border-radius: 999px; color: var(--muted); text-align: center; transition: color .16s; }
.seg button:hover { color: var(--text); }
.seg button.on { background: var(--text); color: var(--ground); }
.seg button:disabled { opacity: .4; cursor: default; }
.seg .pill { position: absolute; left: 0; top: 0; display: none; border-radius: 999px; background: var(--text); pointer-events: none; }
.seg.jm .pill { display: block; }
.seg.jm button.on { background: none; }
.seg.jm.jpre button { transition: none; }
.seg.jm.jpre button.on { color: var(--muted); }
.seg.jm.jpre button.jwas { color: var(--ground); }
.sel { position: relative; max-width: 100%; min-width: 0; flex: 0 1 auto; display: inline-block; }
.sel.disabled { opacity: .45; pointer-events: none; }
.sel select { appearance: none; -webkit-appearance: none; max-width: 100%; font-size: 14px; color: var(--text); background: var(--raised); border: 1px solid transparent; border-radius: 12px; padding: 9px 36px 9px 14px; }
.sel.full, .sel.full select { width: 100%; }
.sel select:focus-visible, .inp:focus-visible, button:focus-visible { outline: 2px solid var(--signal); outline-offset: 1px; }
.sel .mdi { position: absolute; right: 9px; top: 50%; transform: translateY(-50%); pointer-events: none; color: var(--muted); }
.inp { font-size: 14px; background: var(--raised); border: 1px solid transparent; border-radius: 12px; padding: 9px 13px; color: var(--text); min-width: 0; width: 100%; }
.inp.time { width: 112px; font-variant-numeric: tabular-nums; }
.inp.short { width: 200px; max-width: 100%; }
.switch { --swW: 48px; --swH: 28px; --kn: 20px; position: relative; width: var(--swW); height: var(--swH); flex: none; display: inline-block; touch-action: pan-y; }
.switch input { opacity: 0; width: 100%; height: 100%; margin: 0; position: absolute; cursor: pointer; z-index: 1; touch-action: pan-y; }
.switch span { --on: 0; position: absolute; inset: 0; border-radius: 999px; background: var(--raised); box-shadow: inset 0 0 0 1px var(--line); }
.switch span::before { content: ""; position: absolute; inset: 0; border-radius: inherit; background: var(--signal); opacity: var(--on); }
.switch span i { position: absolute; left: 4px; top: 4px; width: var(--kn); height: var(--kn); border-radius: 999px; background: color-mix(in srgb, #F5F3EE calc(var(--on) * 100%), var(--faint)); }
.switch input:checked + span { --on: 1; }
.switch input:checked + span i { left: calc(var(--swW) - 4px - var(--kn)); }
.switch input:disabled { cursor: default; }
.switch input:disabled + span { opacity: .45; }
.switch input:focus-visible + span { outline: 2px solid var(--signal); outline-offset: 2px; }
.slider { display: flex; align-items: center; gap: 12px; min-width: 0; }
input[type=range] { -webkit-appearance: none; appearance: none; width: 220px; max-width: 46vw; height: 28px; background: transparent; cursor: pointer; margin: 0; }
input[type=range]::-webkit-slider-runnable-track { height: 6px; border-radius: 3px; background: linear-gradient(to right, var(--signal) var(--fill, 0%), var(--raised) var(--fill, 0%)); }
input[type=range]::-moz-range-track { height: 6px; border-radius: 3px; background: var(--raised); }
input[type=range]::-moz-range-progress { height: 6px; border-radius: 3px; background: var(--signal); }
input[type=range]::-webkit-slider-thumb { -webkit-appearance: none; width: 20px; height: 20px; border-radius: 50%; background: var(--surface); border: 2px solid var(--signal); margin-top: -7px; }
input[type=range]::-moz-range-thumb { width: 16px; height: 16px; border-radius: 50%; background: var(--surface); border: 2px solid var(--signal); }
input[type=range]:disabled { opacity: .4; cursor: default; }
.value { min-width: 58px; text-align: right; font-variant-numeric: tabular-nums; color: var(--muted); font-size: 14px; }
.btn { border: 1px solid var(--line); background: var(--surface); border-radius: 999px; padding: 8px 16px; font-size: 14px; display: inline-flex; align-items: center; gap: 7px; color: var(--text); max-width: 100%; min-width: 0; text-align: left; }
.btn .mdi { --mdc-icon-size: 18px; }
.btn:hover { border-color: var(--signal-line); }
.btn.primary { background: var(--text); border-color: var(--text); color: var(--ground); }
.btn.primary .mdi { color: var(--signal); }
.btn.primary:hover { filter: brightness(1.1); }
.btn.ghost { border-color: transparent; background: none; color: var(--muted); }
.btn.ghost:hover { color: var(--text); background: var(--raised); }
.btn.armed { border-color: var(--signal); background: var(--signal-soft); }
.btn.danger { color: var(--bad); }
.btn:disabled { opacity: .45; cursor: default; }
.btn.small { padding: 5px 12px; font-size: 13px; }
.btn.pickbtn { background: var(--raised); border-color: transparent; border-radius: 14px; padding: 7px 10px 7px 12px; font-size: 14px; overflow-wrap: anywhere; }
.btn.pickbtn > .mdi:last-child { color: var(--muted); }
.ddw { position: relative; display: inline-block; width: fit-content; max-width: 100%; min-width: 0; }
.ddw.full { display: block; width: 100%; }
.ddw.full .ddm { left: 0; right: auto; }
.dd { display: inline-flex; align-items: center; gap: 8px; max-width: 100%; min-width: 0; height: 38px; font-size: 14px; color: var(--text); background: var(--raised); border: 1px solid transparent; border-radius: 12px; padding: 0 10px 0 14px; text-align: left; }
.dd.full { width: 100%; }
.dd:hover, .dd.open { border-color: var(--signal-line); }
.dd:disabled { opacity: .45; cursor: default; }
.dd .v { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.dd .v.none { color: var(--muted); }
.dd .more { font-size: 12px; font-weight: 600; color: var(--muted); background: var(--line); border-radius: 9px; padding: 0 7px; line-height: 18px; flex: none; }
.dd > .mdi { color: var(--muted); flex: none; }
.ddm { position: absolute; z-index: 30; top: calc(100% + 6px); right: 0; min-width: 100%; width: max-content; max-width: min(420px, 92vw); background: var(--surface); border: 1px solid var(--line); border-radius: 14px; padding: 6px; box-shadow: 0 12px 32px rgba(0,0,0,.22); animation: ddin .14s ease-out; transform-origin: top left; }
.ddm.up { top: auto; bottom: calc(100% + 6px); transform-origin: bottom left; }
@keyframes ddin { from { opacity: 0; transform: scale(.96); } to { opacity: 1; transform: none; } }
.ddl { max-height: 288px; overflow-y: auto; display: flex; flex-direction: column; }
.ddr { display: flex; align-items: center; gap: 8px; min-height: 36px; padding: 6px 10px; border: 0; background: none; border-radius: 10px; font-size: 14px; color: var(--text); text-align: left; }
.ddr:hover, .ddr:focus-visible { background: var(--raised); outline: none; }
.ddr.on { font-weight: 600; }
.ddr > .mdi { color: var(--signal); --mdc-icon-size: 18px; flex: none; }
.ddr .sp { width: 18px; flex: none; }
.ddr .t { min-width: 0; overflow-wrap: anywhere; }
.ddr:disabled { opacity: .4; }
.jck { position: relative; width: 18px; height: 18px; flex: none; display: block; color: #F5F3EE; }
.jck .bw { position: absolute; inset: 0; display: block; }
.jck .bx { position: absolute; inset: 0; border-radius: 5px; border: 1.5px solid var(--faint); transition: background .08s, border-color .08s; }
.jck.round .bx { border-radius: 50%; }
.jck svg { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
.jck path { stroke-dasharray: 1; stroke-dashoffset: 1; }
.jck.on .bx { background: var(--signal); border-color: var(--signal); }
.jck.on path { stroke-dashoffset: 0; }
.ddbar { display: flex; gap: 4px; padding: 0 0 6px; border-bottom: 1px solid var(--line); margin-bottom: 6px; }
.dds { margin-bottom: 6px; }
.dddone { display: none; padding-top: 6px; justify-content: flex-end; }
.wdays { display: flex; gap: 6px; flex-wrap: wrap; }
.wd { position: relative; width: 32px; height: 32px; border-radius: 50%; border: 0; background: none; color: var(--muted); font-size: 13px; font-weight: 500; display: grid; place-items: center; padding: 0; transition: color .08s; }
.wd .base, .wd .blob { position: absolute; inset: 0; border-radius: 50%; display: block; }
.wd .base { background: var(--raised); }
.wd .blob { background: var(--text); transform: scale(0); }
.wd span { position: relative; }
.wd.on { color: var(--ground); }
.wd.on .blob { transform: none; }
.wdays .wd.jl { color: var(--muted); }
.wdays .wd.jl.lit { color: var(--ground); }
/* the touch density (the Home Assistant app on a phone): the panel's sizes (spec 28) */
@media (pointer: coarse) {
  .dd { height: 48px; font-size: 15px; border-radius: 14px; }
  .ddr { min-height: 48px; font-size: 15px; }
  .jck { width: 22px; height: 22px; }
  .jck .bx { border-radius: 6px; }
  .dddone { display: flex; }
  .seg { padding: 2px; border-radius: 20px; }
  .seg button { min-height: 36px; font-size: 15px; padding: 8px 16px; }
  .switch { --swW: 56px; --swH: 32px; --kn: 24px; }
  .btn { min-height: 48px; font-size: 16px; font-weight: 600; padding: 0 22px; }
  .btn.small { min-height: 36px; font-size: 14px; padding: 0 16px; }
  .wd { width: 40px; height: 40px; }
}
.badge { font-size: 11.5px; line-height: 18px; padding: 0 7px; border-radius: 9px; background: var(--raised); color: var(--muted); font-weight: 500; white-space: nowrap; }
.badge.lit { background: var(--signal); color: var(--on-signal); font-weight: 600; }
.tag { font-size: 12px; color: var(--muted); border: 1px solid var(--line); border-radius: 999px; padding: 1px 8px; white-space: nowrap; }
.tag.sig { border-color: var(--signal-line); }

.more { border-top: 1px solid var(--line); }
.more > button { width: 100%; display: flex; align-items: center; gap: 8px; border: 0; background: none; padding: 14px 0; color: var(--muted); font-size: 14px; text-align: left; }
.more > button:hover { color: var(--text); }
.more > button .mdi { transition: transform .2s; }
.more.open > button .mdi { transform: rotate(180deg); }
.more .body { padding-bottom: 6px; }
.more .body > .row:first-child { border-top: 1px solid var(--line); }

/* look picker */
.looks { position: relative; display: flex; align-items: flex-start; gap: 12px; overflow-x: auto; padding: 4px 4px 12px; margin: 0 -4px; scrollbar-width: thin; scroll-snap-type: x proximity; }
.look { flex: none; width: 112px; border: 0; background: none; padding: 0; text-align: left; display: flex; flex-direction: column; gap: 7px; scroll-snap-align: start; }
.look .pic { width: 112px; height: 112px; border-radius: 14px; overflow: hidden; position: relative; background: #000; box-shadow: 0 0 0 1px var(--line); transition: box-shadow .15s; }
.look > span { font-size: 13.5px; line-height: 1.3; color: var(--muted); padding: 0 2px; overflow-wrap: anywhere; }
.look:hover .pic { box-shadow: 0 0 0 2px var(--signal-line); }
.look.on .pic { box-shadow: 0 0 0 3px var(--signal); }
.look.on > span { color: var(--text); font-weight: 600; }
.look:disabled { opacity: .45; cursor: default; }
.lookimg { width: 100%; height: 100%; object-fit: cover; display: block; }
.lookimg.contain { object-fit: contain; }
.looks.wide .look { width: 150px; }
.looks.wide .look .pic { width: 150px; height: 74px; }
.look .chk { position: absolute; right: 7px; top: 7px; width: 24px; height: 24px; border-radius: 12px; background: var(--signal); color: #101114; display: grid; place-items: center; box-shadow: 0 1px 4px rgba(0,0,0,.4); }
.look .chk .mdi { --mdc-icon-size: 16px; }
.art, .art svg { width: 100%; height: 100%; display: block; }
.swatches { display: flex; gap: 10px; flex-wrap: wrap; }
.sw { width: 34px; height: 34px; border-radius: 50%; border: 0; box-shadow: inset 0 0 0 1px rgba(0,0,0,.15); position: relative; padding: 0; flex: none; }
.sw.on { box-shadow: 0 0 0 2px var(--surface), 0 0 0 4px var(--signal); }
.sw.wood { border-radius: 10px; width: 52px; height: 34px; }
.sw.panel { background: conic-gradient(#FF4A1C, #FFB020, #3CCB7F, #2E9BFF, #B06CFF, #FF4A1C); }
.sw-lab { display: flex; flex-direction: column; align-items: center; gap: 4px; font-size: 12px; color: var(--muted); border: 0; background: none; padding: 0; max-width: 72px; text-align: center; }
.sw-lab.on { color: var(--text); font-weight: 600; }

/* page list */
.plist { display: flex; flex-direction: column; }
.prow { display: flex; align-items: center; gap: 12px; padding: 10px 0; border-top: 1px solid var(--line); min-width: 0; background: var(--surface); position: relative; }
.prow:first-child { border-top: 0; }
.prow.dragging { z-index: 5; box-shadow: var(--shadow-lg); border-radius: 14px; border-top-color: transparent; opacity: .96; }
.prow .handle { width: 28px; height: 44px; display: grid; place-items: center; color: var(--faint); cursor: grab; touch-action: none; flex: none; border: 0; background: none; margin-left: -8px; padding: 0; }
.prow .handle.fixed { cursor: default; opacity: .35; }
.prow .pthumb { width: 54px; height: 54px; border-radius: 10px; overflow: hidden; flex: none; background: #000; position: relative; box-shadow: 0 0 0 1px var(--line); cursor: pointer; }
.prow .ptxt { flex: 1; min-width: 0; display: flex; flex-direction: column; cursor: pointer; overflow-wrap: anywhere; }
.prow .ptxt b { font-weight: 600; }
.prow .ptxt small { color: var(--muted); font-size: 13px; }
.prow.hidden-page .pthumb, .prow.hidden-page .ptxt { opacity: .45; }
.prow .chev { color: var(--faint); cursor: pointer; }
:host([size=narrow]) .prow { gap: 10px; }
:host([size=narrow]) .prow .pthumb { width: 46px; height: 46px; }
:host([size=narrow]) .prow .chev { display: none; }

/* catalogue */
.cat { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 12px; }
.cat-item { border: 1px solid var(--line); background: var(--surface); border-radius: 16px; padding: 10px; display: flex; flex-direction: column; gap: 8px; text-align: left; position: relative; min-width: 0; }
.cat-item:hover:not(:disabled) { border-color: var(--signal-line); }
.cat-item .pic { aspect-ratio: 1; border-radius: 10px; overflow: hidden; background: #000; }
.cat-item .pic.wicon { display: grid; place-items: center; background: var(--raised); color: var(--muted); aspect-ratio: 2; }
.cat-item .pic.wicon .mdi { --mdc-icon-size: 34px; }
.cat-item .pic.wicon .wpv { width: 100%; height: 100%; }
.wpv-one { border-radius: 14px; overflow: hidden; background: var(--raised); margin: 0 auto 14px; }
.cat-item > b { font-weight: 600; font-size: 14.5px; overflow-wrap: anywhere; }
.cat-item > small { color: var(--muted); font-size: 12.5px; line-height: 1.35; overflow-wrap: anywhere; }
.cat-item:disabled { opacity: .5; cursor: default; }
.cat-item .tag { position: absolute; top: 16px; left: 16px; background: var(--surface); }

/* overview */
.glance { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 10px; padding: 4px 0 14px; }
.gl { background: var(--raised); border-radius: 14px; padding: 12px 14px; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.gl small { color: var(--muted); font-size: 12.5px; display: flex; align-items: center; gap: 6px; }
.gl small .mdi { --mdc-icon-size: 16px; }
.gl b { font-family: "PanelOS Heading", sans-serif; font-weight: 500; font-size: 22px; font-variant-numeric: tabular-nums; line-height: 1.15; overflow-wrap: anywhere; }
.gl em { font-style: normal; font-size: 12.5px; color: var(--faint); overflow-wrap: anywhere; }
.bars { display: inline-flex; gap: 2px; align-items: flex-end; height: 14px; }
.bars i { width: 4px; background: var(--faint); border-radius: 1px; opacity: .45; }
.bars i.on { background: var(--ok); opacity: 1; }

/* all panels */
.pgrid { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 16px; }
.pcard { background: var(--surface); border: 1px solid var(--line); border-radius: 18px; padding: 14px; display: flex; flex-direction: column; gap: 12px; box-shadow: var(--shadow); min-width: 0; cursor: pointer; text-align: left; }
.pcard:hover { border-color: var(--signal-line); }
.pcard .stage { height: 190px; display: grid; place-items: center; background: var(--raised); border-radius: 12px; }
.pcard .name { display: flex; align-items: center; gap: 8px; min-width: 0; }
.pcard .name b { font-size: 17px; font-family: "PanelOS Heading", sans-serif; font-weight: 500; flex: 1; min-width: 0; overflow-wrap: anywhere; }
.pcard .meta { font-size: 13px; color: var(--muted); display: flex; flex-direction: column; gap: 3px; overflow-wrap: anywhere; }
.pcard.add { border-style: dashed; box-shadow: none; display: grid; place-items: center; min-height: 300px; color: var(--muted); background: none; }
.pcard.add .mdi { --mdc-icon-size: 34px; }
.matrix { width: 100%; border-collapse: collapse; font-size: 14px; }
.matrix th, .matrix td { padding: 9px 8px; border-top: 1px solid var(--line); text-align: center; overflow-wrap: anywhere; }
.matrix th:first-child, .matrix td:first-child { text-align: left; padding-left: 0; }
.matrix thead th { border-top: 0; font-weight: 600; font-size: 13px; color: var(--muted); }
.matrix .mdi { --mdc-icon-size: 18px; }
.matrix .y { color: var(--ok); } .matrix .n { color: var(--faint); }
.scroll-x { overflow-x: auto; }

/* device frame */
.device { position: relative; flex: none; }
.device .bezel { position: absolute; inset: 0; border-radius: 22px; background: linear-gradient(160deg, color-mix(in srgb, var(--bezel) 100%, #fff 6%), var(--bezel)); box-shadow: 0 0 0 1px var(--bezel-edge), 0 18px 40px rgba(0,0,0,.18), inset 0 1px 0 rgba(255,255,255,.18); }
.device .glass { position: absolute; overflow: hidden; background: #000; border-radius: 6px; cursor: pointer; display: grid; place-items: center; }
.device .glass img { width: 100%; height: 100%; display: block; object-fit: contain; }
.device .glass .off { color: #8a877f; font-size: 13px; text-align: center; padding: 8px; }
.device .glass::after { content: ""; position: absolute; inset: 0; pointer-events: none; background: linear-gradient(125deg, rgba(255,255,255,.07), transparent 38%); }
.pv { display: flex; flex-direction: column; align-items: center; gap: 12px; width: 100%; }
.pv-title { font-size: 13px; color: var(--muted); display: flex; align-items: center; gap: 8px; flex-wrap: wrap; justify-content: center; text-align: center; }
.pv-acts { display: flex; gap: 8px; flex-wrap: wrap; justify-content: center; }

/* weather page layout editor */
.editor-wrap { display: flex; justify-content: center; padding: 6px 0 14px; }
.hcanvas { position: relative; border-radius: 14px; overflow: hidden; box-shadow: 0 0 0 1px var(--line); touch-action: none; user-select: none; -webkit-user-select: none; flex: none; }
.hcanvas > .art { position: absolute; inset: 0; width: 100%; height: 100%; }
.hcanvas.live .wbox { background: transparent; backdrop-filter: none; outline-style: dashed; }
.hcanvas.live .wbox > .mdi, .hcanvas.live .wbox > span { opacity: 0; }
.hcanvas.live .wbox.sel { outline: 2.5px solid var(--signal); }
.hcanvas .gridline { position: absolute; pointer-events: none; border-color: rgba(255,255,255,.45); border-style: dashed; border-width: 0; }
.hcanvas.light .gridline { border-color: rgba(16,17,20,.3); }
.hcanvas.light .wbox { background: rgba(16,17,20,.74); }
.wbox { position: absolute; z-index: 3; border-radius: 12px; background: rgba(16,17,20,.42); backdrop-filter: blur(3px); color: #F5F3EE; outline: 1.5px solid rgba(255,255,255,.55); outline-offset: -1px; cursor: grab; touch-action: none; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 2px; padding: 4px; text-align: center; font-size: 12px; line-height: 1.2; overflow: hidden; }
.wbox .mdi { --mdc-icon-size: 20px; }
.wbox span { overflow-wrap: anywhere; }
.wbox:hover { outline-color: #fff; }
.wbox.sel { outline: 2.5px solid var(--signal); box-shadow: 0 0 0 4px rgba(255,74,28,.28); z-index: 4; }
.wbox.bad { outline-color: #ff2a2a; background: rgba(255,0,0,.25); }
.wbox.over { outline-style: dashed; outline-color: #FFB020; background: rgba(16,17,20,.30); }
.wbox.over.sel { outline-color: var(--signal); }
.sw.tslot { display: inline-flex; align-items: center; justify-content: center; font-size: 11px; font-weight: 700; color: rgba(0,0,0,.72); }
.warnline { display: flex; gap: 8px; align-items: flex-start; margin: 0 0 14px; padding: 10px 12px; border-radius: 12px; background: rgba(255,176,32,.14); color: var(--text); font-size: 13px; line-height: 1.35; }
.warnline ha-icon { flex: none; --mdc-icon-size: 18px; color: #c27a00; }
.wbox .rs { position: absolute; right: 0; bottom: 0; width: 30px; height: 30px; cursor: nwse-resize; touch-action: none; }
.wbox .rs::after { content: ""; position: absolute; right: 4px; bottom: 4px; width: 14px; height: 14px; border-right: 3px solid #fff; border-bottom: 3px solid #fff; border-bottom-right-radius: 6px; }
.wbox.sel .rs::after { border-color: var(--signal); }
.wbar { display: flex; gap: 8px; justify-content: center; flex-wrap: wrap; padding-bottom: 12px; min-height: 48px; align-items: center; }

/* sheets */
.layer { position: fixed; inset: 0; z-index: 60; display: flex; justify-content: flex-end; }
.layer .scrim { position: absolute; inset: 0; background: var(--scrim); animation: fadein .2s; }
@keyframes fadein { from { opacity: 0; } }
/* drawers are as tall as what they hold (the page shows below a short one) */
.sheet { position: relative; z-index: 1; width: min(560px, 100%); max-height: 100%; height: auto; align-self: flex-start; border-bottom-left-radius: 22px; background: var(--ground); display: flex; flex-direction: column; box-shadow: var(--shadow-lg); animation: slidein .25s cubic-bezier(.2,.8,.2,1); }
.sheet.wide { width: min(880px, 100%); }
@keyframes slidein { from { transform: translateX(40px); opacity: 0; } }
.sheet-head { display: flex; align-items: center; gap: 8px; padding: 14px 14px 12px 22px; border-bottom: 1px solid var(--line); min-height: 64px; position: relative; flex: none; }
.sheet-head h2 { font-size: 20px; flex: 1; min-width: 0; overflow-wrap: anywhere; }
.sheet-body { flex: 0 1 auto; overflow-y: auto; padding: 18px 22px 24px; display: flex; flex-direction: column; gap: 16px; overscroll-behavior: contain; min-height: 0; }
.sheet-foot { display: flex; gap: 10px; padding: 12px 22px calc(12px + env(safe-area-inset-bottom, 0px)); border-top: 1px solid var(--line); justify-content: flex-end; align-items: center; flex-wrap: wrap; flex: none; }
.sheet-foot .grow { flex: 1; }
.sheet .grab { display: none; }
.layer.center { justify-content: center; align-items: center; }
.layer.center .sheet { height: auto; align-self: center; max-height: min(86vh, 760px); border-radius: 22px; width: min(520px, calc(100% - 32px)); animation: popin .22s cubic-bezier(.2,.8,.2,1); }
@keyframes popin { from { transform: scale(.96); opacity: 0; } }
:host([size=narrow]) .layer, :host([size=narrow]) .layer.center { align-items: flex-end; justify-content: stretch; }
:host([size=narrow]) .sheet, :host([size=narrow]) .sheet.wide, :host([size=narrow]) .layer.center .sheet { width: 100%; height: auto; max-height: calc(100% - 28px); border-radius: 22px 22px 0 0; animation: slideup .28s cubic-bezier(.2,.8,.2,1); align-self: auto; }
@keyframes slideup { from { transform: translateY(60px); opacity: 0; } }
:host([size=narrow]) .sheet .grab { display: block; width: 40px; height: 4px; border-radius: 2px; background: var(--line); position: absolute; top: 7px; left: 50%; margin-left: -20px; }
:host([size=narrow]) .sheet-head { padding: 16px 10px 10px 18px; min-height: 60px; }
:host([size=narrow]) .sheet-body { padding: 14px 16px 22px; }
:host([size=narrow]) .sheet-foot { padding: 10px 16px calc(10px + env(safe-area-inset-bottom, 0px)); }
/* a sheet drawn again does not slide in again (stronger than the narrow layout's slide-up) */
.layer.noanim .sheet, .layer.noanim .scrim { animation: none !important; }
.sec { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
.sec > h3 { font-size: 15px; font-family: "PanelOS Text", sans-serif; font-weight: 600; color: var(--text); letter-spacing: 0; display: flex; align-items: center; gap: 6px; position: relative; flex-wrap: wrap; }
.group { background: var(--surface); border: 1px solid var(--line); border-radius: 16px; padding: 2px 16px; min-width: 0; }
.group > .row:first-child { border-top: 0; }
.opt { display: flex; align-items: center; gap: 12px; padding: 12px 0; border-top: 1px solid var(--line); cursor: pointer; min-width: 0; }
.opt:first-child { border-top: 0; }
.opt { position: relative; }
.opt input[type=checkbox], .opt input[type=radio] { position: absolute; left: 0; top: 50%; width: 22px; height: 22px; margin: -11px 0 0; opacity: 0; cursor: pointer; }
.opt input:focus-visible + .jck { outline: 2px solid var(--signal); outline-offset: 2px; border-radius: 6px; }
.opt .t { flex: 1; min-width: 0; display: flex; flex-direction: column; overflow-wrap: anywhere; }
.opt .t small { color: var(--muted); font-size: 12.5px; }
.opt.disabled { cursor: default; opacity: .6; }

/* menus */
.menu { position: fixed; z-index: 80; min-width: 230px; max-width: calc(100vw - 24px); background: var(--surface); border: 1px solid var(--line); border-radius: 14px; box-shadow: var(--shadow-lg); padding: 6px; animation: popin .14s ease-out; max-height: calc(100vh - 24px); overflow-y: auto; }
.menu button { display: flex; align-items: center; gap: 12px; width: 100%; border: 0; background: none; border-radius: 9px; padding: 9px 12px; text-align: left; font-size: 14px; color: var(--text); overflow-wrap: anywhere; }
.menu button:hover { background: var(--raised); }
.menu button .mdi { --mdc-icon-size: 19px; color: var(--muted); }
.menu button.danger, .menu button.danger .mdi { color: var(--bad); }
.menu button:disabled { opacity: .45; cursor: default; }
.menu hr { border: 0; border-top: 1px solid var(--line); margin: 5px 4px; }

/* toast */
.toasts { position: fixed; left: 50%; bottom: 24px; transform: translateX(-50%); z-index: 90; display: flex; flex-direction: column; gap: 8px; align-items: center; pointer-events: none; width: max-content; max-width: calc(100vw - 32px); }
:host([size=narrow]) .toasts { bottom: calc(88px + env(safe-area-inset-bottom, 0px)); }
.toast { background: #101114; color: #F5F3EE; border-radius: 14px; padding: 11px 16px; font-size: 14px; display: flex; align-items: center; gap: 10px; box-shadow: 0 12px 32px rgba(0,0,0,.3); animation: toastin .25s ease-out; pointer-events: none; max-width: 100%; overflow-wrap: anywhere; }
.toast button { pointer-events: auto; }
:host([dark]) .toast { background: #2A2C30; }
.toast .mdi { color: var(--signal); --mdc-icon-size: 19px; }
.toast.err .mdi { color: #FF6A4D; }
.toast button { border: 0; background: none; color: #F5F3EE; font-weight: 600; padding: 0 0 0 6px; text-decoration: underline; }
@keyframes toastin { from { transform: translateY(12px); opacity: 0; } }

.syncline { display: inline-flex; align-items: center; gap: 6px; font-size: 12.5px; color: var(--faint); }
.syncline .mdi { --mdc-icon-size: 15px; }
.syncline.busy .mdi { color: var(--signal); animation: spin 1s linear infinite; }
.syncline.err { color: var(--bad); }
@keyframes spin { to { transform: rotate(360deg); } }
.fab { display: none; }
:host([size=narrow]) .fab { display: flex; position: absolute; right: 14px; bottom: calc(84px + env(safe-area-inset-bottom, 0px)); z-index: 24; border: 0; padding: 0; border-radius: 14px; overflow: hidden; box-shadow: 0 10px 28px rgba(0,0,0,.35), 0 0 0 1px var(--bezel-edge); background: #000; width: 64px; height: 64px; }
.fab img { width: 100%; height: 100%; object-fit: cover; }
.fab .mdi { color: #8a877f; margin: auto; }
.empty { color: var(--muted); font-size: 14px; padding: 6px 0 16px; }
.blank { padding: 64px 24px; text-align: center; color: var(--muted); display: flex; flex-direction: column; align-items: center; gap: 16px; }
.steps { display: flex; gap: 6px; }
.steps i { height: 4px; flex: 1; border-radius: 2px; background: var(--line); }
.steps i.on { background: var(--signal); }
.result { display: flex; gap: 10px; align-items: flex-start; font-size: 14px; padding: 7px 0; border-top: 1px solid var(--line); overflow-wrap: anywhere; }
.result:first-child { border-top: 0; }
.result .mdi { --mdc-icon-size: 18px; margin-top: 1px; }
.result .y { color: var(--ok); } .result .a { color: var(--warn); } .result .n { color: var(--faint); }
.result small { color: var(--muted); display: block; font-size: 12.5px; }
.catgrid { display: grid; grid-template-columns: repeat(auto-fill, minmax(100px, 1fr)); gap: 8px; }
.catbtn { border: 1px solid var(--line); background: var(--surface); border-radius: 14px; padding: 8px 6px; display: flex; flex-direction: column; align-items: center; gap: 6px; font-size: 12px; text-align: center; line-height: 1.25; min-width: 0; color: var(--muted); overflow-wrap: anywhere; }
.catbtn .pc { width: 44px; height: 44px; border-radius: 12px; display: grid; place-items: center; }
.catbtn .pc img { width: 30px; height: 30px; }
.catbtn.on { border-color: var(--signal); background: var(--signal-soft); color: var(--text); }
.catbtn:hover { border-color: var(--signal-line); }
.stlead { width: 40px; height: 40px; border-radius: 12px; display: grid; place-items: center; flex: none; color: #fff; }
.stlead img { width: 28px; height: 28px; }
.picto { width: 48px; height: 48px; border-radius: 14px; display: grid; place-items: center; border: 0; padding: 0; color: #fff; }
.picto img { width: 34px; height: 34px; }
.picto .mdi { --mdc-icon-size: 26px; }
.picto.on { box-shadow: 0 0 0 2px var(--surface), 0 0 0 4px var(--signal); }
.split { display: grid; grid-template-columns: minmax(0, 1fr) 250px; gap: 22px; align-items: start; }
.split .stick { position: sticky; top: 0; display: flex; flex-direction: column; align-items: center; gap: 8px; }
:host([size=narrow]) .split { grid-template-columns: minmax(0, 1fr); }
:host([size=narrow]) .split .stick { position: static; order: -1; }
.stpv { width: 220px; aspect-ratio: 1; border-radius: 14px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; color: #fff; padding: 16px; text-align: center; box-shadow: 0 0 0 1px var(--line); }
.stpv img { width: 72px; height: 72px; }
.stpv .mdi { --mdc-icon-size: 64px; }
.stpv b { font-family: "PanelOS Heading", sans-serif; font-weight: 500; font-size: 19px; line-height: 1.2; overflow-wrap: anywhere; }
.stpv small { font-size: 13px; opacity: .85; overflow-wrap: anywhere; }
.stpv .bt { margin-top: 4px; background: #F5F3EE; color: #101114; font-weight: 600; border-radius: 999px; padding: 4px 14px; font-size: 12.5px; }
.note { font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
.yaml { font-family: ui-monospace, "DejaVu Sans Mono", monospace; font-size: 12.5px; background: var(--sunken); border: 1px solid var(--line); border-radius: 12px; padding: 12px 14px; white-space: pre-wrap; overflow-wrap: anywhere; color: var(--text); }
.tilegrid { display: grid; gap: 8px; padding: 4px 0 12px; }
.tslot { border: 1px dashed var(--line); background: none; border-radius: 14px; min-height: 64px; display: flex; align-items: center; gap: 10px; padding: 8px 10px; text-align: left; min-width: 0; color: var(--muted); font-size: 13px; }
.tslot.full { border-style: solid; background: var(--raised); color: var(--text); }
.tslot:hover { border-color: var(--signal-line); }
.tslot .t { display: flex; flex-direction: column; min-width: 0; overflow-wrap: anywhere; }
.tslot .t b { font-weight: 600; font-size: 13.5px; }
.tslot .t small { color: var(--muted); font-size: 12px; }
.tslot { position: relative; }
.tslot.dragging { z-index: 5; box-shadow: var(--shadow-lg); opacity: .96; }
.tslot .handle { width: 22px; height: 40px; display: grid; place-items: center; color: var(--faint); cursor: grab; touch-action: none; flex: none; margin: 0 -6px 0 -4px; }
.tape { font-family: "PanelOS Heading", sans-serif; font-size: 11px; letter-spacing: .08em; background: #111; color: #f2f2f2; border-radius: 3px; padding: 1px 5px; text-transform: uppercase; align-self: flex-start; overflow-wrap: anywhere; }
.picker-list { display: flex; flex-direction: column; max-height: 360px; overflow-y: auto; border: 1px solid var(--line); border-radius: 14px; background: var(--surface); }
.picker-list > button { display: flex; align-items: center; gap: 12px; border: 0; border-top: 1px solid var(--line); background: none; padding: 10px 14px; text-align: left; min-width: 0; }
.picker-list > button:first-child { border-top: 0; }
.picker-list > button:hover { background: var(--raised); }
.picker-list > button.on { background: var(--signal-soft); }
.picker-list .t { flex: 1; min-width: 0; display: flex; flex-direction: column; overflow-wrap: anywhere; }
.picker-list .t span { font-size: 14px; }
.picker-list .t small { color: var(--muted); font-size: 12px; }
.picker-list .empty { padding: 14px; }
.res { display: flex; flex-direction: column; }
.res .item { display: flex; align-items: center; gap: 12px; padding: 9px 0; border-top: 1px solid var(--line); flex-wrap: wrap; }
.res .item:first-child { border-top: 0; }
.res .name { flex: 1 1 160px; min-width: 0; font-size: 14px; overflow-wrap: anywhere; }
.res .name small { color: var(--muted); margin-left: 8px; }
.res .size { color: var(--muted); font-size: 13px; font-variant-numeric: tabular-nums; min-width: 64px; text-align: right; }
/* sounds */
.spin { animation: spin 1s linear infinite; }
/* loading: the PanelOS start-up keying, three dots lighting in turn */
.pload { display: flex; flex-direction: column; align-items: center; gap: 12px; padding: 36px 0 30px; color: var(--muted); font-size: 13px; }
.pload .d { display: flex; gap: 9px; }
.pload .d i { width: 10px; height: 10px; border-radius: 50%; background: var(--signal); opacity: .22; animation: pdot 1.2s ease-in-out infinite; }
.pload .d i:nth-child(2) { animation-delay: .2s; }
.pload .d i:nth-child(3) { animation-delay: .4s; }
@keyframes pdot { 0%, 60%, 100% { opacity: .22; transform: scale(.85); } 30% { opacity: 1; transform: scale(1); } }
.picker-list .more { padding: 12px 14px; color: var(--muted); font-size: 13px; text-align: center; border-top: 1px solid var(--line); }
.sndctl { display: flex; align-items: center; gap: 8px; min-width: 0; justify-content: flex-end; }
.sndpick { border: 1px solid transparent; background: var(--raised); border-radius: 12px; padding: 7px 8px 7px 14px; font-size: 14px; color: var(--text); display: inline-flex; align-items: center; gap: 8px; text-align: left; min-width: 0; max-width: 100%; }
.sndpick:hover { border-color: var(--signal-line); }
.sndpick span { overflow-wrap: anywhere; min-width: 0; }
.sndpick small { font-size: 12px; color: var(--muted); border: 1px solid var(--line); border-radius: 999px; padding: 0 7px; white-space: nowrap; }
.sndpick .mdi { color: var(--muted); --mdc-icon-size: 18px; }
.playbtn { width: 36px; height: 36px; border-radius: 18px; border: 1px solid var(--line); background: var(--surface); color: var(--text); display: grid; place-items: center; flex: none; padding: 0; }
.playbtn .mdi { --mdc-icon-size: 20px; }
.playbtn:hover { border-color: var(--text); }
.playbtn.on { background: var(--text); color: var(--ground); border-color: var(--text); }
.sndlist { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.snd { display: flex; align-items: center; gap: 12px; padding: 7px 8px; border-radius: 12px; cursor: pointer; min-width: 0; }
.snd:hover { background: var(--raised); }
.linkbtn { border: 0; background: none; padding: 0; margin-top: 2px; color: var(--text); text-decoration: underline; font: inherit; font-size: 13px; cursor: pointer; text-align: left; }
.playbtn.ma { display: inline-flex; align-items: center; justify-content: center; color: var(--muted); background: none; border: 0; pointer-events: none; }
.snd.on { background: var(--signal-soft); }
.snd .t { flex: 1; min-width: 0; display: flex; flex-direction: column; align-items: flex-start; border: 0; background: none; padding: 0; text-align: left; color: var(--text); overflow-wrap: anywhere; }
.snd .t span { font-size: 14.5px; max-width: 100%; }
.snd .t small { color: var(--muted); font-size: 12.5px; max-width: 100%; }
.snd .chk { color: var(--signal); display: grid; place-items: center; flex: none; }
.snd.busy { cursor: default; }
.snd.busy:hover { background: none; }
.snd.busy .lead { width: 36px; height: 36px; border-radius: 18px; }
.snd.busy .lead .mdi { color: var(--signal); }
.snddel { border: 0; background: none; color: var(--muted); width: 36px; height: 36px; border-radius: 18px; display: grid; place-items: center; flex: none; padding: 0; }
.snddel:hover { background: var(--sunken); color: var(--text); }
.upl { align-self: flex-start; }
/* alarms */
.alarms { display: flex; flex-direction: column; padding-bottom: 6px; }
.alarm { display: flex; align-items: center; gap: 14px; padding: 10px 0; border-top: 1px solid var(--line); min-width: 0; }
.alarm:first-child { border-top: 0; }
.alarm .open { flex: 1; min-width: 0; display: flex; align-items: center; gap: 14px; border: 0; background: none; padding: 0; text-align: left; color: var(--text); }
.alarm .tm { font-family: "PanelOS Heading", "Space Grotesk", sans-serif; font-size: 26px; line-height: 1.1; font-variant-numeric: tabular-nums; flex: none; }
.alarm .d { flex: 1; min-width: 0; display: flex; flex-direction: column; color: var(--muted); font-size: 13.5px; overflow-wrap: anywhere; }
.alarm .d b { color: var(--text); font-weight: 500; font-size: 15px; }
.alarm .open:hover .d b { text-decoration: underline; text-underline-offset: 3px; }
.alarm.off .tm, .alarm.off .d { opacity: .5; }
:host([size=narrow]) .alarm, :host([size=narrow]) .alarm .open { gap: 12px; }
:host([size=narrow]) .alarm .tm { font-size: 22px; }
.replies { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; width: 100%; min-width: 0; }
.replies .rfield, .replies .rline { display: flex; min-width: 0; width: 100%; }
.replies .rfield { flex-direction: column; gap: 4px; }
.replies .rline { align-items: center; gap: 4px; }
.replies .rlong { color: var(--bad); font-size: 13px; padding-left: 4px; }
.inp.alarm-time { width: 210px; max-width: 100%; font-size: 34px; text-align: center; font-family: "PanelOS Heading", "Space Grotesk", sans-serif; font-variant-numeric: tabular-nums; padding: 8px 12px; }
.only-narrow { display: none; }
:host([size=narrow]) .only-narrow { display: flex; }
:host([size=narrow]) .only-wide { display: none !important; }
:host(:not([size=wide])) .only-xl { display: none !important; }
.pv-card { flex-direction: column; align-items: center; gap: 12px; padding: 16px 12px 14px; }
@media (prefers-reduced-motion: reduce) { *, *::before, *::after { animation: none !important; transition: none !important; } }
`;

// ---------------------------------------------------------------- the page

class PanelOSPage extends HTMLElement {
  constructor() {
    super();
    this.attachShadow({ mode: "open" });
    // an open info bubble follows its button when something scrolls or the window changes size
    const follow = () => { if (this._ui && (this._ui.info || this._ui.allInfo)) requestAnimationFrame(() => this._placePops()); };
    this.shadowRoot.addEventListener("scroll", follow, true);
    window.addEventListener("resize", follow);
    // an open dropdown closes on a click anywhere else (its field and menu keep their clicks)
    this.shadowRoot.addEventListener("click", () => { if (this._ui && this._ui.dd) { this._ui.dd = null; this._ui.ddq = ""; this._render(); } });
    this._data = null;
    this._satCfg = {}; // voice satellites' wake word configuration, by entity
    this._reg = null; // registries from the websocket API when hass has none
    this._path = "";
    this._routed = false;
    this._prefix = "/panelos";
    this._static = "/panelos_static";
    this._ui = { info: null, more: {}, sheet: null, armed: null, sel: null, allMore: false, playing: null, uploading: null, delSounds: new Set(), dd: null, ddq: "" };
    // sound previews: the one playing, and the uploaded sounds fetched so far (id -> object URL)
    this._audio = null;
    this._soundUrls = {};
    this._sync = {};
    this._pics = {};
    this._pic = { timer: null, busy: false, due: 0, allAt: 0 };
    this._liveLooks = readLiveLooks();
    this._lookChangedAt = 0;
    this._menu = null;
    this._pending = false;
    this._dragging = false;
    this._toastBox = h("div", { class: "toasts", "aria-live": "polite" });
    this._layer = h("div");
    this._onVisibility = () => { this._onUse(); this._schedulePicture(0); };
    // Use of the page keeps the live preview going: only while this is the active window (a pointer over a window in
    // the background does not count), and making the window active again counts as use.
    this._use = { at: Date.now(), paused: false };
    this._onUse = (ev) => {
      if (document.visibilityState !== "visible" || !document.hasFocus()) return;
      const now = Date.now();
      if (!this._use.paused && now - this._use.at < 1000) return;
      this._use.at = now;
      if (this._use.paused) {
        this._use.paused = false;
        this._resumeRender(ev);
        this._schedulePicture(0);
      }
    };
    // The page drawn again after the pause, not under a press that is starting: this listener (on the window) runs
    // before the press is noted (_onPointerDown), so a redraw now would replace the element under the pointer and its
    // click would be lost. After a press or a touch the redraw waits until the press is noted (and then until it ends);
    // after a key until the key is up (Space clicks a button on key up).
    this._resumeRender = (ev) => {
      const type = ev?.type;
      if (type === "pointerdown" || type === "touchstart" || type === "mousedown") {
        setTimeout(() => this._render(), 0);
      } else if (type === "keydown") {
        let timer = null;
        const go = () => {
          window.removeEventListener("keyup", go, true);
          clearTimeout(timer);
          setTimeout(() => this._render(), 0);
        };
        timer = setTimeout(go, 1500);
        window.addEventListener("keyup", go, true);
      } else this._requestRender();
    };
    this._resize = new ResizeObserver(() => this._fit());
    this._onClick = (ev) => {
      const path = ev.composedPath();
      if (this._menu && !path.includes(this._menu)) this._closeMenu();
      if (this._ui.info && !path.some((n) => n.classList && (n.classList.contains("pop") || n.classList.contains("info")))) {
        this._ui.info = null;
        this._render();
      }
      this._endPress();
    };
    // In a still sheet (_render leaves it alone) a click anywhere but on a bubble or an ⓘ closes the open bubble in
    // place, before the click's own handler (a row that is picked, a play button: they keep the click to themselves).
    this._onClickCapture = (ev) => {
      if (!this._ui.info || !this._stillSheet()) return;
      if (ev.composedPath().some((n) => n.classList && (n.classList.contains("pop") || n.classList.contains("info")))) return;
      this._closePopsInPlace();
    };
    // the jelly controls (spec 28.15): bound once, so the listeners can be added and removed
    this._jRuns = new Map();
    this._jShapes = new Map();
    this._jDown = (ev) => this._jPointerDown(ev);
    this._jMove = (ev) => this._jPointerMove(ev);
    this._jUp = (ev) => this._jPointerUp(ev);
    this._jClick = (ev) => this._jOnClick(ev);
    this._jChange = (ev) => this._jOnChange(ev);
    this._onKey = (ev) => {
      if (ev.key !== "Escape") return;
      if (this._menu) this._closeMenu();
      else if (this._ui.sheet) this._closeSheet();
    };
    this._onFocusOut = () => setTimeout(() => this._flush(), 0);
    // A press lasts from pointerdown until its click: the page is not drawn again in between, so the element
    // under the pointer is still there when the click comes (on touch screens the click comes after the finger
    // has left; a text field that saves on leaving would otherwise redraw the page between the two). The click
    // ends the press (_onClick, after the element's own handler), and what it changed is drawn at once.
    this._press = 0;
    this._onPointerDown = () => {
      clearTimeout(this._pressTimer);
      this._press = Date.now();
    };
    this._onPointerUp = () => {
      if (!this._press) return;
      clearTimeout(this._pressTimer);
      this._pressTimer = setTimeout(() => this._endPress(), 450);
    };
    this._endPress = () => {
      clearTimeout(this._pressTimer);
      this._press = 0;
      this._flush();
    };
  }

  // -- Home Assistant's properties -------------------------------------------

  set hass(hass) {
    const first = !this._hass;
    const old = this._hass;
    this._hass = hass;
    // a renamed or moved panel (or a new area) shows its new name at once
    if (!first && (hass?.devices !== old?.devices || hass?.areas !== old?.areas)) this._requestRender();
    const dark = Boolean(hass?.themes?.darkMode);
    if (dark !== this.hasAttribute("dark")) {
      this.toggleAttribute("dark", dark);
      if (!first) this._requestRender(); // the brand pattern follows the theme
    }
    if (first && this.isConnected) this._start();
    this._liveSoon();
    this._watchSat(hass);
  }

  get hass() {
    return this._hass;
  }

  set narrow(value) {
    this.toggleAttribute("ha-narrow", Boolean(value));
  }

  set panel(value) {
    this._static = value?.config?.static_url || "/panelos_static";
    if (value?.url_path) this._prefix = `/${value.url_path}`;
  }

  set route(route) {
    if (route?.prefix) this._prefix = route.prefix;
    this._setPath(route?.path || "");
  }

  connectedCallback() {
    if (!document.getElementById("panelos-fonts")) {
      document.head.append(h("style", { id: "panelos-fonts" }, FONTS.replaceAll("{STATIC}", this._static)));
    }
    document.addEventListener("visibilitychange", this._onVisibility);
    window.addEventListener("focus", this._onUse);
    for (const ev of USE_EVENTS) window.addEventListener(ev, this._onUse, { capture: true, passive: true });
    this.shadowRoot.addEventListener("scroll", this._onUse, { capture: true, passive: true });
    this.shadowRoot.addEventListener("click", this._onClick);
    this.shadowRoot.addEventListener("click", this._onClickCapture, true);
    // the jelly controls: their press, drag and change, before the controls' own handlers (a handler may draw the
    // page again at once)
    this.shadowRoot.addEventListener("pointerdown", this._jDown, true);
    this.shadowRoot.addEventListener("pointermove", this._jMove, true);
    this.shadowRoot.addEventListener("click", this._jClick, true);
    this.shadowRoot.addEventListener("change", this._jChange, true);
    window.addEventListener("pointerup", this._jUp, true);
    window.addEventListener("pointercancel", this._jUp, true);
    this.shadowRoot.addEventListener("focusout", this._onFocusOut);
    this.shadowRoot.addEventListener("pointerdown", this._onPointerDown, true);
    window.addEventListener("pointerup", this._onPointerUp, true);
    window.addEventListener("pointercancel", this._onPointerUp, true);
    window.addEventListener("keydown", this._onKey);
    this._resize.observe(this);
    if (this._hass && !this._unsub) this._start();
    this._fit();
    this._render();
  }

  disconnectedCallback() {
    document.removeEventListener("visibilitychange", this._onVisibility);
    window.removeEventListener("focus", this._onUse);
    for (const ev of USE_EVENTS) window.removeEventListener(ev, this._onUse, { capture: true });
    this.shadowRoot.removeEventListener("scroll", this._onUse, { capture: true });
    window.removeEventListener("keydown", this._onKey);
    window.removeEventListener("pointerup", this._onPointerUp, true);
    window.removeEventListener("pointercancel", this._onPointerUp, true);
    window.removeEventListener("pointerup", this._jUp, true);
    window.removeEventListener("pointercancel", this._jUp, true);
    this._resize.disconnect();
    this._stopSound();
    clearTimeout(this._pic.timer);
    this._pic.timer = null;
    if (this._unsub) {
      this._unsub.then((unsub) => unsub()).catch(() => {});
      this._unsub = null;
    }
    if (this._notesUnsub) {
      this._notesUnsub.then((unsub) => unsub()).catch(() => {});
      this._notesUnsub = null;
    }
  }

  _fit() {
    const w = this.clientWidth || window.innerWidth;
    const size = w >= 1160 ? "wide" : w >= 861 ? "medium" : "narrow";
    // a wide window has room for a half again bigger live picture
    const big = w >= 1480;
    if (big !== this.hasAttribute("pvbig")) {
      this.toggleAttribute("pvbig", big);
      if (this.getAttribute("size") === size) this._render();
    }
    if (this.getAttribute("size") === size) {
      if (this._lastH !== this.clientHeight) {
        this._lastH = this.clientHeight;
        this._requestRender();
      }
      return;
    }
    this._lastH = this.clientHeight;
    this.setAttribute("size", size);
    this._render();
  }

  _start() {
    this._unsub = this._hass.connection.subscribeMessage((data) => this._receive(data), { type: "panelos/page/subscribe" });
    this._unsub.catch((err) => this._toast(`PanelOS: ${err.message || err}`, null, true));
    this._loadRegistries();
    // Home Assistant's notifications: the panels show the new ones; the page lists what is there now
    this._notes = {};
    this._notesUnsub = this._hass.connection.subscribeMessage((msg) => {
      if (msg.type === "current") this._notes = { ...(msg.notifications || {}) };
      else if (msg.type === "removed") for (const id of Object.keys(msg.notifications || {})) delete this._notes[id];
      else Object.assign(this._notes, msg.notifications || {});
      this._requestRender();
    }, { type: "persistent_notification/subscribe" });
    this._notesUnsub.catch(() => { /* an older Home Assistant: no list */ });
  }

  // The notifications now, newest first.
  _noteList() {
    return Object.values(this._notes || {}).sort((a, b) => String(b.created_at || "").localeCompare(String(a.created_at || "")));
  }

  // Each notification: its title (or the start of its text), the text, when it came, and a Dismiss button.
  _notesCard(p) {
    const list = this._noteList();
    const ago = (t) => {
      const m = Math.max(0, Math.round((Date.now() - new Date(t).getTime()) / 60000));
      return !t ? "" : m < 1 ? "just now" : m < 60 ? `${m} min ago` : m < 1440 ? `${Math.round(m / 60)} h ago` : `${Math.round(m / 1440)} d ago`;
    };
    const strip = (s) => String(s || "").replace(/[*_`#>]/g, "").replace(/\[([^\]]+)\]\([^)]+\)/g, "$1").trim();
    return this._card("Notifications", { info: `Home Assistant's notifications. A new one shows on ${this._pname(p)} as it comes (Display → New notifications); here are the ones there now.` },
      list.length ? h("div", { class: "below" }, list.map((n, k) => {
        const text = strip(n.message);
        // with a title: the title and the text under it; without one: the text itself
        const title = strip(n.title) || text || "Notification";
        const row = this._row(title, this._btn("Dismiss", () => this._hass.callService("persistent_notification", "dismiss", { notification_id: n.notification_id }), { cls: "small ghost" }),
          { lead: h("span", { class: "lead" }, icon("bell-outline")), sub: strip(n.title) && text ? [text, h("br"), ago(n.created_at)] : ago(n.created_at), keep: true, cls: "atop" });
        if (k === 0) row.classList.add("first");
        return row;
      })) : h("div", { class: "empty" }, "No notifications."));
  }

  async _loadRegistries() {
    if (this._hass.devices && this._hass.areas && this._hass.entities) return;
    try {
      const [areas, devices, entities] = await Promise.all([
        this._hass.callWS({ type: "config/area_registry/list" }),
        this._hass.callWS({ type: "config/device_registry/list" }),
        this._hass.callWS({ type: "config/entity_registry/list" }),
      ]);
      this._reg = {
        areas: Object.fromEntries(areas.map((a) => [a.area_id, a])),
        devices: Object.fromEntries(devices.map((d) => [d.id, d])),
        entities: Object.fromEntries(entities.map((e) => [e.entity_id, e])),
      };
      this._requestRender();
    } catch (_err) {
      // names and areas then come from the states only
    }
  }

  _receive(data) {
    const first = !this._data;
    // While a change is on its way, an update sent before it (with the older settings) does not undo it
    // on the page: the local settings stay until every change has its answer.
    const flying = this._flying || {};
    for (const np of data.panels || []) {
      const old = flying[np.entry_id] > 0 && this._panelById(np.entry_id);
      if (old) np.settings = old.settings;
    }
    if (flying.house > 0 && this._data?.house && data.house) data.house = { ...this._data.house, active: data.house.active };
    const cmd = this._cmdFly || {};
    for (const np of data.panels || []) {
      for (const [key, f] of Object.entries(cmd[np.entry_id] || {})) {
        if (Date.now() > f.until || (np.states && np.states[key] === f.value)) delete cmd[np.entry_id][key];
        else if (np.states) np.states[key] = f.value;
      }
    }
    this._data = data;
    for (const [id, s] of Object.entries(this._sync)) {
      if (s.state === "wait") this._syncDone(id);
    }
    if (first && !this._routed) this._routed = true;
    this._requestRender();
    if (first) this._schedulePicture(0);
  }

  // -- registries, names, entities ----------------------------------------------

  get _areas() {
    return this._hass?.areas || this._reg?.areas || {};
  }

  get _devices() {
    return this._hass?.devices || this._reg?.devices || {};
  }

  get _entityReg() {
    return this._hass?.entities || this._reg?.entities || {};
  }

  _device(p) {
    return (p?.device_id && this._devices[p.device_id]) || null;
  }

  _pname(p) {
    if (!p) return "Panel";
    const d = this._device(p);
    return (d && (d.name_by_user || d.name)) || p.device?.name || p.title || "Panel";
  }

  _parea(p) {
    const d = this._device(p);
    return (d?.area_id && this._areas[d.area_id]?.name) || "";
  }

  _pareaId(p) {
    return this._device(p)?.area_id || null;
  }

  _pmodel(p) {
    const d = p?.device || {};
    return [d.manufacturer, d.model].filter(Boolean).join(" ") || "PanelOS panel";
  }

  _caps(p) {
    return Array.isArray(p?.caps) && p.caps.length ? p.caps : DEFAULT_CAPS;
  }

  _has(p, cap) {
    return this._caps(p).includes(cap);
  }

  _reason(p, need) {
    return (NEEDS_REASON[need] || "Not on {p}.").replace("{p}", this._pname(p));
  }

  _pageSupported(p, kind) {
    const t = PAGE_TYPES[kind];
    if (!t) return { ok: false, why: "Unknown page type." };
    for (const need of t.needs) if (!this._has(p, need)) return { ok: false, why: this._reason(p, need) };
    return { ok: true };
  }

  _lookSupported(p, kind, variant) {
    if (variant > 0 && LOOK_NEEDS_IMAGES.has(kind) && !this._has(p, "images")) return { ok: false, why: this._reason(p, "images") };
    return { ok: true };
  }

  _sections(p) {
    return SECTIONS.filter((s) => !s.needs || this._has(p, s.needs));
  }

  _earea(id) {
    const e = this._entityReg[id];
    if (!e) return null;
    if (e.area_id) return e.area_id;
    return (e.device_id && this._devices[e.device_id]?.area_id) || null;
  }

  _ename(id) {
    const st = this._hass?.states?.[id];
    return st?.attributes?.friendly_name || this._entityReg[id]?.name || id;
  }

  _eicon(id) {
    const st = this._hass?.states?.[id];
    const reg = this._entityReg[id];
    if (st?.attributes?.icon) return st.attributes.icon;
    if (reg?.icon) return reg.icon;
    const domain = id.split(".")[0];
    const dc = st?.attributes?.device_class;
    if (domain === "sensor" && SENSOR_ICONS[dc]) return SENSOR_ICONS[dc];
    if (domain === "binary_sensor" && SAFETY[dc]) return SAFETY[dc].icon;
    return DOMAIN_ICONS[domain] || "circle-outline";
  }

  // Entities of some domains, the panel's area first, then by name. Same name
  // twice: the integration (or the area) in brackets.
  _ents(domains, { p = null, filter = null } = {}) {
    const states = this._hass?.states || {};
    const area = p ? this._pareaId(p) : null;
    const list = Object.keys(states)
      .filter((id) => domains.includes(id.split(".")[0]) && (!filter || filter(states[id])))
      .map((id) => ({ id, name: this._ename(id), area: this._earea(id) }));
    const seen = {};
    for (const e of list) seen[e.name] = (seen[e.name] || 0) + 1;
    for (const e of list) {
      if (seen[e.name] > 1) {
        const extra = (e.area && this._areas[e.area]?.name) || platformName(this._entityReg[e.id]?.platform) || e.id;
        e.name = `${e.name} (${extra})`;
      }
    }
    return list.sort((a, b) => (b.area === area && area ? 1 : 0) - (a.area === area && area ? 1 : 0) || a.name.localeCompare(b.name));
  }

  _areaName(id) {
    return (id && this._areas[id]?.name) || "";
  }

  // -- routing ---------------------------------------------------------------------

  _route() {
    const parts = (this._path || "").split("/").filter(Boolean);
    if (parts[0] === "all") return { view: "all" };
    if (parts[0] === "p" && parts[1]) {
      if (parts[2] === "pages" && parts[3] != null) return { view: "page", panel: decodeURIComponent(parts[1]), page: Number(parts[3]) };
      return { view: parts[2] || "overview", panel: decodeURIComponent(parts[1]) };
    }
    return { view: "overview", panel: null };
  }

  _setPath(path) {
    if (this._routed && path === this._path) return;
    const before = this._route();
    this._path = path;
    this._routed = true;
    const now = this._route();
    this._ui.info = null;
    this._ui.sel = null;
    if (before.view !== now.view || before.panel !== now.panel || before.page !== now.page) this._stopSound();
    // a page shown while its settings were open: the panel goes back to its start page on leaving them
    if (this._pageLive && (now.view !== "page" || now.panel !== this._pageLive.entry)) {
      const shown = this._panelById(this._pageLive.entry);
      const failed = this._pageLive.failed;
      this._pageLive = null;
      if (shown?.connected && !failed) this._hass.callWS({ type: "panelos/page/command", entry_id: shown.entry_id, key: "page", value: 0 }).catch(() => {});
    }
    // the screensaver shown while its settings were changed: the panel comes back on leaving them
    if (this._saverShown && (now.view !== "display" || now.panel !== this._saverShown)) {
      const shown = this._panelById(this._saverShown);
      this._saverShown = null;
      if (shown?.connected) this._command(shown, "screen", true);
    }
    if (this._ui.sheet && !this._ui.sheet.keep) {
      this._ui.sheet = null;
      this._discardUntouched();
    }
    this._render();
    if (before.view !== now.view || before.panel !== now.panel || before.page !== now.page) {
      const main = this.shadowRoot.querySelector(".main");
      if (main) main.scrollTop = 0;
    }
    this._schedulePicture(0);
  }

  _go(path, replace = false) {
    const url = this._prefix + path;
    if (replace) history.replaceState(null, "", url);
    else history.pushState(null, "", url);
    window.dispatchEvent(new CustomEvent("location-changed", { detail: { replace } }));
    this._setPath(path);
  }

  _panelPath(p, section) {
    return `/p/${encodeURIComponent(p.entry_id)}${section && section !== "overview" ? `/${section}` : ""}`;
  }

  get _panels() {
    return this._data?.panels || [];
  }

  _panelById(id) {
    return this._panels.find((p) => p.entry_id === id) || null;
  }

  _cur() {
    const r = this._route();
    return this._panelById(r.panel) || this._panels[0] || null;
  }

  // -- saving, with the sync line ------------------------------------------------------

  _busy(ids) {
    for (const id of ids) {
      clearTimeout(this._sync[id]?.timer);
      this._sync[id] = { state: "busy" };
    }
    this._refreshSync();
  }

  _settle(ids, ok) {
    for (const id of ids) {
      clearTimeout(this._sync[id]?.timer);
      if (!ok) this._sync[id] = { state: "err", timer: setTimeout(() => this._syncDone(id), 6000) };
      // saved: the panel has it once the next update of this page arrives
      // (the integration sends the new settings to the panel first)
      else this._sync[id] = { state: "wait", timer: setTimeout(() => this._syncDone(id), SYNC_FALLBACK_MS) };
    }
    this._refreshSync();
  }

  _syncDone(id) {
    clearTimeout(this._sync[id]?.timer);
    delete this._sync[id];
    this._refreshSync();
  }

  _syncText(p) {
    const s = this._sync[p.entry_id]?.state;
    if (s === "busy" || s === "wait") return ["busy", "sync", "Sending to the panel…"];
    if (s === "err") return ["err", "alert-circle-outline", "Not saved"];
    return p.connected ? ["", "check", "Up to date on the panel"] : ["", "cloud-off-outline", "Saved · the panel is offline"];
  }

  _syncLine(p) {
    const [cls, ic, text] = this._syncText(p);
    return h("span", { class: `syncline ${cls}`, "data-sync": p.entry_id, role: "status" }, icon(ic), text);
  }

  _refreshSync() {
    this.shadowRoot.querySelectorAll("[data-sync]").forEach((e) => {
      const p = this._panelById(e.dataset.sync);
      if (!p) return;
      const [cls, ic, text] = this._syncText(p);
      e.className = `syncline ${cls}`;
      e.replaceChildren(icon(ic), text);
    });
  }

  async _options(p, changes, { optimistic = null, toast = null, undo = null } = {}) {
    const before = clone(p.settings);
    if (optimistic) {
      optimistic(p.settings);
      // (not under a finger that is still dragging a slider: drawn once it lets go)
      if (this._interacting()) this._pending = true;
      else this._render();
    }
    this._busy([p.entry_id]);
    const fly = (this._flying = this._flying || {});
    fly[p.entry_id] = (fly[p.entry_id] || 0) + 1;
    try {
      await this._hass.callWS({ type: "panelos/page/options", entry_id: p.entry_id, options: changes });
      this._settle([p.entry_id], true);
      if (toast) this._toast(toast, undo);
      this._schedulePicture(PICTURE_AFTER_CHANGE_MS);
      return true;
    } catch (err) {
      p.settings = before;
      this._settle([p.entry_id], false);
      this._toast(err.message || String(err), null, true);
      this._render();
      return false;
    } finally {
      fly[p.entry_id] -= 1;
    }
  }

  // The stored page list (hidden pages too).
  _pages(p) {
    return p?.settings?.all_pages || [];
  }

  _savePages(p, pages, opts = {}) {
    const clean = pages.map((pg) => ({
      kind: pg.kind,
      variant: pg.variant || 0,
      entities: [...(pg.entities || [])],
      options: clone(pg.options || {}),
      hidden: Boolean(pg.hidden),
    }));
    return this._options(p, { pages: clean }, { ...opts, optimistic: (s) => { s.all_pages = clean; } });
  }

  _setPage(p, index, patch, opts) {
    this._lookChangedAt = Date.now();
    const pages = clone(this._pages(p));
    pages[index] = { ...pages[index], ...patch };
    return this._savePages(p, pages, opts);
  }

  _setPageOption(p, index, key, value, opts) {
    this._lookChangedAt = Date.now();
    const pages = clone(this._pages(p));
    const options = { ...(pages[index].options || {}) };
    if (value === undefined) delete options[key];
    else options[key] = value;
    pages[index].options = options;
    return this._savePages(p, pages, opts);
  }

  _house() {
    const hs = this._data?.house || {};
    return {
      critical: clone(hs.critical || []), own_critical: clone(hs.own_critical || []), dismissed: clone(hs.dismissed || []),
      status: clone(hs.status || []), sounds: clone(hs.sounds || []),
    };
  }

  async _saveHouse(mutate, { toast = null, undo = null } = {}) {
    if (this._fresh && !this._fresh.adding) this._fresh.touched = true;
    const house = this._house();
    mutate(house);
    const before = this._data.house;
    this._data.house = { ...house, active: before?.active || { critical: [], status: [] } };
    // a text field that saved on losing focus: drawn again once the next field or button has it
    this._requestRender();
    const ids = this._panels.map((p) => p.entry_id);
    this._busy(ids);
    const fly = (this._flying = this._flying || {});
    fly.house = (fly.house || 0) + 1;
    try {
      await this._hass.callWS({ type: "panelos/page/house", house });
      this._settle(ids, true);
      if (toast) this._toast(toast, undo);
      return true;
    } catch (err) {
      this._data.house = before;
      this._settle(ids, false);
      this._toast(err.message || String(err), null, true);
      this._render();
      return false;
    } finally {
      fly.house -= 1;
    }
  }

  // The panel's alarms live on the panel: the whole list goes to it, it reports them back.
  async _saveAlarms(p, list, { toast = null, undo = null } = {}) {
    const clean = list.slice(0, ALARMS_MAX).map((a) => ({
      h: a.h, m: a.m, days: [...a.days], label: a.label || "", on: Boolean(a.on), tone: a.tone || "auto", tone_name: a.tone_name || "",
    }));
    const before = p.alarms;
    p.alarms = clean;
    this._render();
    this._busy([p.entry_id]);
    try {
      await this._hass.callWS({ type: "panelos/page/alarms", entry_id: p.entry_id, alarms: clean });
      this._settle([p.entry_id], true);
      if (toast) this._toast(toast, undo);
      this._schedulePicture(PICTURE_AFTER_CHANGE_MS);
      return true;
    } catch (err) {
      p.alarms = before;
      this._settle([p.entry_id], false);
      this._toast(err.message || String(err), null, true);
      this._render();
      return false;
    }
  }

  // While a page's settings are open the panel shows that page (a hidden page cannot be shown): true
  // when it does. Asked once per page; the panel goes back to its start page when they are left.
  _livePage(p, i) {
    const pages = this._pages(p);
    if (!p?.connected || !pages[i] || (i > 0 && pages[i].hidden)) return false;
    const at = pages.slice(0, i).filter((x, k) => k === 0 || !x.hidden).length;
    const key = `${p.entry_id}:${at}`;
    if (this._pageLive?.key !== key) {
      this._pageLive = { key, entry: p.entry_id, i, at, since: Date.now(), sent: Date.now() };
      const live = this._pageLive;
      this._hass.callWS({ type: "panelos/page/command", entry_id: p.entry_id, key: "page", value: at })
        .then(() => { live.shownAt = Date.now(); this._schedulePicture(400); }, () => { this._pageLive = { key, entry: p.entry_id, failed: true }; });
    }
    return !this._pageLive.failed;
  }

  // The panel's picture shows the page whose settings are open: it was taken after the panel went to that page
  // (until then the editors draw the page's own background, not the page the panel was on before).
  _picOfLivePage(p) {
    const pic = this._pics[p.entry_id];
    const live = this._pageLive;
    return Boolean(pic && live && live.entry === p.entry_id && !live.failed && live.shownAt && pic.asked >= live.shownAt);
  }

  // The panel shows its screensaver now (while its settings are being changed); it comes back when
  // the Display page is left.
  _showSaver(p) {
    if (!p?.connected) return;
    this._lookChangedAt = Date.now();
    this._saverShown = p.entry_id;
    if (p.states?.screen !== false) this._command(p, "screen", false);
  }

  async _command(p, key, value) {
    // shown at once; an update sent before the panel took it does not flip it back (for 6 s at most)
    const fly = (this._cmdFly = this._cmdFly || {});
    (fly[p.entry_id] = fly[p.entry_id] || {})[key] = { value, until: Date.now() + 6000 };
    if (p.states) p.states[key] = value;
    this._render();
    try {
      await this._hass.callWS({ type: "panelos/page/command", entry_id: p.entry_id, key, value });
      this._schedulePicture(PICTURE_AFTER_CHANGE_MS);
      return true;
    } catch (err) {
      this._toast(err.message || String(err), null, true);
      return false;
    }
  }

  async _test(p, kind, item) {
    try {
      await this._hass.callWS({ type: "panelos/page/test", entry_id: p.entry_id, kind, item, seconds: 10 });
      this._toast(`Showing on ${this._pname(p)} for 10 seconds`);
      this._schedulePicture(900);
      this._previewSheetIfNarrow(p);
    } catch (err) {
      this._toast(err.message || String(err), null, true);
    }
  }

  // -- the panel's screen -----------------------------------------------------------

  _schedulePicture(delay) {
    if (!this.isConnected || document.visibilityState !== "visible" || !this._hass) return;
    // a minute without use: the preview stops on its last picture until the page is used again (`_onUse`)
    if (Date.now() - this._use.at > PREVIEW_IDLE_MS) {
      if (!this._use.paused) {
        this._use.paused = true;
        this._requestRender();
      }
      return;
    }
    const due = Date.now() + delay;
    if (this._pic.timer && this._pic.due <= due) return;
    clearTimeout(this._pic.timer);
    this._pic.due = due;
    this._pic.timer = setTimeout(() => {
      this._pic.timer = null;
      this._fetchPictures();
    }, delay);
  }

  async _fetchPictures() {
    if (this._pic.busy) return;
    const all = this._route().view === "all";
    const p = this._cur();
    const targets = all ? this._panels.filter((x) => x.connected) : p && p.connected ? [p] : [];
    if (!targets.length || (all && Date.now() - this._pic.allAt < ALL_PICTURES_EVERY_MS - 100)) {
      this._schedulePicture(all ? ALL_PICTURES_EVERY_MS - (Date.now() - this._pic.allAt) : PICTURE_IDLE_MS);
      return;
    }
    this._pic.busy = true;
    try {
      for (const t of targets) await this._fetchPicture(t);
      if (all) this._pic.allAt = Date.now();
    } finally {
      this._pic.busy = false;
      this._schedulePicture(all ? ALL_PICTURES_EVERY_MS : PICTURE_EVERY_MS);
    }
  }

  async _fetchPicture(p) {
    try {
      const asked = Date.now();
      const result = await this._hass.callWS({ type: "panelos/page/screen", entry_id: p.entry_id });
      const bytes = Uint8Array.from(atob(result.png), (c) => c.charCodeAt(0));
      const url = URL.createObjectURL(new Blob([bytes], { type: result.type || "image/png" }));
      const img = new Image();
      await new Promise((resolve) => {
        img.onload = img.onerror = resolve;
        img.src = url;
      });
      const old = this._pics[p.entry_id];
      const shape = old && (old.w !== img.naturalWidth || old.h !== img.naturalHeight);
      const wasLive = this._picOfLivePage(p);
      this._pics[p.entry_id] = { url, w: img.naturalWidth || 1, h: img.naturalHeight || 1, asked };
      // the first picture of the page whose settings are open replaces the drawn background
      const nowLive = !wasLive && this._picOfLivePage(p);
      this._captureLook(p, img);
      this._keepPageLive(p);
      if (old) setTimeout(() => URL.revokeObjectURL(old.url), 2000);
      if (!old || shape || nowLive) this._requestRender();
      else this._showPictures(p.entry_id);
    } catch (_err) {
      // keeps the last picture; the next try comes on schedule
    }
  }

  // The panel's screen as the picture of the look it shows: the open page's look, or the screensaver's clock face
  // while the screensaver is shown for its settings. Not right after a change (the panel needs a moment) and at most
  // every 20 s per look.
  _captureLook(p, img) {
    const now = Date.now();
    // the change has to be on the panel first: saved, sent, applied and faded in (a weather crossfade takes 3 s,
    // and a picture can be a second old), so nothing is kept until 4.5 s after the last change was confirmed
    const sync = this._sync[p.entry_id]?.state;
    if (sync === "busy" || sync === "wait") { this._lookChangedAt = Math.max(this._lookChangedAt, now); return; }
    if (now - this._lookChangedAt < 4500 || !img.naturalWidth) return;
    // dark or light only when it is clear which one the panel shows (Auto switches below 10 / above 30 lx)
    const theme = String(p?.states?.theme || "").toLowerCase();
    const lx = Number(p?.states?.illuminance);
    if (theme !== "dark" && theme !== "light" && !(Number.isFinite(lx) && (lx < 8 || lx > 35))) return;
    let name = null;
    if (this._saverShown === p.entry_id) {
      if (p.states?.screen !== false) return;
      if (!/clock/i.test(String(p.states?.screensaver_style || "clock"))) return;
      const saver = { ...DEFAULT_SCREENSAVER, ...(this._settings(p).screensaver || {}) };
      name = `saver-${Number.isInteger(saver.face) ? saver.face : 1}`;
    } else {
      const live = this._pageLive;
      if (!live || live.failed || live.entry !== p.entry_id || live.i == null || now - (live.since || 0) < 2500) return;
      if (p.states?.screen === false) return;
      const page = this._pages(p)[live.i];
      if (!page || !this._liveWorthy(p, page)) return;
      name = lookName(page.kind, page.variant, page);
    }
    const key = `${p.entry_id}|${name}|${this._lookMode(p)}`;
    const old = this._liveLooks[key];
    if (old && now - old.t < 6000) return;
    try {
      const c = document.createElement("canvas");
      c.width = c.height = 224;
      const g = c.getContext("2d");
      g.imageSmoothingQuality = "high";
      g.drawImage(img, 0, 0, 224, 224);
      const u = c.toDataURL("image/webp", 0.82);
      this._liveLooks[key] = { u, t: now };
      saveLiveLooks(this._liveLooks);
      this.shadowRoot.querySelectorAll(`img[data-look="${CSS.escape(key)}"]`).forEach((el) => { el.src = u; });
    } catch (_err) {
      // a picture that cannot be read back: the demo picture stays
    }
  }

  // While a page's settings are open the panel keeps showing that page: asked again every 15 s (its screensaver
  // would come after its own timeout, and a page swiped away on the panel comes back).
  _keepPageLive(p) {
    const live = this._pageLive;
    if (!live || live.failed || live.entry !== p.entry_id || Date.now() - live.sent < 15000) return;
    live.sent = Date.now();
    this._hass.callWS({ type: "panelos/page/command", entry_id: p.entry_id, key: "page", value: live.at }).catch(() => {});
  }

  // Whether the panel's screen shows a look well enough to stand for it: not the timers page (without a running
  // timer it shows its start buttons), the music page only with a track, the notifications only when there are some.
  _liveWorthy(p, page) {
    if (page.kind === "timers") return false;
    if (page.kind === "music") {
      const id = page.options?.player || this._settings(p).media_player_entity;
      const st = id && this._hass?.states?.[id];
      return Boolean(st && ["playing", "paused"].includes(st.state) && st.attributes?.media_title);
    }
    if (page.kind === "info") return this._noteList().length > 0;
    return true;
  }

  // Dark or light pictures for this panel: its theme, or with Auto the light in the room (as the panel switches).
  _lookMode(p) {
    const t = String(p?.states?.theme || "").toLowerCase();
    if (t === "dark" || t === "light") return t;
    const lx = Number(p?.states?.illuminance);
    return Number.isFinite(lx) && lx > 20 ? "light" : "dark";
  }

  // A look's picture: the panel's own screen when it showed that look, else the picture taken with demo content,
  // else the drawing.
  _lookPic(p, name, fallback, { fit = "cover" } = {}) {
    const mode = this._lookMode(p);
    const key = `${p?.entry_id}|${name}|${mode}`;
    const live = name.startsWith("timers-") ? null : this._liveLooks[key];
    if (!live && MISSING_LOOKS.has(`${name}-${mode}`) && fallback) return fallback();
    return h("img", {
      class: `lookimg${fit === "contain" ? " contain" : ""}`, alt: "", draggable: "false", "data-look": key, loading: "lazy",
      src: live ? live.u : `${this._static}/looks/${name}-${mode}.webp`,
      onerror: (e) => { MISSING_LOOKS.add(`${name}-${mode}`); if (fallback) e.currentTarget.replaceWith(fallback()); },
    });
  }

  _showPictures(id) {
    const pic = this._pics[id];
    if (!pic) return;
    this.shadowRoot.querySelectorAll(`img[data-pic="${CSS.escape(id)}"]`).forEach((img) => {
      if (img.src !== pic.url) img.src = pic.url;
    });
  }

  // The live picture of a panel (an img), or a placeholder.
  _picture(p, { placeholder = true } = {}) {
    const pic = this._pics[p.entry_id];
    if (p.connected && pic) return h("img", { src: pic.url, alt: `What ${this._pname(p)} shows now`, "data-pic": p.entry_id, draggable: "false" });
    if (!placeholder) return icon(p.connected ? "monitor" : "monitor-off");
    return h("div", { class: "off" }, p.connected ? "Waiting for a picture…" : "Offline");
  }

  // -- live values -------------------------------------------------------------------

  _liveSoon() {
    if (this._liveTimer) return;
    this._liveTimer = setTimeout(() => {
      this._liveTimer = null;
      this._updateLive();
    }, 1000);
  }

  _updateLive() {
    this.shadowRoot.querySelectorAll("[data-live]").forEach((e) => {
      const v = this._liveValue(e.dataset.live);
      if (v != null && e.textContent !== v) e.textContent = v;
    });
  }

  // "ha:<entity>" = an entity's state with its unit; "lux:<entry>" = the screensaver's light reading.
  _liveValue(key) {
    const [kind, id] = [key.slice(0, key.indexOf(":")), key.slice(key.indexOf(":") + 1)];
    if (kind === "ha") {
      const st = this._hass?.states?.[id];
      if (!st) return "–";
      const unit = st.attributes?.unit_of_measurement;
      return unit ? `${st.state} ${unit}` : st.state;
    }
    if (kind === "lux") {
      const p = this._panelById(id);
      if (!p) return "";
      const s = p.settings.screensaver || DEFAULT_SCREENSAVER;
      let v = null;
      if (s.light_entity) {
        const st = this._hass?.states?.[s.light_entity];
        v = st ? Number(st.state) : null;
      } else if (typeof p.states?.illuminance === "number") v = p.states.illuminance;
      if (v == null || Number.isNaN(v)) return "No reading yet";
      return `Now ${Math.round(v)} lx · ${v < s.lux ? "dark enough" : "too bright"}`;
    }
    return null;
  }

  // -- rendering ---------------------------------------------------------------------

  // Typing (a text field with the focus), a press, a drag or an open menu: the page is not drawn again now.
  _interacting() {
    const a = this.shadowRoot.activeElement;
    const pressing = Boolean(this._press) && Date.now() - this._press < 30000;
    const typing = Boolean(a && (a.tagName === "TEXTAREA" || (a.tagName === "INPUT" && !["checkbox", "radio", "range"].includes(a.type))));
    return this._dragging || pressing || Boolean(this._menu) || typing;
  }

  // Every redraw goes through here: it waits while the user is interacting and runs right after (_flush).
  _render() {
    if (this._interacting()) {
      this._pending = true;
      return;
    }
    this._renderNow();
  }

  _requestRender() {
    this._render();
  }

  _flush() {
    if (this._pending && !this._interacting()) {
      this._pending = false;
      this._renderNow();
    }
  }

  // onExpire: runs when the toast goes away without Undo (a change that waits for its Undo first).
  _toast(text, undo = null, error = false, { onExpire = null } = {}) {
    let done = false;
    const t = h("div", { class: `toast${error ? " err" : ""}` }, icon(error ? "alert-circle-outline" : "check"), h("span", null, text),
      undo ? h("button", { onclick: () => { if (done) return; done = true; undo(); t.remove(); } }, "Undo") : null);
    this._toastBox.append(t);
    setTimeout(() => {
      t.remove();
      if (!done) { done = true; if (onExpire) onExpire(); }
    }, undo ? 6000 : error ? 6000 : 3200);
  }

  _renderNow() {
    const root = this.shadowRoot;
    if (!root) return;
    this._pending = false;
    if (!this._styleEl || this._styleStatic !== this._static) {
      this._styleEl = h("style", null, STYLE.replaceAll("{STATIC}", this._static));
      this._styleStatic = this._static;
    }
    const main0 = root.querySelector(".main");
    const scroll = main0 ? main0.scrollTop : 0;
    const sb = root.querySelector(".sheet-body");
    const sheetScroll = sb ? sb.scrollTop : 0;
    const lists = [...root.querySelectorAll(".looks")].map((e) => e.scrollLeft);
    this._closeMenu(true);
    const brandBg = this._brandBg();

    if (!this._data || !this._panels.length) {
      const menu = h("button", { class: "ib", "aria-label": "Home Assistant menu", onclick: () => this._toggleHaMenu() }, icon("menu"));
      this._mount(
        h("div", { class: "app no-preview" },
          h("nav", { class: "nav", "aria-label": "PanelOS" }, h("div", { class: "brand", style: brandBg }, h("span", { class: "brand-box" }, h("i", { role: "img", "aria-label": "PanelOS" })))),
          h("header", { class: "top" }, h("div", { class: "top-row" }, this.hasAttribute("ha-narrow") ? menu : null, h("i", { class: "logo", role: "img", "aria-label": "PanelOS" }))),
          h("main", { class: "main" }, h("div", { class: "blank" },
            !this._data ? "Loading…" : [h("span", null, "No panel is set up yet."), this._btn("Add a panel", () => this._startAdd(), { cls: "primary", icon: "plus" })]))));
      this._renderLayer();
      return;
    }

    const r = this._route();
    const all = r.view === "all";
    const p = this._cur();
    if (!all && r.panel && !this._panelById(r.panel)) {
      setTimeout(() => this._go(this._panelPath(p), true));
    }
    const secs = all ? [] : this._sections(p);
    const section = r.view === "page" ? "pages" : r.view;
    const goSec = (s) => this._go(this._panelPath(p, s.id));

    const nav = h("nav", { class: "nav", "aria-label": "PanelOS" },
      h("div", { class: "brand", style: brandBg }, h("span", { class: "brand-box" }, h("i", { role: "img", "aria-label": "PanelOS" }))),
      h("div", { class: "nav-scroll" },
        this._switcher(p, all),
        all ? null : h("div", { class: "links" }, secs.map((s) => h("button", { class: s.id === section ? "on" : "", onclick: () => goSec(s), "aria-current": s.id === section ? "page" : null },
          icon(s.icon), s.long || s.name,
          s.id === "pages" ? h("span", { class: "badge" }, this._pages(p).filter((x, i) => i === 0 || !x.hidden).length) : null)))),
      h("div", { class: "nav-foot links" },
        h("button", { class: all ? "on" : "", onclick: () => this._go("/all") }, icon("view-grid-outline"), "All panels", h("span", { class: "badge" }, this._panels.length)),
        h("button", { onclick: () => this._startAdd() }, icon("plus"), "Add a panel")));

    const top = h("header", { class: "top" },
      h("div", { class: "top-row" },
        this.hasAttribute("ha-narrow") ? h("button", { class: "ib", "aria-label": "Home Assistant menu", onclick: () => this._toggleHaMenu() }, icon("menu")) : null,
        this._switcher(p, all),
        h("button", { class: "ib", "aria-label": "More", "aria-haspopup": "menu", onclick: (e) => { e.stopPropagation(); this._moreMenu(e.currentTarget); } }, icon("dots-vertical"))),
      all ? null : h("div", { class: "tabs" }, secs.map((s) => h("button", { class: s.id === section ? "on" : "", onclick: () => goSec(s) }, icon(s.icon), s.name))));

    const content = h("div", { class: `content${all ? " wide" : ""}` }, this._view(p, r));
    const narrow = this.getAttribute("size") === "narrow";
    const pv = all || narrow ? null : h("aside", { class: "preview-col", "aria-label": "The panel's screen" }, this._previewBlock(p));
    let bnav = null;
    if (narrow) {
      const items = all ? [] : ["overview", "pages", "alerts", "display"].map((id) => secs.find((s) => s.id === id)).filter(Boolean);
      const inMore = !all && !items.some((s) => s.id === section);
      bnav = h("nav", { class: "bnav", "aria-label": "Sections" },
        all ? [h("button", { class: "on" }, icon("view-grid-outline"), "Panels"), h("button", { onclick: () => this._startAdd() }, icon("plus"), "Add")]
          : [...items.map((s) => h("button", { class: s.id === section ? "on" : "", onclick: () => goSec(s) }, icon(s.icon), s.name)),
            h("button", { class: inMore ? "on" : "", onclick: () => this._openSheet({ kind: "more" }) }, icon("dots-horizontal"), inMore ? (secs.find((s) => s.id === section) || {}).name || "More" : "More")]);
    }
    const fab = narrow && !all && ["page", "display", "voice", "alerts", "pages"].includes(r.view)
      ? h("button", { class: "fab", "aria-label": "Show the panel's screen", onclick: () => this._openSheet({ kind: "preview", panel: p.entry_id }) }, this._picture(p, { placeholder: false }))
      : null;

    const app = h("div", { class: `app${pv ? "" : " no-preview"}` }, nav, top, h("main", { class: "main" }, content), pv, bnav, fab);
    this._mount(app);
    // a list picker (entities, sounds) is left as it is by background updates: rebuilding it under the
    // finger made the list jump back to the top and lost taps; it redraws itself when its own content changes
    if (!this._ui.sheet || !STILL_SHEETS.has(this._ui.sheet.kind)) this._renderLayer();
    const main = root.querySelector(".main");
    if (main && scroll) main.scrollTop = scroll;
    const nsb = root.querySelector(".sheet-body");
    if (nsb && sheetScroll) nsb.scrollTop = sheetScroll;
    // once the sheet is drawn and the scroll is back: the bubble is placed by its button in the same frame
    if (this._ui.info || this._ui.allInfo) { this._placePops(); requestAnimationFrame(() => this._placePops()); }
    const sameView = this._looksPath === this._path;
    this._looksPath = this._path;
    root.querySelectorAll(".looks").forEach((e, i) => {
      if (sameView && lists[i] != null) e.scrollLeft = lists[i];
      else {
        const on = e.querySelector(".look.on");
        if (on && on.offsetLeft + on.offsetWidth > e.clientWidth) e.scrollLeft = on.offsetLeft - (e.clientWidth - on.offsetWidth) / 2;
      }
    });
    this._afterRender();
  }

  // The page drawn anew: only its main part is swapped. The sheet layer and the toasts stay where they
  // are (taken out and put back, an open list inside a sheet would jump back to its top).
  _mount(app) {
    const root = this.shadowRoot;
    const old = [...root.children].find((n) => n.classList.contains("app"));
    if (old && [this._styleEl, this._layer, this._toastBox].every((n) => n.parentNode === root)) old.replaceWith(app);
    else root.replaceChildren(this._styleEl, app, this._layer, this._toastBox);
  }

  // the header's pattern texture (the brand kit's picture, filling the header)
  _brandBg() {
    return { backgroundImage: `url("${this._static}/pattern-${this.hasAttribute("dark") ? "dark" : "light"}.svg")` };
  }

  // Info bubbles: next to their button in window coordinates, whole inside the window (below the button,
  // or above when there is no room; scrolling inside only when taller than the window).
  _placePops() {
    const root = this.shadowRoot;
    if (!root) return;
    root.querySelectorAll(".pop").forEach((p) => {
      const b = p.previousElementSibling && p.previousElementSibling.classList.contains("info") ? p.previousElementSibling : null;
      if (!b) return;
      const br = b.getBoundingClientRect();
      const vw = Math.min(window.innerWidth, document.documentElement.clientWidth || window.innerWidth);
      const vh = window.innerHeight;
      const m = 12;
      const w = Math.min(360, vw - 2 * m);
      Object.assign(p.style, { position: "fixed", width: `${w}px`, right: "auto", transform: "none", maxHeight: "none", overflowY: "visible" });
      const left = Math.min(Math.max(m, br.left - 14), vw - m - w);
      p.style.left = `${left}px`;
      p.style.top = "0px";
      const ph = p.getBoundingClientRect().height;
      let top = br.bottom + 8;
      if (top + ph > vh - m) {
        if (br.top - 8 - ph >= m) top = br.top - 8 - ph;
        else {
          top = Math.max(m, Math.min(top, vh - m - ph));
          if (ph > vh - 2 * m) { top = m; p.style.maxHeight = `${vh - 2 * m}px`; p.style.overflowY = "auto"; }
        }
      }
      p.style.top = `${top}px`;
      // a transformed ancestor (a sheet sliding in) makes "fixed" relative to it: correct by what was measured
      const r = p.getBoundingClientRect();
      if (Math.abs(r.left - left) > 0.5 || Math.abs(r.top - top) > 0.5) {
        p.style.left = `${2 * left - r.left}px`;
        p.style.top = `${2 * top - r.top}px`;
      }
    });
  }

  _toggleHaMenu() {
    this.dispatchEvent(new Event("hass-toggle-menu", { bubbles: true, composed: true }));
  }

  _afterRender() {
    // keep the explanations inside the window
    this.shadowRoot.querySelectorAll(".pop").forEach((pop) => {
      const r = pop.getBoundingClientRect();
      let dx = 0;
      if (r.right > window.innerWidth - 12) dx = window.innerWidth - 12 - r.right;
      if (r.left + dx < 12) dx = 12 - r.left;
      if (dx) pop.style.transform = `translateX(${dx}px)`;
    });
  }

  _view(p, r) {
    if (r.view === "all") return this._viewAll();
    if (r.view === "page") return this._viewPage(p, r.page);
    const fn = { overview: this._viewOverview, pages: this._viewPages, alerts: this._viewAlerts, voice: this._viewVoice, display: this._viewDisplay, device: this._viewDevice }[r.view];
    if (!fn || !this._sections(p).some((s) => s.id === r.view)) return this._viewOverview(p);
    return fn.call(this, p);
  }

  _switcher(p, all) {
    const b = h("button", { class: "switcher", "aria-haspopup": "menu", "aria-label": "Choose a panel", onclick: (e) => { e.stopPropagation(); this._panelPicker(b); } },
      all ? h("span", { class: "thumb" }, icon("view-grid-outline")) : h("span", { class: "thumb" }, this._picture(p, { placeholder: false })),
      h("span", { class: "txt" },
        h("b", null, all ? "All panels" : this._pname(p)),
        h("small", null, all ? plural(this._panels.length, "panel", "panels") : [this._pmodel(p), this._parea(p)].filter(Boolean).join(" · "))),
      icon("chevron-down"));
    return b;
  }

  _panelPicker(anchor) {
    if (this.getAttribute("size") === "narrow") {
      this._openSheet({ kind: "panels" });
      return;
    }
    const r = this._route();
    this._openMenu(anchor, [
      ...this._panels.map((x) => ({
        icon: x.connected ? "monitor-dashboard" : "monitor-off",
        label: this._pname(x),
        run: () => this._go(this._panelPath(x, r.view !== "all" && r.view !== "page" && this._sections(x).some((s) => s.id === r.view) ? r.view : "")),
      })),
      "-",
      { icon: "view-grid-outline", label: "All panels", run: () => this._go("/all") },
      { icon: "plus", label: "Add a panel", run: () => this._startAdd() },
    ]);
  }

  _moreMenu(anchor) {
    const p = this._cur();
    const all = this._route().view === "all";
    this._openMenu(anchor, [
      ...(all ? [] : [
        { icon: "pencil-outline", label: `Rename ${this._pname(p)}`, run: () => this._openSheet({ kind: "rename", panel: p.entry_id }), disabled: !p.device_id },
        { icon: "content-copy", label: "Copy settings to other panels…", run: () => this._openCopy(p), disabled: this._panels.length < 2 },
        { icon: "restart", label: "Restart the app on the panel", run: () => this._command(p, "restart_app", null).then((ok) => ok && this._toast(`Restarting the app on ${this._pname(p)}…`)), disabled: !p.connected },
        "-"]),
      { icon: "view-grid-outline", label: "All panels", run: () => this._go("/all") },
      { icon: "plus", label: "Add a panel", run: () => this._startAdd() },
    ]);
  }

  // -- controls ----------------------------------------------------------------------

  // (the jelly of the switch, the segmented control, the weekday row and the checkbox: see _jDown and the ones after)
  _toggle(checked, onchange, { label = "On", disabled = false } = {}) {
    return h("label", { class: "switch", "data-jelly": "sw", "data-jl": label }, h("input", { type: "checkbox", role: "switch", checked, disabled, "aria-label": label, onchange: (e) => onchange(e.target.checked) }), h("span", null, h("i")));
  }

  _seg(options, value, onchange, { label = null } = {}) {
    return h("div", { class: "seg", role: "radiogroup", "aria-label": label, "data-jelly": "seg", "data-jl": label || options.map((o) => o[1]).join("|") },
      h("span", { class: "pill", "aria-hidden": "true" }),
      options.map(([v, text, dis]) => h("button", {
        class: v === value ? "on" : "", role: "radio", "aria-checked": v === value ? "true" : "false",
        disabled: Boolean(dis), title: typeof dis === "string" ? dis : null, onclick: () => onchange(v),
      }, text)));
  }

  _select(options, value, onchange, { label = "Choose", full = false, disabled = false } = {}) {
    return this._dropdown(options, value, onchange, { label, full, disabled });
  }

  // A dropdown (spec 28.5, 28.6): a field with the chosen name (several: as many as fit, then "+N") and a chevron; the
  // menu opens under it (above when there is no room), the chosen row ticked. Multi: a checkbox per row, it stays open,
  // "All" / "None" from 5 options, "Done" on touch. Over 12 options a search field. Keys: ↓ / Enter / Space open,
  // ↑ / ↓ move, Enter picks, Esc closes, a letter jumps to the next match.
  //   single: options [[value, label, disabled?]], value, onchange(value)
  //   multi:  items [{ on, label, onclick }] (each row toggles itself)
  _dd({ label = "Choose", key = null, full = false, disabled = false, multi = false }, rows, current, onpick) {
    const id = key || `dd:${label}:${rows.length}`;
    const open = this._ui.dd === id && !disabled;
    const chosen = multi ? rows.filter((r) => r.on) : rows.filter((r) => r.value === current);
    const q = open && rows.length > 12 ? (this._ui.ddq || "").toLowerCase() : "";
    const shown = q ? rows.filter((r) => String(r.label).toLowerCase().includes(q)) : rows;
    const close = () => { this._ui.dd = null; this._ui.ddq = ""; this._render(); };
    // (the state at the click, not at the render: a click may come before the next render)
    const toggle = (e) => { e.stopPropagation(); const was = this._ui.dd === id; this._ui.dd = was ? null : id; this._ui.ddq = ""; this._render(); if (!was) requestAnimationFrame(() => this._ddPlace(id)); };
    const pick = (r) => {
      if (r.disabled) return;
      if (multi) { r.onclick(); return; }
      this._ui.dd = null; this._ui.ddq = "";
      onpick(r.value);
    };
    const keys = (e) => {
      const list = [...(e.currentTarget.closest(".ddw")?.querySelectorAll(".ddr") || [])];
      const at = list.indexOf(document.activeElement);
      if (e.key === "Escape") { e.preventDefault(); close(); return; }
      const isOpen = this._ui.dd === id;
      if (!isOpen && (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ")) { e.preventDefault(); toggle(e); requestAnimationFrame(() => this._ddFocus(id)); return; }
      if (!isOpen) return;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); list[Math.max(0, Math.min(list.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)))]?.focus(); return; }
      if (e.key.length === 1 && /\S/.test(e.key) && !e.target.matches("input")) {
        const k = e.key.toLowerCase();
        const next = list.slice(at + 1).concat(list.slice(0, at + 1)).find((b) => b.textContent.trim().toLowerCase().startsWith(k));
        if (next) next.focus();
      }
    };
    const names = chosen.map((r) => r.label);
    const field = h("button", { class: `dd${open ? " open" : ""}${full ? " full" : ""}`, type: "button", disabled, "aria-haspopup": "listbox", "aria-expanded": open ? "true" : "false", "aria-label": label, onclick: toggle, onkeydown: keys },
      h("span", { class: `v${names.length ? "" : " none"}` }, names.length ? (multi ? names.slice(0, 2) : names).join(", ") : (multi ? "None" : "Choose")),
      multi && names.length > 2 ? h("span", { class: "more" }, `+${names.length - 2}`) : null,
      icon(open ? "chevron-up" : "chevron-down"));
    const menu = open ? h("div", { class: "ddm", role: "listbox", "aria-multiselectable": multi ? "true" : null, onclick: (e) => e.stopPropagation(), onkeydown: keys },
      rows.length > 12 ? h("input", { class: "inp dds", value: this._ui.ddq || "", placeholder: "Search", "aria-label": "Search", oninput: (e) => { this._ui.ddq = e.target.value; this._renderNow(); requestAnimationFrame(() => { const f = this.shadowRoot?.querySelector(`.ddw[data-dd="${CSS.escape(id)}"] .dds`); if (f) { f.focus(); f.setSelectionRange(f.value.length, f.value.length); } }); } }) : null,
      multi && rows.length >= 5 ? h("div", { class: "ddbar" },
        this._btn("All", () => rows.filter((r) => !r.on && !r.disabled).forEach((r) => r.onclick()), { cls: "small ghost" }),
        this._btn("None", () => rows.filter((r) => r.on).forEach((r) => r.onclick()), { cls: "small ghost" })) : null,
      h("div", { class: "ddl" }, shown.map((r) => h("button", { class: `ddr${(multi ? r.on : r.value === current) ? " on" : ""}`, type: "button", role: "option", disabled: Boolean(r.disabled),
        "aria-selected": (multi ? r.on : r.value === current) ? "true" : "false", onclick: () => pick(r) },
        multi ? this._ckBox(r.on, String(r.label)) : (r.value === current ? icon("check") : h("span", { class: "sp" })),
        h("span", { class: "t" }, r.label)))),
      multi ? h("div", { class: "dddone" }, this._btn("Done", close, { cls: "primary" })) : null) : null;
    return h("div", { class: `ddw${full ? " full" : ""}`, "data-dd": id }, field, menu);
  }

  // the open menu above its field when there is no room below
  _ddPlace(id) {
    const w = this.shadowRoot?.querySelector(`.ddw[data-dd="${CSS.escape(id)}"]`);
    const m = w?.querySelector(".ddm");
    if (!w || !m) return;
    const r = w.getBoundingClientRect();
    const below = window.innerHeight - r.bottom, above = r.top;
    m.classList.toggle("up", m.offsetHeight + 12 > below && above > below);
    m.querySelector(".ddr.on")?.scrollIntoView({ block: "nearest" });
  }

  _ddFocus(id) {
    const w = this.shadowRoot?.querySelector(`.ddw[data-dd="${CSS.escape(id)}"]`);
    (w?.querySelector(".ddr.on") || w?.querySelector(".ddr"))?.focus();
  }

  // A single choice: segmented when it fits (up to 3 options, or 4 with labels of up to 10 characters), else a dropdown
  _choice(options, value, onchange, { label = "Choose", full = false } = {}) {
    const list = options.map((o) => (Array.isArray(o) ? o : [o, o]));
    const fits = list.length <= 3 ? list.every(([, l]) => String(l).length <= 16) : list.length === 4 && list.every(([, l]) => String(l).length <= 10);
    if (fits) return this._seg(list, value, onchange, { label });
    return this._dropdown(list, value, onchange, { label, full });
  }

  _dropdown(options, value, onchange, { label = "Choose", full = false, disabled = false, key = null } = {}) {
    const list = options.map((o) => (Array.isArray(o) ? o : [o, o]));
    if (value != null && value !== "" && !list.some(([v]) => v === value)) list.push([value, value]);
    return this._dd({ label, full, disabled, key }, list.map(([v, l, dis]) => ({ value: v, label: l, disabled: Boolean(dis) })), value, onchange);
  }

  // several choices: rows of { on, label, onclick } (see _mi)
  _multi({ label = "Choose", key = null, full = true } = {}, items) {
    return h("div", { class: "below" }, this._dd({ label, key, full, multi: true }, items.filter(Boolean), null, null));
  }

  _mi(on, label, onclick) {
    return { on: Boolean(on), label, onclick };
  }

  // -- jelly motion (spec 28.15) -------------------------------------------------------
  // A control's motion survives the page being drawn again: it is kept by the control's place (its kind, its label
  // and its position among the controls of that kind and label, in the page or in the sheet), and each frame draws it
  // on whichever element stands there now.

  _jScope(el) {
    return this._layer.contains(el) ? this._layer : this.shadowRoot.querySelector(".app");
  }

  _jSame(scope, kind, label) {
    return [...scope.querySelectorAll(`[data-jelly="${kind}"]`)].filter((x) => (x.dataset.jl || "") === label);
  }

  _jKey(el) {
    const scope = el && this._jScope(el);
    if (!scope) return null;
    const kind = el.dataset.jelly;
    const label = el.dataset.jl || "";
    const n = this._jSame(scope, kind, label).indexOf(el);
    return n < 0 ? null : JSON.stringify([scope === this._layer ? "L" : "A", kind, label, n]);
  }

  _jFind(key) {
    const [sc, kind, label, n] = JSON.parse(key);
    const scope = sc === "L" ? this._layer : this.shadowRoot.querySelector(".app");
    return scope ? this._jSame(scope, kind, label)[n] || null : null;
  }

  // One motion per control part. frame(el, t, done) draws it at t ms since the start and returns its shape (kept for a
  // motion that starts during this one: it goes on from the shape, without a jump) or null (back at rest).
  _jRun(el, part, dur, frame) {
    const key = this._jKey(el);
    if (!key) return;
    const id = `${key}#${part}`;
    const old = this._jRuns.get(id);
    if (old) cancelAnimationFrame(old.raf);
    const rec = { el, t0: performance.now(), raf: 0 };
    this._jRuns.set(id, rec);
    const step = (now) => {
      if (this._jRuns.get(id) !== rec) return;
      if (!rec.el.isConnected) rec.el = this._jFind(key) || rec.el;
      const t = Math.max(0, now - rec.t0);
      const done = t >= dur;
      if (rec.el.isConnected) {
        const shape = frame(rec.el, done ? dur : t, done);
        if (shape === null) this._jShapes.delete(id);
        else if (shape !== undefined) this._jShapes.set(id, shape);
      }
      if (done) this._jRuns.delete(id);
      else rec.raf = requestAnimationFrame(step);
    };
    rec.raf = requestAnimationFrame(step);
  }

  _jShape(el, part) {
    const key = this._jKey(el);
    return key ? this._jShapes.get(`${key}#${part}`) || null : null;
  }

  // A press holds its squashed shape until the change takes over or the press ends without one (then it springs back).
  _jHold(el, relax) {
    this._jHeld = { el, relax };
  }

  _jPointerDown(ev) {
    if (ev.button > 0) return;
    const t = ev.target;
    const sw = t.closest?.(".switch[data-jelly]");
    if (sw) {
      const input = sw.querySelector("input");
      if (input.disabled) return;
      this._jPress = { sw, sx: ev.clientX, flipped: false };
      this._jSwPress(sw);
      return;
    }
    const day = t.closest?.(".wdays[data-jelly] > .wd");
    if (day) { this._jDayPress(day); return; }
    const opt = t.closest?.(".opt, .ddr");
    const box = opt && !opt.disabled && opt.querySelector(".jck[data-jelly]");
    if (box && !opt.querySelector("input:disabled")) this._jBoxPress(box);
  }

  // a sideways drag of 30 % of the switch's width flips it before the release
  _jPointerMove(ev) {
    const pr = this._jPress;
    if (!pr || pr.flipped || !pr.sw.isConnected) return;
    const input = pr.sw.querySelector("input");
    const d = ev.clientX - pr.sx;
    const w = pr.sw.offsetWidth || 48;
    if ((d > w * 0.3 && !input.checked) || (d < -w * 0.3 && input.checked)) {
      pr.flipped = true;
      input.checked = !input.checked;
      input.dispatchEvent(new Event("change", { bubbles: true }));
      this._jNoClick = { sw: pr.sw, until: Date.now() + 800 };
    }
  }

  _jPointerUp() {
    this._jPress = null;
    const held = this._jHeld;
    if (!held) return;
    // the click (and its change) comes right after the release; without one the press springs back
    setTimeout(() => { if (this._jHeld === held) { this._jHeld = null; held.relax(); } }, 150);
  }

  _jOnClick(ev) {
    const t = ev.target;
    // the click after a drag that flipped the switch already: it does not flip it back
    const nc = this._jNoClick;
    if (nc) {
      this._jNoClick = null;
      if (Date.now() < nc.until && nc.sw.contains(t)) { ev.preventDefault(); return; }
    }
    const b = t.closest?.(".seg[data-jelly] > button");
    if (b && !b.disabled && !b.classList.contains("on")) { this._jSegGo(b.parentElement, b); return; }
    const day = t.closest?.(".wdays[data-jelly] > .wd");
    if (day) { this._jDayToggle(day); return; }
    const row = t.closest?.(".ddr");
    const box = row && !row.disabled && row.querySelector(".jck[data-jelly]");
    if (box) this._jBoxTick(box, !box.classList.contains("on"));
  }

  _jOnChange(ev) {
    const input = ev.target;
    if (!(input instanceof HTMLInputElement)) return;
    const sw = input.closest(".switch[data-jelly]");
    if (sw) { this._jSwFlip(sw); return; }
    const opt = input.closest(".opt");
    const box = opt && opt.querySelector(".jck[data-jelly]");
    if (!box) return;
    this._jBoxTick(box, input.checked);
    // a radio: the one that was chosen in the same group lets go
    if (input.type === "radio" && input.name) {
      this._jScope(input)?.querySelectorAll(`.opt input[type=radio][name="${CSS.escape(input.name)}"]`).forEach((o) => {
        const ob = o !== input && o.closest(".opt")?.querySelector(".jck.on");
        if (ob) this._jBoxTick(ob, false);
      });
    }
  }

  // ---- switch (28.3): the knob flattens 7 px toward its travel on a press; on a flip its leading edge races to the
  // far end (J-edge 90/240 ms) and presses flat against the track's inner wall, the trailing edge follows 30 ms later
  // (130/340 ms); the height keeps the area (kn² / w, 0.7 kn … the track's height − 4 px), the width at least 0.72 kn;
  // the track crossfades over 160 ms. Sizes come from the design values (--swW, --swH, --kn), never from the layout box.
  _jSwGeo(sw) {
    const cs = getComputedStyle(sw);
    const kn = parseFloat(cs.getPropertyValue("--kn")) || 20;
    const W = parseFloat(cs.getPropertyValue("--swW")) || 48;
    const H = parseFloat(cs.getPropertyValue("--swH")) || 28;
    return { kn, W, H, minL: 4, maxR: W - 4 };
  }

  _jSwRest(g, on) {
    return on ? { L: g.maxR - g.kn, R: g.maxR } : { L: g.minL, R: g.minL + g.kn };
  }

  _jSwPut(sw, g, L, R, k) {
    const knob = sw.querySelector("span > i");
    const track = sw.querySelector("span");
    if (!knob || !track) return;
    const w = Math.max(1, R - L);
    const hh = Math.min(g.H - 4, Math.max(g.kn * 0.7, (g.kn * g.kn) / w));
    knob.style.left = `${L}px`;
    knob.style.width = `${w}px`;
    knob.style.height = `${hh}px`;
    knob.style.top = `${(g.H - hh) / 2}px`;
    if (k != null) track.style.setProperty("--on", k.toFixed(3));
  }

  _jSwClear(sw) {
    const knob = sw.querySelector("span > i");
    const track = sw.querySelector("span");
    if (knob) knob.style.cssText = "";
    if (track) track.style.removeProperty("--on");
    return null;
  }

  _jSwPress(sw) {
    if (jReduce()) return;
    const on = sw.querySelector("input").checked;
    const g = this._jSwGeo(sw);
    const r = this._jSwRest(g, on);
    const s0 = this._jShape(sw, "m") || r;
    this._jRun(sw, "m", 90, (el, t) => {
      const u = jEo(t / 90);
      const L = on ? s0.L + (r.L - 7 - s0.L) * u : s0.L + (r.L - s0.L) * u;
      const R = on ? s0.R + (r.R - s0.R) * u : s0.R + (r.R + 7 - s0.R) * u;
      this._jSwPut(el, g, L, R, null);
      return { L, R, k: on ? 1 : 0 };
    });
    this._jHold(sw, () => this._jSwRelax(sw));
  }

  _jSwFlip(sw) {
    if (this._jHeld?.el === sw) this._jHeld = null;
    const to = sw.querySelector("input").checked;
    const g = this._jSwGeo(sw);
    const e = this._jSwRest(g, to);
    const s0 = this._jShape(sw, "m") || { ...this._jSwRest(g, !to), k: to ? 0 : 1 };
    const k0 = s0.k ?? (to ? 0 : 1);
    const k1 = to ? 1 : 0;
    const kAt = (t) => k0 + (k1 - k0) * Math.min(1, t / 160);
    if (jReduce()) {
      this._jRun(sw, "m", 150, (el, t, done) => {
        if (done) return this._jSwClear(el);
        const u = jEo(t / 150);
        const L = s0.L + (e.L - s0.L) * u;
        const R = s0.R + (e.R - s0.R) * u;
        this._jSwPut(el, g, L, R, kAt(t));
        return { L, R, k: kAt(t) };
      });
      return;
    }
    this._jRun(sw, "m", 700, (el, t, done) => {
      if (done) return this._jSwClear(el);
      let L;
      let R;
      if (to) {
        R = Math.min(g.maxR, s0.R + (e.R - s0.R) * jEdge(t, ...J_LEAD));
        L = s0.L + (e.L - s0.L) * jEdge(t - J_LAG, ...J_TRAIL);
      } else {
        L = Math.max(g.minL, s0.L + (e.L - s0.L) * jEdge(t, ...J_LEAD));
        R = s0.R + (e.R - s0.R) * jEdge(t - J_LAG, ...J_TRAIL);
      }
      if (R - L < g.kn * 0.72) { if (to) L = R - g.kn * 0.72; else R = L + g.kn * 0.72; }
      this._jSwPut(el, g, L, R, kAt(t));
      return { L, R, k: kAt(t) };
    });
  }

  // let go without a flip: back to rest with a small wobble (J-edge 70/220)
  _jSwRelax(sw) {
    if (!sw.isConnected) return;
    const s0 = this._jShape(sw, "m");
    if (!s0) return;
    const g = this._jSwGeo(sw);
    const e = this._jSwRest(g, sw.querySelector("input").checked);
    this._jRun(sw, "m", jReduce() ? 150 : 500, (el, t, done) => {
      if (done) return this._jSwClear(el);
      const a = jReduce() ? jEo(t / 150) : jEdge(t, 70, 220);
      const L = s0.L + (e.L - s0.L) * a;
      const R = s0.R + (e.R - s0.R) * a;
      this._jSwPut(el, g, L, R, null);
      return { L, R, k: s0.k };
    });
  }

  // ---- segmented (28.4): one pill travels. Its leading edge races (J-edge 90/240) and may pass the target by at most
  // 0.28 × its height (soft cap), the trailing edge follows 30 ms later (130/340, cap 0.21 ×); the pill thins while
  // stretched and bulges when squashed (H·√(w₀/w), 0.8 H … H + 2·padding − 2), never under 0.6 w₀ wide; done in
  // 750 ms. The labels change colour over 160 ms, 70 ms after the tap. A choice on another row of a wrapped control
  // moves at once.
  _jSegBox(seg, b) {
    const r = seg.getBoundingClientRect();
    const x = b.getBoundingClientRect();
    return { l: x.left - r.left, r: x.right - r.left, t: x.top - r.top, h: x.height };
  }

  _jSegGo(seg, b) {
    const btns = [...seg.querySelectorAll(":scope > button")];
    const was = seg.querySelector(":scope > button.on");
    const from = btns.indexOf(was);
    if (from < 0) return;
    const e1 = this._jSegBox(seg, b);
    const e0 = this._jSegBox(seg, was);
    if (Math.abs(e1.t - e0.t) > 1 || e1.h < 1) return;
    const s0 = this._jShape(seg, "pill") || { L: e0.l, R: e0.r };
    const H = e1.h;
    const pad = parseFloat(getComputedStyle(seg).paddingTop) || 3;
    const W = seg.clientWidth;
    const cy = e1.t + H / 2;
    const l1 = e1.l;
    const r1 = e1.r;
    const w0 = r1 - l1;
    const right = l1 > s0.L;
    const cap = H * 0.28;
    const reduce = jReduce();
    const put = (el, L, R) => {
      const pill = el.querySelector(":scope > .pill");
      if (!pill) return;
      const w = Math.max(1, R - L);
      const hh = H * Math.min((H + 2 * pad - 2) / H, Math.max(0.8, Math.sqrt(w0 / w)));
      pill.style.left = `${L}px`;
      pill.style.width = `${w}px`;
      pill.style.height = `${hh}px`;
      pill.style.top = `${cy - hh / 2}px`;
    };
    this._jRun(seg, "pill", reduce ? 150 : 750, (el, t, done) => {
      if (done) {
        el.classList.remove("jm", "jpre");
        el.querySelectorAll(":scope > button.jwas").forEach((x) => x.classList.remove("jwas"));
        const pill = el.querySelector(":scope > .pill");
        if (pill) pill.style.cssText = "";
        return null;
      }
      el.classList.add("jm");
      el.classList.toggle("jpre", t < 70);
      el.querySelectorAll(":scope > button").forEach((x, i) => x.classList.toggle("jwas", i === from));
      let L;
      let R;
      if (reduce) {
        const u = jEo(t / 150);
        L = s0.L + (l1 - s0.L) * u;
        R = s0.R + (r1 - s0.R) * u;
      } else {
        const a = jEdge(t, ...J_LEAD);
        const c = jEdge(t - J_LAG, ...J_TRAIL);
        if (right) {
          R = r1 + jSoft(s0.R + (r1 - s0.R) * a - r1, cap);
          L = l1 + jSoft(s0.L + (l1 - s0.L) * c - l1, cap * 0.75);
        } else {
          L = l1 - jSoft(l1 - (s0.L + (l1 - s0.L) * a), cap);
          R = r1 - jSoft(r1 - (s0.R + (r1 - s0.R) * c), cap * 0.75);
        }
        L = Math.max(pad, L);
        R = Math.min(W - pad, R);
        if (R - L < w0 * 0.6) { if (right) L = R - w0 * 0.6; else R = L + w0 * 0.6; }
      }
      put(el, L, R);
      return { L, R };
    });
  }

  // ---- weekday row (28.10): a press squashes the day (1.08 × 0.92); choosing it pops the text-colour disc from 0.4
  // (J-edge 90/260) with a wobble (A 0.20), the day's ground wobbles (A 0.10) and its neighbours ripple (A 0.05, 40 ms
  // later); un-choosing swells the disc to 1.08 (60 ms), then drops it into the centre (120 ms) while the ground
  // wobbles (A 0.08, from 60 ms) and the neighbours ripple (A 0.04, from 100 ms). The letter turns when the disc
  // passes 0.6.
  _jDayPress(day) {
    if (jReduce()) return;
    const row = day.parentElement;
    const k = [...row.children].indexOf(day);
    this._jRun(row, `b${k}`, 90, (el, t) => {
      const u = jEo(t / 90);
      jScale(el.children[k]?.querySelector(".base"), 1 + 0.08 * u, 1 - 0.08 * u);
      return { sx: 1 + 0.08 * u };
    });
    this._jHold(row, () => {
      if (!row.isConnected) return;
      this._jRun(row, `b${k}`, 150, (el, t, done) => {
        const base = el.children[k]?.querySelector(".base");
        if (done) { if (base) base.style.transform = ""; return null; }
        const s = 1 + 0.08 * (1 - jEo(t / 150));
        jScale(base, s, 2 - s);
        return { sx: s };
      });
    });
  }

  _jDayToggle(day) {
    if (this._jHeld?.el === day.parentElement) this._jHeld = null;
    const row = day.parentElement;
    const n = row.children.length;
    const k = [...row.children].indexOf(day);
    const on = day.getAttribute("aria-pressed") !== "true";
    const base = (el, i) => el.children[i]?.querySelector(".base");
    const blob = (el) => el.children[k]?.querySelector(".blob");
    const letter = (el, lit, done) => {
      const d = el.children[k];
      if (!d) return;
      if (done) { d.classList.remove("jl", "lit"); return; }
      d.classList.add("jl");
      d.classList.toggle("lit", lit);
    };
    const wobble = (i, A, lag, dur) => this._jRun(row, `b${i}`, dur, (el, t, done) => {
      const b = base(el, i);
      if (done) { if (b) b.style.transform = ""; return null; }
      const w = jWob(t - lag, A);
      jScale(b, w, 1 / w);
      return { sx: w };
    });
    if (jReduce()) {
      this._jRun(row, `o${k}`, 150, (el, t, done) => {
        const bl = blob(el);
        if (done) { if (bl) bl.style.transform = ""; letter(el, on, true); return null; }
        const u = jEo(t / 150);
        const s = on ? u : 1 - u;
        jScale(bl, s, s);
        letter(el, s > 0.6, false);
        return { s };
      });
      return;
    }
    if (on) {
      this._jRun(row, `o${k}`, 750, (el, t, done) => {
        const bl = blob(el);
        if (done) { if (bl) bl.style.transform = ""; letter(el, true, true); return null; }
        const s = 0.4 + 0.6 * jEdge(t, 90, 260);
        const w = jWob(t, 0.2);
        jScale(bl, s * w, s / w);
        letter(el, s > 0.6, false);
        return { s };
      });
      wobble(k, 0.1, 0, 750);
      if (k > 0) wobble(k - 1, 0.05, 40, 800);
      if (k < n - 1) wobble(k + 1, 0.05, 40, 800);
    } else {
      this._jRun(row, `o${k}`, 180, (el, t, done) => {
        const bl = blob(el);
        if (done) { if (bl) bl.style.transform = ""; letter(el, false, true); return null; }
        const s = Math.max(0, t < 60 ? 1 + 0.08 * jEo(t / 60) : 1.08 * (1 - ((t - 60) / 120) ** 2));
        jScale(bl, s, s);
        letter(el, s > 0.6, false);
        return { s };
      });
      wobble(k, 0.08, 60, 800);
      if (k > 0) wobble(k - 1, 0.04, 100, 800);
      if (k < n - 1) wobble(k + 1, 0.04, 100, 800);
    }
  }

  // ---- checkbox (28.8): a press squashes it (1.12 × 0.88); ticking fills it at once, pops it from 0.6 (J-edge
  // 90/230) with an area-keeping wobble (A 0.22, 140 / 160 ms) while the tick draws (150 ms, from 60 ms); unticking
  // dips it (1 − 0.12·sin(π·min(1, t/120))) with a wobble (A 0.12) while the tick retracts (100 ms).
  _jBoxPress(box) {
    if (jReduce()) return;
    this._jRun(box, "j", 90, (el, t) => {
      const u = jEo(t / 90);
      jScale(el.querySelector(".bw"), 1 + 0.12 * u, 1 - 0.12 * u);
      return { sx: 1 + 0.12 * u };
    });
    this._jHold(box, () => {
      if (!box.isConnected) return;
      this._jRun(box, "j", 150, (el, t, done) => {
        const bw = el.querySelector(".bw");
        if (done) { if (bw) bw.style.transform = ""; return null; }
        const s = 1 + 0.12 * (1 - jEo(t / 150));
        jScale(bw, s, 2 - s);
        return { sx: s };
      });
    });
  }

  _jBoxTick(box, on) {
    if (this._jHeld?.el === box) this._jHeld = null;
    const reduce = jReduce();
    const end = (el) => {
      const bw = el.querySelector(".bw");
      const path = el.querySelector("path");
      if (bw) bw.style.transform = "";
      if (path) path.style.strokeDashoffset = "";
      return null;
    };
    this._jRun(box, "j", reduce ? 150 : on ? 700 : 600, (el, t, done) => {
      if (done) return end(el);
      // the fill follows the change at once (the page may not be drawn again yet)
      el.classList.toggle("on", on);
      const bw = el.querySelector(".bw");
      const path = el.querySelector("path");
      if (reduce) {
        if (bw) bw.style.transform = "";
        if (path) path.style.strokeDashoffset = String(on ? 1 - jEo(t / 150) : jEo(t / 150));
        return {};
      }
      let k;
      let w;
      if (on) {
        k = 0.6 + 0.4 * jEdge(t, 90, 230);
        w = jWob(t, 0.22, 140, 160);
        if (path) path.style.strokeDashoffset = String(1 - jEo((t - 60) / 150));
      } else {
        k = 1 - 0.12 * Math.sin(Math.PI * Math.min(1, t / 120));
        w = jWob(t, 0.12, 140, 160);
        if (path) path.style.strokeDashoffset = String(jEo(t / 100));
      }
      jScale(bw, k * w, k / w);
      return {};
    });
  }

  // A weekday row (spec 28.10): seven round toggles with two-letter names
  _weekdays(days, onchange) {
    return h("div", { class: "wdays", role: "group", "aria-label": "Days", "data-jelly": "wd", "data-jl": "Days" }, DAY_NAMES.map((n, k) => h("button", {
      class: `wd${days[k] ? " on" : ""}`, type: "button", "aria-pressed": days[k] ? "true" : "false", "aria-label": n, onclick: () => onchange(k) },
      h("b", { class: "base" }), h("i", { class: "blob" }), h("span", null, n.slice(0, 2)))));
  }

  // The checkbox of spec 28.8 (a box and a drawn tick); in an option list it stands beside its own input.
  _ckBox(on, label, { round = false } = {}) {
    const bw = h("i", { class: "bw" }, h("i", { class: "bx" }));
    bw.insertAdjacentHTML("beforeend", J_TICK);
    return h("span", { class: `jck${on ? " on" : ""}${round ? " round" : ""}`, "data-jelly": "ck", "data-jl": label, "aria-hidden": "true" }, bw);
  }

  // The letters' weight: a slider over the four weights (the nearest one to a stored value).
  _weightSlider(value, onchange) {
    const at = WEIGHTS.reduce((best, [w], k) => (Math.abs(w - value) < Math.abs(WEIGHTS[best][0] - value) ? k : best), 0);
    return this._slider(at, 0, WEIGHTS.length - 1, 1, (k) => onchange(WEIGHTS[k][0]), (k) => WEIGHTS[k][1], { label: "Letters", live: true });
  }

  // live: the value is also sent while the thumb moves (at most every 140 ms), so the panel shows it at once
  _slider(value, min, max, step, onchange, fmt = (v) => String(v), { disabled = false, label = null, live = false } = {}) {
    const out = h("span", { class: "value" }, fmt(value));
    const fill = (v) => `${((v - min) / (max - min)) * 100}%`;
    let timer = null;
    let sent = value;
    const send = (v) => { if (v !== sent) { sent = v; onchange(v); } };
    const inp = h("input", {
      type: "range", min, max, step, value, disabled, "aria-label": label, style: { "--fill": fill(value) },
      oninput: (e) => {
        const v = +e.target.value;
        out.textContent = fmt(v);
        e.target.style.setProperty("--fill", fill(v));
        if (live && !timer) timer = setTimeout(() => { timer = null; send(+inp.value); }, 140);
      },
      onchange: (e) => { clearTimeout(timer); timer = null; send(+e.target.value); e.target.blur(); },
    });
    inp.value = value;
    return h("div", { class: "slider" }, inp, out);
  }

  _text(value, onchange, { placeholder = "", max = 40, label = null, cls = "", type = "text" } = {}) {
    return h("input", {
      class: `inp ${cls}`, type, value: value || "", placeholder, maxlength: max, "aria-label": label || placeholder || "Text",
      onchange: (e) => onchange(e.target.value), onkeydown: (e) => { if (e.key === "Enter") e.target.blur(); },
    });
  }

  // One entity to choose: its name on a button (long names wrap), the picker on a tap.
  _entityButton(current, { title, domains, none = null, onPick, p = null, filter = null, back = null, exclude = null }) {
    const name = current ? this._ename(current) : none || "Choose";
    return h("button", { class: "btn small pickbtn", "aria-label": `${title}: ${name}`, onclick: () => this._openSheet({ kind: "pick", title, domains, current, none, filter, exclude, panel: p?.entry_id, onPick, back }) },
      current ? icon(this._eicon(current)) : null, h("span", null, name), icon("chevron-down"));
  }

  _times(from, to, onchange) {
    return h("div", { style: { display: "flex", gap: "8px", alignItems: "center", flexWrap: "wrap" } },
      this._text(from, (v) => v && onchange(v, to), { type: "time", cls: "time", label: "From" }), "–",
      this._text(to, (v) => v && onchange(from, v), { type: "time", cls: "time", label: "To" }));
  }

  _infoBtn(key, text) {
    const open = this._ui.info === key || this._ui.allInfo;
    return [
      h("button", { class: "ib info", "aria-label": "More about this", "aria-expanded": open ? "true" : "false", onclick: (e) => {
        e.stopPropagation();
        if (this._stillSheet() && this._layer.contains(e.currentTarget)) { this._infoInPlace(e.currentTarget, key, text); return; }
        this._ui.info = open ? null : key;
        this._render();
      } }, icon("information-outline")),
      open && this._ui.info === key ? h("div", { class: "pop", role: "note", onclick: (e) => e.stopPropagation() }, text) : null,
    ];
  }

  // A sheet that background redraws leave alone (a list under the finger) is open.
  _stillSheet() {
    return Boolean(this._ui.sheet && STILL_SHEETS.has(this._ui.sheet.kind));
  }

  // In a still sheet the ⓘ opens and closes its bubble in place: the sheet is not drawn again, so a scrolled list
  // stays where it is.
  _infoInPlace(btn, key, text) {
    const wasOpen = this._ui.info === key;
    this._closePopsInPlace();
    if (wasOpen) return;
    this._ui.info = key;
    btn.setAttribute("aria-expanded", "true");
    btn.after(h("div", { class: "pop", role: "note", onclick: (e) => e.stopPropagation() }, text));
    this._placePops();
  }

  _closePopsInPlace() {
    this._ui.info = null;
    this._layer.querySelectorAll(".pop").forEach((p) => p.remove());
    this._layer.querySelectorAll(".ib.info[aria-expanded=true]").forEach((b) => b.setAttribute("aria-expanded", "false"));
  }

  _card(title, opts, ...body) {
    return h("section", { class: `card${opts.cls ? ` ${opts.cls}` : ""}` },
      title != null ? h("div", { class: "card-head" }, opts.lead || null,
        h("h2", null, title, opts.sub ? h("div", { class: "sub" }, opts.sub) : null),
        opts.acts || opts.info ? h("div", { class: "acts" }, opts.acts || null, opts.info ? this._infoBtn(opts.infoKey || `c:${title}`, opts.info) : null) : null) : null,
      ...body);
  }

  _row(label, control, opts = {}) {
    const cls = ["row", opts.stack && "stack", opts.click && "click", opts.dim && "dim", opts.keep && "keep", opts.cls].filter(Boolean).join(" ");
    return h("div", { class: cls, onclick: opts.click || null },
      opts.lead || null,
      h("div", { class: "lw" },
        h("div", { class: "label" }, h("span", null, label), opts.sub ? h("small", null, opts.sub) : null, opts.extra || null),
        opts.info ? h("span", { class: "rinfo", onclick: (e) => e.stopPropagation() }, this._infoBtn(opts.infoKey || `r:${label}`, opts.info)) : null),
      control != null ? h("div", { class: "control", onclick: opts.click ? (e) => e.stopPropagation() : null }, control) : null);
  }

  _more(key, ...rows) {
    const open = Boolean(this._ui.more[key]) || this._ui.allMore;
    const body = rows.flat().filter(Boolean);
    if (!body.length) return null;
    return h("div", { class: `more${open ? " open" : ""}` },
      h("button", { onclick: () => { this._ui.more[key] = !open; this._render(); }, "aria-expanded": open ? "true" : "false" }, icon("chevron-down"), open ? "Fewer options" : "More options"),
      open ? h("div", { class: "body" }, ...body) : null);
  }

  _btn(label, onclick, opts = {}) {
    return h("button", { class: `btn${opts.cls ? ` ${opts.cls}` : ""}`, onclick, disabled: Boolean(opts.disabled), title: opts.title || null, "aria-label": opts.aria || null, ...(opts.attrs || {}) },
      opts.icon ? icon(opts.icon) : null, h("span", null, label));
  }

  // A button that needs a second tap (restart, remove).
  _armed(key, label, armedLabel, action, opts = {}) {
    const armed = this._ui.armed === key;
    return this._btn(armed ? armedLabel : label, () => {
      if (armed) {
        this._ui.armed = null;
        action();
        this._render();
        return;
      }
      this._ui.armed = key;
      this._render();
      setTimeout(() => { if (this._ui.armed === key) { this._ui.armed = null; this._render(); } }, 4000);
    }, { ...opts, cls: `${armed ? "armed " : ""}${opts.cls || ""}` });
  }

  // Looks to pick from; the ones this panel cannot show are greyed, with the reason once below.
  _lookPicker(items, value, onpick, { wide = false } = {}) {
    const why = [...new Set(items.filter((it) => it.disabled && it.why).map((it) => it.why))];
    return [
      h("div", { class: `looks${wide ? " wide" : ""}`, role: "radiogroup" }, items.map((it) =>
        h("button", { class: `look${it.id === value ? " on" : ""}`, disabled: Boolean(it.disabled), title: it.why || null, role: "radio", "aria-checked": it.id === value ? "true" : "false", onclick: () => onpick(it.id) },
          h("div", { class: "pic" }, it.pic, it.id === value ? h("span", { class: "chk" }, icon("check")) : null),
          h("span", null, it.name)))),
      why.length ? h("p", { class: "note", style: { margin: "0 0 14px" } }, `Greyed looks are not available: ${why.join(" ")}`) : null,
    ];
  }

  _swatches(items, value, onpick, { wood = false } = {}) {
    return h("div", { class: "swatches" }, items.map((it) =>
      h("button", { class: `sw-lab${it.id === value ? " on" : ""}`, onclick: () => onpick(it.id), "aria-pressed": it.id === value ? "true" : "false" },
        h("span", { class: `sw${wood ? " wood" : ""}${it.id === value ? " on" : ""}`, style: { background: it.c } }), it.name)));
  }

  // -- sounds ------------------------------------------------------------------------

  // The uploaded sounds (house.sounds), without the ones waiting for their Undo after a delete.
  _ownSounds() {
    return (this._data?.house?.sounds || []).filter((s) => !this._ui.delSounds.has(s.id));
  }

  // A stored sound value as "lib:<id>" / "own:<id>"; "auto", unknown or deleted ones = the place's default.
  _soundKey(value, def) {
    const v = typeof value === "string" ? value : "";
    if (v.startsWith("ma:") && v.length > 3) return v;
    if (v.startsWith("lib:") && SOUND_LIB.some((s) => `lib:${s.id}` === v)) return v;
    if (v.startsWith("own:") && this._ownSounds().some((s) => `own:${s.id}` === v)) return v;
    return `lib:${def}`;
  }

  // the name of a Music Assistant item: from a search, else from an alarm that has it
  _maName(k) {
    const known = (this._ui.maNames || {})[k];
    if (known) return known;
    for (const p of this._panels || []) for (const a of p.alarms || []) if (a.tone === k && a.tone_name) return a.tone_name;
    return "Music";
  }

  _soundInfo(value, def) {
    const k = this._soundKey(value, def);
    if (k.startsWith("ma:")) return { k, own: false, ma: true, id: k.slice(3), name: this._maName(k), sub: "Music Assistant" };
    const id = k.slice(4);
    if (k.startsWith("own:")) {
      const s = this._ownSounds().find((x) => x.id === id);
      return { k, own: true, id, name: s.name, sub: this._soundSize(s) };
    }
    const s = SOUND_LIB.find((x) => x.id === id) || SOUND_LIB[2];
    return { k, own: false, id: s.id, name: s.name, sub: null };
  }

  _soundSize(s) {
    const size = `${Math.max(0.1, (s.bytes || 0) / 1048576).toFixed(1)} MB`;
    const secs = Number(s.secs) || 0;
    return secs ? `${size} · ${secs < 10 ? secs.toFixed(1) : Math.round(secs)} s` : size;
  }

  // Home Assistant's authenticated fetch (the sound files and the upload).
  _authFetch(url, init = {}) {
    if (typeof this._hass.fetchWithAuth === "function") return this._hass.fetchWithAuth(url, init);
    const token = this._hass.auth?.data?.access_token;
    return fetch(url, { ...init, headers: { ...(init.headers || {}), Authorization: `Bearer ${token}` } });
  }

  async _ownSoundUrl(id) {
    if (this._soundUrls[id]) return this._soundUrls[id];
    const res = await this._authFetch(`/api/panelos/sound/${encodeURIComponent(id)}`);
    if (!res.ok) throw new Error(`The sound could not be loaded (HTTP ${res.status}).`);
    const blob = await res.blob();
    const url = URL.createObjectURL(blob.type ? blob : new Blob([blob], { type: "audio/wav" }));
    this._soundUrls[id] = url;
    return url;
  }

  // ▶ on a button with this key plays the sound k ("lib:<id>" / "own:<id>"), ■ (the same key again) stops it;
  // another one playing stops first.
  async _play(key, k) {
    const again = this._ui.playing === key;
    this._stopSound();
    if (again) return;
    this._ui.playing = key;
    this._refreshPlay();
    let src;
    try {
      src = k.startsWith("own:") ? await this._ownSoundUrl(k.slice(4)) : `${this._static}/sounds/${k.slice(4)}.mp3`;
    } catch (err) {
      if (this._ui.playing === key) this._stopSound();
      this._toast(err.message || String(err), null, true);
      return;
    }
    if (this._ui.playing !== key) return; // stopped while it loaded
    const audio = new Audio(src);
    this._audio = audio;
    const end = () => { if (this._audio === audio) this._stopSound(); };
    audio.addEventListener("ended", end);
    audio.addEventListener("error", () => { if (this._audio === audio) { this._stopSound(); this._toast("The sound could not be played.", null, true); } });
    audio.play().catch((err) => {
      if (this._audio !== audio || err?.name === "AbortError") return;
      this._stopSound();
      this._toast("The sound could not be played.", null, true);
    });
  }

  _stopSound() {
    if (this._audio) {
      const a = this._audio;
      this._audio = null;
      a.pause();
    }
    if (this._ui.playing != null) {
      this._ui.playing = null;
      this._refreshPlay();
    }
  }

  // The play buttons follow what plays without drawing the page again.
  _refreshPlay() {
    this.shadowRoot?.querySelectorAll("[data-play]").forEach((b) => {
      const on = this._ui.playing === b.dataset.play;
      if (b.classList.contains("on") === on) return;
      b.classList.toggle("on", on);
      b.setAttribute("aria-label", `${on ? "Stop" : "Listen to"} ${b.dataset.name}`);
      b.title = on ? "Stop" : "Listen";
      b.replaceChildren(icon(on ? "stop" : "play"));
    });
  }

  _playBtn(key, k, name) {
    if (k.startsWith("ma:")) return h("span", { class: "playbtn ma", "aria-hidden": "true" }, icon("music-note"));
    const on = this._ui.playing === key;
    return h("button", { class: `playbtn${on ? " on" : ""}`, "data-play": key, "data-name": name, "aria-label": `${on ? "Stop" : "Listen to"} ${name}`, title: on ? "Stop" : "Listen",
      onclick: (e) => { e.stopPropagation(); this._play(key, k); } }, icon(on ? "stop" : "play"));
  }

  // A sound setting: ▶ and the chosen sound (marked when it is the place's default); a tap opens the
  // picker. ctx: {def, defNote, title, crit, p, parent (the sheet to go back to)}.
  _soundRow(label, value, ctx, onpick, opts = {}) {
    const s = this._soundInfo(value, ctx.def);
    const isDef = s.k === `lib:${ctx.def}`;
    const open = () => this._openSheet({ kind: "sound", title: ctx.title, value: value || "auto", def: ctx.def, defNote: ctx.defNote, crit: Boolean(ctx.crit), ma: Boolean(ctx.ma), panel: ctx.p.entry_id, parent: ctx.parent || null, onpick });
    return this._row(label, h("div", { class: "sndctl" },
      this._playBtn(`row:${ctx.title}:${s.k}`, s.k, s.name),
      h("button", { class: "sndpick", onclick: open, "aria-label": `${label}: ${s.name}${isDef ? ", the default" : ""}. Change` },
        h("span", null, s.name), isDef ? h("small", null, "Default") : null, icon("chevron-right"))), { info: opts.info, infoKey: opts.infoKey, cls: opts.cls });
  }

  // Upload a sound for every panel: POST /api/panelos/sounds (file, name); the reply is the new
  // house.sounds entry. done(value) gets "own:<id>".
  async _uploadSound(file, done) {
    if (!file) return;
    if (!SOUND_TYPES.test(file.name) && !/^audio\/(mpeg|mp3|wav|x-wav|wave|vnd\.wave|ogg|flac|x-flac)$/i.test(file.type)) { this._toast("MP3, WAV, OGG or FLAC files only.", null, true); return; }
    if (file.size > SOUND_UPLOAD_MAX) { this._toast("Sounds up to 10 MB only.", null, true); return; }
    const name = file.name.replace(/\.[^.]+$/, "").trim().slice(0, 40) || "Sound";
    const form = new FormData();
    form.append("name", name);
    form.append("file", file);
    this._ui.uploading = { name };
    this._renderLayer();
    try {
      const res = await this._authFetch("/api/panelos/sounds", { method: "POST", body: form });
      let body = null;
      try { body = await res.json(); } catch (_err) { body = null; }
      if (!res.ok || !body?.id) throw new Error(body?.message || `Not uploaded (HTTP ${res.status}).`);
      const hs = this._data.house || (this._data.house = {});
      if (!(hs.sounds || []).some((s) => s.id === body.id)) hs.sounds = [...(hs.sounds || []), { id: body.id, name: body.name || name, bytes: body.bytes || file.size, secs: body.secs || 0 }];
      this._ui.uploading = null;
      done(`own:${body.id}`);
      this._toast(`${body.name || name} uploaded`);
    } catch (err) {
      this._ui.uploading = null;
      this._toast(err.message || String(err), null, true);
    }
    this._renderLayer();
  }

  // Delete an uploaded sound: it disappears at once, and is deleted (the house saved without it, which
  // deletes the file) when the Undo is gone.
  _deleteSound(s, sh) {
    this._stopSound();
    this._ui.delSounds.add(s.id);
    const was = sh && sh.value === `own:${s.id}` ? sh.value : null;
    if (was) { sh.value = "auto"; sh.onpick("auto"); }
    this._renderLayer();
    this._toast(`${s.name} deleted`, () => {
      this._ui.delSounds.delete(s.id);
      if (was) { sh.value = was; sh.onpick(was); }
      this._renderLayer();
    }, false, { onExpire: () => {
      this._saveHouse((hs) => { hs.sounds = hs.sounds.filter((x) => x.id !== s.id); }).then(() => {
        this._ui.delSounds.delete(s.id);
        if (this._soundUrls[s.id]) { URL.revokeObjectURL(this._soundUrls[s.id]); delete this._soundUrls[s.id]; }
        this._renderLayer();
      });
    } });
  }

  // -- menus ---------------------------------------------------------------------------

  _openMenu(anchor, items) {
    this._closeMenu(true);
    const m = h("div", { class: "menu", role: "menu" }, items.filter(Boolean).map((it) => (it === "-" ? h("hr") :
      h("button", { role: "menuitem", class: it.danger ? "danger" : "", disabled: Boolean(it.disabled), title: it.title || null, onclick: (e) => { e.stopPropagation(); this._closeMenu(); it.run(); } }, icon(it.icon || "chevron-right"), it.label))));
    this.shadowRoot.append(m);
    const r = anchor.getBoundingClientRect();
    const mw = m.offsetWidth;
    const mh = m.offsetHeight;
    let x = Math.min(r.right - mw, window.innerWidth - mw - 12);
    x = Math.max(12, x);
    let y = r.bottom + 6;
    if (y + mh > window.innerHeight - 12) y = Math.max(12, r.top - mh - 6);
    m.style.left = `${x}px`;
    m.style.top = `${y}px`;
    this._menu = m;
    this._menuAnchor = anchor;
    anchor.setAttribute("aria-expanded", "true");
  }

  _closeMenu(quiet = false) {
    if (!this._menu) return;
    this._menu.remove();
    this._menu = null;
    this._menuAnchor?.setAttribute("aria-expanded", "false");
    if (!quiet) this._flush();
  }

  // -- sheets ---------------------------------------------------------------------------

  _openSheet(sheet) {
    this._stopSound();
    this._ui.sheet = sheet;
    this._ui.sheetFresh = true;
    this._ui.info = null;
    this._renderLayer();
  }

  _closeSheet() {
    this._stopSound();
    this._ui.sheet = null;
    this._discardUntouched();
    this._renderLayer();
    this._flush();
  }

  // A new critical alert or status screen closed without any change is not kept.
  _discardUntouched() {
    const n = this._fresh;
    this._fresh = null;
    if (!n || n.touched) return;
    if (!(this._data?.house?.[n.kind] || []).some((x) => x.id === n.id)) return;
    this._saveHouse((hs) => { hs[n.kind] = (hs[n.kind] || []).filter((x) => x.id !== n.id); });
  }

  _renderLayer() {
    const sh = this._ui.sheet;
    let node = null;
    if (sh && this._data) {
      const fn = this[`_sheet_${sh.kind}`];
      try {
        node = fn ? fn.call(this, sh, (sh.panel && this._panelById(sh.panel)) || this._cur()) : null;
      } catch (err) {
        console.error("PanelOS sheet", sh.kind, err);
        node = null;
        this._ui.sheet = null;
        this._toast("This could not be opened. Reload the page and try again.", null, true);
      }
    }
    if (node && !this._ui.sheetFresh) node.classList.add("noanim");
    // the same sheet drawn again keeps its scroll (a pick, an upload, a search result)
    const oldBody = this._layer.querySelector(".sheet-body");
    const keepScroll = node && !this._ui.sheetFresh && this._layerSheet === sh && oldBody ? oldBody.scrollTop : 0;
    this._layerSheet = node ? sh : null;
    this._ui.sheetFresh = false;
    // a field being typed in keeps the focus and the caret when the sheet is drawn again (a search's results)
    const a = this.shadowRoot && this.shadowRoot.activeElement;
    const keep = a && a.tagName === "INPUT" && this._layer.contains(a) && a.getAttribute("aria-label")
      ? { label: a.getAttribute("aria-label"), start: a.selectionStart, end: a.selectionEnd } : null;
    this._layer.replaceChildren(node || "");
    if (keepScroll) {
      const body = this._layer.querySelector(".sheet-body");
      if (body) body.scrollTop = keepScroll;
    }
    // an open bubble is placed by its button again
    if (this._ui.info && this._layer.querySelector(".pop")) this._placePops();
    if (keep) {
      const n = [...this._layer.querySelectorAll("input")].find((x) => x.getAttribute("aria-label") === keep.label);
      if (n) {
        n.focus();
        try { n.setSelectionRange(keep.start, keep.end); } catch (_e) { /* not a text field */ }
      }
    }
  }

  _sheetFrame(title, body, foot, opts = {}) {
    return h("div", { class: `layer${opts.center ? " center" : ""}` },
      h("div", { class: "scrim", onclick: () => this._closeSheet() }),
      h("div", { class: `sheet${opts.wide ? " wide" : ""}`, role: "dialog", "aria-modal": "true", "aria-label": title },
        h("i", { class: "grab" }),
        h("div", { class: "sheet-head" },
          opts.back ? h("button", { class: "ib", "aria-label": "Back", onclick: opts.back }, icon("chevron-left")) : null,
          h("h2", null, title),
          opts.info ? h("span", { class: "rinfo" }, this._infoBtn(`s:${title}`, opts.info)) : null,
          h("button", { class: "ib", "aria-label": "Close", onclick: () => this._closeSheet() }, icon("close"))),
        h("div", { class: "sheet-body" }, body),
        foot ? h("div", { class: "sheet-foot" }, foot) : null));
  }

  // On a phone the preview column is hidden: show the screen in a sheet while a test runs.
  _previewSheetIfNarrow(p) {
    if (this.getAttribute("size") === "narrow" && this._ui.sheet?.kind !== "preview") this._openSheet({ kind: "preview", panel: p.entry_id });
  }

  // -- the device frame with the live picture ---------------------------------------------

  _deviceNode(p, size) {
    const pic = this._pics[p.entry_id];
    const ar = pic ? pic.w / pic.h : 1;
    let W = size;
    let H = size;
    let gw = size * 0.76;
    let gh = gw / ar;
    if (ar < 0.9) { H = size; W = size * Math.max(0.55, ar + 0.12); gh = H * 0.86; gw = gh * ar; }
    else if (ar > 1.1) { W = size; H = size * Math.max(0.55, 1 / ar + 0.16); gw = W * 0.86; gh = gw / ar; }
    const gx = (W - gw) / 2;
    const gy = (H - gh) / 2;
    return h("div", { class: "device", style: { width: `${W}px`, height: `${H}px` } },
      h("div", { class: "bezel", style: { borderRadius: `${Math.max(10, size * 0.06)}px` } }),
      h("div", {
        class: "glass", title: p.connected ? "Tap to wake the screen" : null,
        style: { left: `${gx}px`, top: `${gy}px`, width: `${gw}px`, height: `${gh}px`, borderRadius: `${Math.max(3, size * 0.012)}px` },
        onclick: () => p.connected && this._command(p, "screen", true),
      }, this._picture(p)));
  }

  _previewBlock(p, { width = null, height = null } = {}) {
    const colW = width || (this.getAttribute("size") === "wide" ? (this.hasAttribute("pvbig") ? 522 : 348) : 300);
    const avH = height || Math.max(240, (this.clientHeight || window.innerHeight) - (this.getAttribute("size") === "wide" ? 240 : 330));
    const size = Math.max(180, Math.min(colW, avH));
    return h("div", { class: "pv" },
      h("div", { class: "pv-title" }, h("span", { class: `state${p.connected && !this._use.paused ? " on" : ""}` }, h("b")), p.connected ? (this._use.paused ? "Paused" : "Live") : "Offline",
        h("span", { class: "rinfo" }, this._infoBtn("pv", "What the panel's screen shows now, live while you use this page. After a minute without use it stops on the last picture and goes on when you use the page again. Tap the screen to wake it."))),
      this._deviceNode(p, size),
      h("div", { class: "pv-acts" },
        this._btn("Wake", () => this._command(p, "screen", true), { cls: "small", icon: "gesture-tap", disabled: !p.connected }),
        this._btn("Screensaver", () => this._command(p, "screen", false), { cls: "small", icon: "sleep", disabled: !p.connected })),
      this._syncLine(p));
  }

  // ================================================================ views

  _hero(title, meta, acts = null, menu = true) {
    return h("div", { class: "hero" },
      h("div", { class: "t" }, h("h1", null, title), meta ? h("div", { class: "meta" }, meta) : null),
      acts ? h("div", { class: "acts" }, acts) : null,
      menu ? h("button", { class: "ib only-xl", "aria-label": "More", "aria-haspopup": "menu", onclick: (e) => { e.stopPropagation(); this._moreMenu(e.currentTarget); } }, icon("dots-vertical")) : null);
  }

  _state(p) {
    return h("span", { class: `state${p.connected ? " on" : ""}` }, h("b"), p.connected ? "Online" : "Offline");
  }

  _gl(ic, label, value, sub, live = null) {
    return h("div", { class: "gl" }, h("small", null, icon(ic), label), h("b", { "data-live": live }, value), sub ? h("em", null, sub) : null);
  }

  // -- overview -----------------------------------------------------------------

  _viewOverview(p) {
    const s = p.states || {};
    const shown = this._pages(p).filter((x, i) => i === 0 || !x.hidden);
    const version = s.app_version || p.device?.sw_version;
    const num = (v) => typeof v === "number" && !Number.isNaN(v);
    const tiles = [
      this._has(p, "light") ? this._gl("brightness-6", "Light", num(s.illuminance) ? `${Math.round(s.illuminance)} lx` : "–", "Built-in sensor") : null,
      this._has(p, "proximity") ? this._gl("motion-sensor", "Presence", s.presence == null ? "–" : s.presence ? "Someone" : "Nobody", "In front of the panel") : null,
      this._gl("monitor", "Screen", s.screen == null ? "–" : s.screen ? "On" : "Screensaver",
        s.auto_brightness ? "Automatic brightness" : num(s.brightness) ? `Brightness ${s.brightness} %` : null),
      this._gl("wifi", "Wi-Fi", num(s.wifi_signal) ? `${s.wifi_signal} dBm` : "–", s.ip_address || null),
      this._gl("chip", "Chip temperature", num(s.soc_temperature) ? `${s.soc_temperature.toFixed(1)} °C` : "–", null),
      this._gl("view-grid-outline", "Pages", String(shown.length), "to swipe through"),
    ];
    const house = this._data.house || {};
    const active = house.active || { critical: [], status: [] };
    const crit = (house.critical || []).filter((c) => active.critical.includes(c.entity_id) && !(c.off || []).includes(p.entry_id));
    const ownCrit = (house.own_critical || []).filter((c) => active.critical.includes(`own:${c.id}`) && !(c.off || []).includes(p.entry_id));
    const stat = (house.status || []).filter((x) => active.status.includes(x.id) && x.panels.includes(p.entry_id));
    const hero = this._hero(this._pname(p), [this._state(p), h("span", null, this._pmodel(p)), this._parea(p) ? h("span", null, this._parea(p)) : null,
      version ? h("span", null, `Version ${version}`) : null, this._syncLine(p)]);
    hero.classList.add("ov-hero");
    return [
      hero,
      h("section", { class: "card only-narrow pv-card" }, this._previewBlock(p, { width: Math.min((this.clientWidth || 360) - 64, 330), height: 330 })),
      p.loaded ? null : this._card(null, {}, this._row("This panel's entry is not loaded", null, { sub: "Check it in Settings → Devices and services.", lead: h("span", { class: "lead" }, icon("alert-outline")), cls: "first", keep: true })),
      this._card("Right now", { info: "Live values from the panel. In Home Assistant they are entities of the panel's device, so automations can use them." },
        h("div", { class: "glance" }, tiles)),
      this._card("Alerts and status now", { info: "Critical alerts and status screens that are on at this moment on this panel. They are set in Alerts." },
        crit.length || ownCrit.length || stat.length
          ? h("div", { class: "below" }, [
            ...crit.map((c) => this._row(this._ename(c.entity_id), null, { lead: h("span", { class: "lead on" }, icon(this._eicon(c.entity_id))), sub: "Critical alert", keep: true })),
            ...ownCrit.map((c) => this._row(this._ownCritName(c), null, { lead: h("span", { class: "lead on" }, icon("alert-octagon-outline")), sub: "Critical alert", keep: true })),
            ...stat.map((x) => this._row(this._statusName(x), null, { lead: this._stLead(x), sub: this._trigText(x), keep: true })),
          ])
          : h("div", { class: "empty" }, "Nothing is on right now.")),
      this._notesCard(p),
    ];
  }

  // -- pages ------------------------------------------------------------------------

  _pageSub(p, page, i) {
    const bits = [];
    if (i === 0) bits.push("Start page");
    if (page.hidden && i > 0) bits.push("Hidden");
    const ents = page.entities || [];
    const names = (domain) => ents.filter((e) => e.startsWith(`${domain}.`));
    if (page.kind === "home" || page.kind === "weather") {
      bits.push((BACKGROUNDS[this._bgOf(p, page)] || {}).name);
      bits.push(plural(this._items(page).length, "widget", "widgets"));
      if (page.kind === "weather") bits.push(ents[0] ? this._ename(ents[0]) : "The panel's weather");
    } else {
      const looks = LOOKS[page.kind] || [];
      if (looks.length > 1) bits.push(looks[page.variant || 0] || looks[0]);
      if (page.kind === "music") bits.push(ents[0] ? this._ename(ents[0]) : "The panel's player");
      if (page.kind === "weather") bits.push(ents[0] ? this._ename(ents[0]) : "The panel's weather");
      if (page.kind === "thermostat") bits.push(ents.length ? this._ename(ents[0]) + (ents.length > 1 ? ` and ${ents.length - 1} more` : "") : "No thermostat yet");
      if (page.kind === "quick") bits.push(ents.length ? this._enames(ents) : "No tiles yet");
      if (page.kind === "camera") bits.push(ents.length ? this._enames(ents) : "No camera yet");
      if (page.kind === "calendar") bits.push(names("calendar").length ? this._enames(names("calendar")) : "No calendar yet");
      if (page.kind === "todo") bits.push(ents.length ? ents.map((e) => this._ename(e)).join(", ") : "No list yet");
      if (page.kind === "info" && (page.variant || 0) === 2) bits.push((PAGER_FINISH.find((f) => f.id === page.options?.finish) || PAGER_FINISH[0]).name);
    }
    return bits.filter(Boolean).join(" · ");
  }

  // A few entities by name, the rest counted ("Desk lamp, Shutter and 3 more").
  _enames(ids, most = 2) {
    const shown = ids.slice(0, most).map((e) => this._ename(e)).join(", ");
    return ids.length > most ? `${shown} and ${ids.length - most} more` : shown;
  }

  _viewPages(p) {
    const pages = this._pages(p);
    const shown = pages.filter((x, i) => i === 0 || !x.hidden).length;
    const max = this._data.max_pages || 12;
    const st = this._settings(p);
    return [
      this._hero("Pages", [h("span", null, this._pname(p)), h("span", null, `${shown} of ${pages.length} shown`), this._syncLine(p)]),
      this._card("Swipe order", {
        info: "The panel shows these pages in this order; swipe left and right between them. After the last page the first comes again, in both directions. The first page is the start page: it stays first and the panel returns to it. A page type can be added more than once, for example two Custom pages with different backgrounds. A hidden page keeps its settings.",
        acts: this._btn("Add a page", () => this._openSheet({ kind: "catalogue", panel: p.entry_id }), { cls: "small", icon: "plus", disabled: pages.length >= max, title: pages.length >= max ? `At most ${max} pages` : null }),
      },
      h("div", { class: "plist" }, pages.map((page, i) => {
        const t = PAGE_TYPES[page.kind] || { name: page.kind };
        const open = () => this._go(`${this._panelPath(p, "pages")}/${i}`);
        return h("div", { class: `prow${page.hidden && i > 0 ? " hidden-page" : ""}`, "data-i": i },
          h("button", { class: `handle${i === 0 ? " fixed" : ""}`, "aria-label": i === 0 ? "The start page stays first" : `Drag to reorder ${t.name}`, onpointerdown: i === 0 ? null : (e) => this._startPageDrag(e, p) }, icon("drag-vertical")),
          h("div", { class: "pthumb", onclick: open }, this._lookPic(p, lookName(page.kind, page.variant, page), () => lookArt(page.kind, page.variant || 0, page, this._plainMode(p)))),
          h("div", { class: "ptxt", onclick: open }, h("b", null, t.name), h("small", null, this._pageSub(p, page, i))),
          i === 0 ? null : this._toggle(!page.hidden, (v) => this._setPage(p, i, { hidden: !v }), { label: `Show ${t.name}` }),
          h("button", { class: "ib", "aria-label": "Page options", "aria-haspopup": "menu", onclick: (e) => { e.stopPropagation(); this._pageMenu(e.currentTarget, p, i); } }, icon("dots-vertical")),
          h("span", { class: "chev", onclick: open }, icon("chevron-right")));
      }))),
      this._card("Used by every page", { info: "Pages and widgets that do not choose their own weather or player use these." },
        this._row("Weather", this._entityButton(st.weather_entity || "", { title: "Weather", domains: ["weather"], none: "None", p, onPick: (v) => this._options(p, { weather_entity: v }, { optimistic: (s) => { s.weather_entity = v; } }) })),
        this._row("Player", this._entityButton(st.media_player_entity || "", { title: "Player", domains: ["media_player"], none: "None", p, onPick: (v) => this._options(p, { media_player_entity: v }, { optimistic: (s) => { s.media_player_entity = v; } }) }))),
      this._has(p, "web") ? this._dashboardCard(p) : null,
    ];
  }

  _settings(p) {
    return p.settings || {};
  }

  _dashboardCard(p) {
    const st = this._settings(p);
    const s = p.states || {};
    const modes = p.select_options?.display_mode || [];
    return this._card("Dashboard", {
      sub: "A Home Assistant dashboard in the web view",
      info: "The panel can show a Home Assistant dashboard instead of its own pages, custom cards included. It is logged in as its own Home Assistant user: dashboards for administrators only, or views shown only to some users, stay empty on it. \"Chosen on the panel\" leaves the choice to the panel's own setting.",
    },
    modes.length ? this._row("The panel shows", this._seg(modes.map((m) => [m, m]), s.display_mode, (v) => this._command(p, "display_mode", v), { label: "Display mode" })) : null,
    this._row("Dashboard", this._dropdown((this._data.dashboards || []).map((d) => [d.value, d.label]), st.dashboard || "", (v) => this._options(p, { dashboard: v }, { optimistic: (x) => { x.dashboard = v; } }), { label: "Dashboard" })),
    this._row("Header", this._toggle(st.show_header, (v) => this._options(p, { show_header: v }, { optimistic: (x) => { x.show_header = v; } }), { label: "Header" })),
    this._row("Sidebar", this._toggle(st.show_sidebar, (v) => this._options(p, { show_sidebar: v }, { optimistic: (x) => { x.show_sidebar = v; } }), { label: "Sidebar" })),
    this._more(`dash:${p.entry_id}`,
      this._row("Size", this._slider(st.zoom ?? 100, 50, 150, 5, (v) => this._options(p, { zoom: v }, { optimistic: (x) => { x.zoom = v; } }), (v) => `${v} %`, { label: "Size" })),
      this._row("Loading screen", this._toggle(st.loading_screen ?? true, (v) => this._options(p, { loading_screen: v }, { optimistic: (x) => { x.loading_screen = v; } }), { label: "Loading screen" })),
      this._resourcesRows(p)));
  }

  _resourcesRows(p) {
    const st = this._settings(p);
    const res = p.resources || [];
    const keep = new Set(st.keep_resources || []);
    const loaded = (r) => r.needed || !st.skip_unused || keep.has(r.url);
    const out = res.filter((r) => !loaded(r));
    const saved = out.reduce((sum, r) => sum + (r.size || 0), 0);
    const rows = [this._row("Load only the cards it uses", this._toggle(st.skip_unused, (v) => this._options(p, { skip_unused_resources: v }, { optimistic: (x) => { x.skip_unused = v; } }), { label: "Load only the cards it uses" }), {
      sub: res.length ? `${res.length - out.length} of ${res.length} card resources load${saved ? ` · ${mb(saved)} left out` : ""}` : null,
      info: "The panel loads only the card resources the dashboard it shows uses, so it opens faster. Switch one on if a card stays empty.",
    })];
    if (res.length) {
      rows.push(h("div", { class: "row stack" }, h("div", { class: "res" }, res.map((r) => {
        const path = r.url.split("?")[0];
        const parts = path.split("/").filter(Boolean);
        const file = parts.pop() || path;
        const folder = parts.length ? parts[parts.length - 1] : "";
        return h("div", { class: "item" },
          h("div", { class: "name" }, folder && folder !== "local" ? folder : file, folder && folder !== file.replace(/\.js$/, "") ? h("small", null, file) : null),
          r.needed ? h("span", { class: "tag sig" }, "Used") : null,
          h("span", { class: "size" }, r.size ? mb(r.size) : "–"),
          this._toggle(loaded(r), (v) => {
            const now = new Set(keep);
            if (v) now.add(r.url);
            else now.delete(r.url);
            this._options(p, { keep_resources: [...now] }, { optimistic: (x) => { x.keep_resources = [...now]; } });
          }, { label: `Always load ${file}`, disabled: r.needed || !st.skip_unused }));
      }))));
    }
    return rows;
  }

  _pageMenu(anchor, p, i) {
    const pages = this._pages(p);
    const page = pages[i];
    const t = PAGE_TYPES[page.kind] || { name: page.kind };
    const max = this._data.max_pages || 12;
    const move = (d) => {
      const next = clone(pages);
      next.splice(i + d, 0, next.splice(i, 1)[0]);
      this._savePages(p, next);
    };
    this._openMenu(anchor, [
      // (not on the page's own settings: they are open already)
      this._route().view === "page" ? null : { icon: "pencil-outline", label: "Edit", run: () => this._go(`${this._panelPath(p, "pages")}/${i}`) },
      { icon: "arrow-up", label: "Move up", disabled: i <= 1, run: () => move(-1) },
      { icon: "arrow-down", label: "Move down", disabled: i === 0 || i >= pages.length - 1, run: () => move(1) },
      "-",
      { icon: "content-copy", label: "Copy to other panels…", disabled: this._panels.length < 2, run: () => this._openCopy(p, i) },
      { icon: "plus-box-multiple-outline", label: "Duplicate", disabled: pages.length >= max, run: () => {
        const next = clone(pages);
        const copy = { ...clone(page), hidden: false };
        next.splice(i + 1, 0, copy);
        this._savePages(p, next, { toast: `${t.name} duplicated` });
      } },
      { icon: "delete-outline", label: "Remove", danger: true, disabled: i === 0, run: () => this._removePage(p, i) },
    ]);
  }

  _removePage(p, i) {
    const pages = clone(this._pages(p));
    const [gone] = pages.splice(i, 1);
    const t = PAGE_TYPES[gone.kind] || { name: gone.kind };
    this._savePages(p, pages, {
      toast: `${t.name} removed from ${this._pname(p)}`,
      undo: () => { const back = clone(this._pages(p)); back.splice(i, 0, gone); this._savePages(p, back); },
    });
    if (this._route().view === "page") this._go(this._panelPath(p, "pages"));
  }

  // Drag a page row by its handle (mouse and touch); the start page stays first.
  _startPageDrag(e, p) {
    this._startDrag(e, ".prow", { fixed: (el) => el.dataset.i === "0" }, (order) => {
      const pages = this._pages(p);
      this._savePages(p, order.map((k) => clone(pages[k])), { toast: `New order sent to ${this._pname(p)}` });
    });
  }

  // Drag an item of a list (or of a grid, `grid`) by its handle, mouse and touch alike: the item follows the pointer and
  // passes a neighbour once its middle is past the neighbour's middle (in a grid: once the pointer is over a
  // neighbour's place); `fixed` items stay where they are. When it is let go, `done` gets the new order of the items'
  // `data-i` (only when it changed).
  _startDrag(e, sel, { fixed = () => false, grid = false } = {}, done) {
    e.preventDefault();
    e.stopPropagation();
    const rowEl = e.currentTarget.closest(sel);
    const list = rowEl.parentElement;
    const items = () => [...list.children].filter((c) => c.matches(sel));
    let startX = e.clientX;
    let startY = e.clientY;
    this._dragging = true;
    rowEl.classList.add("dragging");
    // the item's place moved in the list: the picture stays under the pointer
    const moved = (fn) => {
      const [left, top] = [rowEl.offsetLeft, rowEl.offsetTop];
      fn();
      startX += rowEl.offsetLeft - left;
      startY += rowEl.offsetTop - top;
    };
    const move = (ev) => {
      ev.preventDefault();
      if (grid) {
        const over = items().find((c) => {
          if (c === rowEl || fixed(c)) return false;
          const b = c.getBoundingClientRect();
          return ev.clientX >= b.left && ev.clientX <= b.right && ev.clientY >= b.top && ev.clientY <= b.bottom;
        });
        if (over) {
          const all = items();
          moved(() => list.insertBefore(rowEl, all.indexOf(over) < all.indexOf(rowEl) ? over : over.nextSibling));
        }
      } else {
        const r = rowEl.getBoundingClientRect();
        const mid = r.top + r.height / 2;
        const prev = rowEl.previousElementSibling;
        const next = rowEl.nextElementSibling;
        if (prev && prev.matches(sel) && !fixed(prev)) {
          const pr = prev.getBoundingClientRect();
          if (mid < pr.top + pr.height / 2) moved(() => list.insertBefore(rowEl, prev));
        }
        if (next && next.matches(sel) && !fixed(next)) {
          const nr = next.getBoundingClientRect();
          if (mid > nr.top + nr.height / 2) moved(() => list.insertBefore(next, rowEl));
        }
      }
      rowEl.style.transform = grid ? `translate(${ev.clientX - startX}px, ${ev.clientY - startY}px)` : `translateY(${ev.clientY - startY}px)`;
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      this._dragging = false;
      rowEl.classList.remove("dragging");
      rowEl.style.transform = "";
      // (the click that ends a drag opens nothing)
      this._dragEnded = Date.now();
      const order = items().map((c) => Number(c.dataset.i));
      if (order.some((v, k) => v !== k)) done(order);
      else this._render();
    };
    window.addEventListener("pointermove", move, { passive: false });
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  }

  // -- page editor ------------------------------------------------------------------

  _viewPage(p, i) {
    const pages = this._pages(p);
    const page = pages[i];
    if (!page) {
      setTimeout(() => this._go(this._panelPath(p, "pages"), true));
      return [];
    }
    const t = PAGE_TYPES[page.kind] || { name: page.kind };
    // the panel shows this page while its settings are open (its look and options are seen live)
    this._livePage(p, i);
    const editor = { home: this._editHome, weather: this._editWeather, clock: this._editClock, music: this._editMusic, timers: this._editTimers, thermostat: this._editThermostat, quick: this._editQuick, camera: this._editCamera, calendar: this._editCalendar, todo: this._editTodo, info: this._editInfo }[page.kind];
    return [
      h("button", { class: "back", onclick: () => this._go(this._panelPath(p, "pages")) }, icon("chevron-left"), "Pages"),
      this._hero(t.name, [h("span", null, this._pname(p)), h("span", null, `Page ${i + 1} of ${pages.length}`), i === 0 ? h("span", { class: "tag" }, "Start page") : page.hidden ? h("span", { class: "tag" }, "Hidden") : null, this._syncLine(p)],
        [i === 0 ? null : this._toggle(!page.hidden, (v) => this._setPage(p, i, { hidden: !v }), { label: "Shown on the panel" }),
          h("button", { class: "ib", "aria-label": "Page options", "aria-haspopup": "menu", onclick: (e) => { e.stopPropagation(); this._pageMenu(e.currentTarget, p, i); } }, icon("dots-vertical"))], false),
      this._lookCard(p, page, i),
      ...(editor ? editor.call(this, p, page, i) : []),
    ];
  }

  // The background a Custom or Weather details page shows (a BACKGROUNDS index): its own; a Weather details page
  // without one has the first Custom page's (without a Custom page, the panel's own: Photo · nature).
  _bgOf(p, page) {
    if (page.kind !== "weather" || Number.isInteger(page.options?.theme)) return bgIndex(page);
    const home = this._pages(p).find((x) => x.kind === "home");
    return home ? bgIndex(home) : 0;
  }

  _lookCard(p, page, i) {
    const o = page.options || {};
    if (page.kind === "home" || page.kind === "weather") {
      // the Weather details page: the same backgrounds; its cars, traffic and real sun and moon are the Custom page's
      const weather = page.kind === "weather";
      const cur = this._bgOf(p, page);
      const sw = BACKGROUNDS[cur].theme === 8;
      const plain = BACKGROUNDS[cur].theme === PLAIN_THEME;
      const pics = this._has(p, "images");
      const ps = p.states || {};
      const items = BACKGROUNDS.map((b, k) => {
        const ok = pics || b.theme === PLAIN_THEME;
        return { id: k, name: b.name, pic: this._lookPic(p, `bg-${k}`, () => svgNode(bgArt(k, this._plainMode(p)))), disabled: !ok, why: ok ? null : this._reason(p, "images") };
      });
      return this._card("Background", { info: weather
        ? "The backgrounds follow the real weather and the time of day. Plain is one colour, the panel's own dark or light ground, as its theme is. Until one is chosen here, this page has the first Custom page's background. The synthwave cars and the real sun and moon follow the Custom page's settings; the panel shows one synthwave scene, chosen by the first page with a synthwave background."
        : "The backgrounds follow the real weather and the time of day. Plain is one colour, the panel's own dark or light ground, as its theme is. The panel shows one synthwave scene, chosen by the first page with a synthwave background." },
        this._lookPicker(items, cur, (k) => {
          const b = BACKGROUNDS[k];
          const next = { ...o, theme: b.theme };
          if (b.theme === 8) next.scene = b.scene;
          else delete next.scene;
          this._setPage(p, i, { options: next });
        }),
        sw && !weather ? this._row("Cars", this._toggle(o.cars !== false, (v) => this._setPageOption(p, i, "cars", v), { label: "Cars" })) : null,
        sw && !weather && o.cars !== false ? this._row("Traffic", this._slider(Number.isInteger(o.traffic) ? o.traffic : 0, 0, 2, 1, (v) => this._setPageOption(p, i, "traffic", v), (v) => ["Light", "More", "Busy"][v], { label: "Traffic" }),
          { info: "How often a new car sets off." }) : null,
        // drawn, line and plain backgrounds: light or dark by the panel's theme, or always one of them
        (Number(o.theme) >= 3 && Number(o.theme) !== 8) ? this._row("Light or dark", this._seg([["panel", "Panel setting"], ["light", "Light"], ["dark", "Dark"]], ["light", "dark"].includes(o.light) ? o.light : "panel",
          (v) => this._setPageOption(p, i, "light", v === "panel" ? undefined : v), { label: "Light or dark" }),
          { info: "Panel setting: the light scene with the panel's light theme, the dark one with its dark theme, and Auto follows the panel. Light or Dark keeps this background so." }) : null,
        sw || plain ? null : this._more(`bg:${p.entry_id}:${i}`, weather ? null : this._row("Real sun and moon", this._toggle(o.real_sun !== false, (v) => this._setPageOption(p, i, "real_sun", v), { label: "Real sun and moon" }),
          { info: "On: the sun and the moon stand where they really are in the sky. Off: they keep a fixed place." }),
          // the panel's own switches (its entities): the same on every weather page of the panel
          BACKGROUNDS[cur].theme <= 2 ? this._row("Rain on the glass", this._toggle(ps.rain_on_glass !== false, (v) => this._command(p, "rain_on_glass", v), { label: "Rain on the glass" }),
            { info: "Drops on the glass while it rains, over the photo backgrounds only. This switch is the panel's: it applies to all its weather pages." }) : null,
          this._row("Lightning", this._toggle(ps.lightning !== false, (v) => this._command(p, "lightning", v), { label: "Lightning" }),
            { info: "Flashes and lightning bolts in thunderstorms. Turn them off if the flashes disturb anyone. This switch is the panel's: it applies to all its weather pages." })));
    }
    const looks = LOOKS[page.kind] || [];
    if (looks.length < 2) return null;
    const items = looks.map((name, v) => {
      const sup = this._lookSupported(p, page.kind, v);
      return { id: v, name, pic: this._lookPic(p, lookName(page.kind, v, page), () => lookArt(page.kind, v, page)), disabled: !sup.ok, why: sup.ok ? null : sup.why };
    });
    return this._card("Look", {}, this._lookPicker(items, page.variant || 0, (v) => this._setPage(p, i, { variant: v })));
  }

  // The plain background's colour on this panel: "dark" / "light" by its theme, null when automatic.
  _plainMode(p) {
    const t = String(p?.states?.theme || "").toLowerCase();
    return t === "dark" ? "dark" : t === "light" ? "light" : null;
  }

  // One entity of a page (or none = the panel's default).
  _pageEntityRow(p, i, label, domain, noneLabel) {
    const page = this._pages(p)[i];
    const cur = (page.entities || []).find((e) => e.startsWith(`${domain}.`)) || "";
    return this._row(label, this._entityButton(cur, { title: label, domains: [domain], none: noneLabel, p, onPick: (v) => {
      const fresh = this._pages(p)[i];
      const rest = (fresh.entities || []).filter((e) => !e.startsWith(`${domain}.`));
      this._setPage(p, i, { entities: v ? [v, ...rest] : rest });
    } }));
  }

  // An ordered list of entities of a page, with add (picker), move up and remove.
  _entityList(p, i, domains, { most = 8, addLabel = "Add", leadIcon = null } = {}) {
    const page = this._pages(p)[i];
    const ents = (page.entities || []).filter((e) => domains.includes(e.split(".")[0]));
    const others = (page.entities || []).filter((e) => !domains.includes(e.split(".")[0]));
    const save = (list) => this._setPage(p, i, { entities: [...list, ...others] });
    return [
      ents.length ? h("div", null, ents.map((id, k) => this._row(this._ename(id), h("div", { style: { display: "flex", gap: "2px" } },
        h("button", { class: "ib", "aria-label": "Move up", disabled: k === 0, onclick: () => { const n = [...ents]; n.splice(k - 1, 0, n.splice(k, 1)[0]); save(n); } }, icon("arrow-up")),
        h("button", { class: "ib", "aria-label": `Remove ${this._ename(id)}`, onclick: () => save(ents.filter((x) => x !== id)) }, icon("close"))),
      { lead: h("span", { class: "lead" }, icon(leadIcon || this._eicon(id))), keep: true, cls: k === 0 ? "first" : "" }))) : h("div", { class: "empty" }, "Nothing chosen yet."),
      ents.length < most ? h("div", { class: "below" }, this._btn(addLabel, () => this._openSheet({
        kind: "pick", title: addLabel, domains, exclude: ents, panel: p.entry_id,
        onPick: (id) => { const fresh = this._pages(p)[i]; const cur = (fresh.entities || []).filter((e) => domains.includes(e.split(".")[0])); save([...cur, id]); },
      }), { cls: "small", icon: "plus" })) : null,
    ];
  }

  // The Weather details page: the Custom page's widget editor, with the weather widgets only.
  _editWeather(p, page, i) {
    return this._editHome(p, page, i);
  }

  // The clock page: the same choices as the screensaver's clock (its face is the page's look).
  _editClock(p, page, i) {
    const o = { ...(page.options || {}), face: page.variant || 0 };
    const set = (patch) => {
      const pages = clone(this._pages(p));
      const { face, ...rest } = patch;
      if (face != null) pages[i].variant = face;
      pages[i].options = { ...(pages[i].options || {}), ...rest };
      this._savePages(p, pages);
    };
    return [this._card("Options", {}, this._saverClock(o, set, null))];
  }

  _editMusic(p, page, i) {
    const o = page.options || {};
    return [this._card("Options", {},
      this._pageEntityRow(p, i, "Player", "media_player", "The panel's player"),
      (page.variant || 0) === 2 ? this._row("Wood", null, { stack: true }) : null,
      (page.variant || 0) === 2 ? h("div", { class: "below" }, this._swatches(WOODS, o.wood || "walnut", (v) => this._setPageOption(p, i, "wood", v), { wood: true })) : null,
      [3, 4].includes(page.variant || 0) ? this._row("Cover behind the cassette", this._toggle(o.cover_bg === true, (v) => this._setPageOption(p, i, "cover_bg", v), { label: "Cover behind the cassette" }),
        { info: "On: the playing track's cover fills the screen behind the cassette. Off: the cassette's own backdrop." }) : null,
      ((page.variant || 0) === 0 || ([3, 4].includes(page.variant || 0) && o.cover_bg === true))
        ? this._row("Blurred cover", this._toggle(o.cover_blur !== false, (v) => this._setPageOption(p, i, "cover_blur", v), { label: "Blurred cover" }),
          { info: "On: the cover behind the player is blurred into soft colour. Off: the cover stays recognisable, darkened." }) : null)];
  }

  _editTimers(p, page, i) {
    const o = page.options || {};
    const fns = Array.isArray(o.functions) && o.functions.length ? o.functions : TIMER_FUNCTIONS.map(([k]) => k);
    return [this._card("Options", {},
      this._row("On the panel", null, { stack: true, info: "Which tabs the page has. At least one stays." }),
      TIMER_FUNCTIONS.map(([k, l]) => this._row(l, this._toggle(fns.includes(k), () => {
        const next = fns.includes(k) ? fns.filter((x) => x !== k) : [...fns, k];
        if (!next.length) { this._toast("At least one stays"); return; }
        this._setPageOption(p, i, "functions", TIMER_FUNCTIONS.map(([x]) => x).filter((x) => next.includes(x)));
      }, { label: l }))),
      (page.variant || 0) === 3 ? this._row("Scene", this._dropdown([[-1, "Random, new every time"], ...SW_SCENES.map((n, k) => [k, n])], Number.isInteger(o.scene) ? o.scene : 0, (v) => this._setPageOption(p, i, "scene", Number(v)), { label: "Scene" })) : null,
      this._soundRow("Timer sound", o.sound, { def: TIMER_LOOK_SOUND[page.variant || 0] || TIMER_LOOK_SOUND[0], defNote: "This look's own sound", title: "Timer sound", p },
        (v) => this._setPageOption(p, i, "sound", v), { info: "Each look has its own sound; another one, or your own, can be chosen." }),
      this._row("A colour for each timer", this._toggle(o.colourful !== false, (v) => this._setPageOption(p, i, "colourful", v), { label: "A colour for each timer" }), { sub: o.colourful === false ? "Every timer in the panel's colour" : null }),
      o.colourful !== false ? this._timerColours(p, i, o) : null),
    // the stopwatch's and the alarms' own looks, as the panel draws them
    fns.includes("stopwatch") ? this._card("Stopwatch look", {},
      this._lookPicker(SW_LOOKS.map(([k, n]) => ({ id: k, name: n, pic: this._lookPic(p, `sw-${k}`, () => icon("timer-outline")) })),
        Number.isInteger(o.sw_look) ? o.sw_look : 0, (v) => this._setPageOption(p, i, "sw_look", v))) : null,
    fns.includes("alarm") ? this._card("Alarm look", {},
      this._lookPicker(ALARM_LOOKS.map(([k, n]) => ({ id: k, name: n, pic: this._lookPic(p, `alarm-${k}`, () => icon("alarm")) })),
        Number.isInteger(o.alarm_look) ? o.alarm_look : 0, (v) => this._setPageOption(p, i, "alarm_look", v))) : null,
    fns.includes("alarm") ? this._alarmsCard(p, i) : null];
  }

  // Each timer's own colour: the 1st, 2nd … 6th timer running at once (then it starts again). Tap one, then a colour.
  _timerColours(p, i, o) {
    const cols = Array.from({ length: 6 }, (_, k) => (Array.isArray(o.colours) && o.colours[k]) || TIMER_TINTS[k]);
    const slot = this._ui.timerSlot ?? null;
    const pick = (c) => {
      const next = [...cols];
      next[slot] = c;
      this._setPageOption(p, i, "colours", next.every((x, k) => x === TIMER_TINTS[k]) ? undefined : next);
    };
    return h("div", null,
      this._row("Colours", h("div", { class: "swatches", role: "radiogroup", "aria-label": "Timer colours" }, cols.map((c, k) => h("button", {
        class: `sw tslot${slot === k ? " on" : ""}`, style: { background: c }, "aria-label": `Timer ${k + 1}`, title: `Timer ${k + 1}`,
        onclick: () => { this._ui.timerSlot = slot === k ? null : k; this._render(); },
      }, h("span", null, String(k + 1))))), { info: "The 1st, 2nd … 6th timer running at the same time each take their colour; the 7th starts again with the 1st." }),
      slot != null ? h("div", { class: "swatches below" }, TIMER_PALETTE.map((c) => h("button", {
        class: `sw${cols[slot].toLowerCase() === c.toLowerCase() ? " on" : ""}`, style: { background: c }, "aria-label": c, onclick: () => pick(c),
      }))) : null);
  }

  // -- alarms (they live on the panel; the page shows what it reports) ----------------------

  _hhmm(a) {
    return `${String(a.h).padStart(2, "0")}:${String(a.m).padStart(2, "0")}`;
  }

  _repeatText(days) {
    const d = Array.from({ length: 7 }, (_, k) => Boolean((days || [])[k]));
    const n = d.filter(Boolean).length;
    if (n === 7) return "Every day";
    if (n === 0) return "Once";
    if (d.every((x, k) => x === k < 5)) return "Weekdays";
    if (d.every((x, k) => x === k >= 5)) return "Weekends";
    return DAY_NAMES.filter((_, k) => d[k]).join(", ");
  }

  _alarmsCard(p, i) {
    const alarms = Array.isArray(p.alarms) ? p.alarms : null;
    const full = alarms && alarms.length >= ALARMS_MAX;
    return this._card("Alarms", {
      info: "Alarms live on the panel, so they ring even when Home Assistant is away. They can be set by voice too.",
      acts: alarms ? this._btn("Add an alarm", () => this._openSheet({ kind: "alarm", panel: p.entry_id, page: i, idx: -1, t: "07:00", days: [true, true, true, true, true, false, false], label: "", sound: "auto" }),
        { cls: "small", icon: "plus", disabled: full, title: full ? `At most ${ALARMS_MAX} alarms` : null }) : null,
    },
    alarms == null ? h("div", { class: "empty" }, "The panel shows its alarms here once it is connected.")
      : alarms.length ? h("div", { class: "alarms" }, alarms.map((a, k) => {
        const open = () => this._openSheet({ kind: "alarm", panel: p.entry_id, page: i, idx: k, t: this._hhmm(a), days: Array.from({ length: 7 }, (_, d) => Boolean((a.days || [])[d])), label: a.label || "", sound: a.tone || "auto" });
        return h("div", { class: `alarm${a.on ? "" : " off"}` },
          h("button", { class: "open", onclick: open, "aria-label": `${a.label || "Alarm"}, ${this._hhmm(a)}, ${this._repeatText(a.days)}. Edit` },
            h("span", { class: "tm" }, this._hhmm(a)),
            h("span", { class: "d" }, h("b", null, a.label || "Alarm"), `${this._repeatText(a.days)} · ${this._soundInfo(a.tone, "birds").name}`)),
          this._toggle(a.on, (v) => {
            const list = clone(p.alarms || []);
            list[k].on = v;
            this._saveAlarms(p, list, { toast: v ? `Alarm on for ${this._hhmm(a)}` : "Alarm off" });
          }, { label: `${a.label || "Alarm"} ${this._hhmm(a)}` }));
      }))
        : h("div", { class: "empty" }, "No alarms yet."),
    alarms && alarms.some((a) => String(a.tone || "").startsWith("ma:")) ? this._maPlayerRow(p) : null);
  }

  // Music alarms play through Music Assistant: it must know the panel's player. The integration adds it
  // there by itself when an alarm gets music; this row shows the state and can add it by hand.
  _maPlayerRow(p) {
    const st = (this._ui.maPlayer = this._ui.maPlayer || {});
    const cur = st[p.entry_id];
    if (cur === undefined) {
      st[p.entry_id] = null;
      this._hass.callWS({ type: "panelos/page/ma_player", entry_id: p.entry_id })
        .then((r) => { st[p.entry_id] = r; this._render(); }, (e) => { st[p.entry_id] = { error: e?.message || String(e) }; this._render(); });
    }
    const ask = (add) => {
      const byHand = Boolean(cur?.by_hand);
      st[p.entry_id] = null;
      this._render();
      this._hass.callWS({ type: "panelos/page/ma_player", entry_id: p.entry_id, add })
        .then((r) => {
          st[p.entry_id] = { ...r, by_hand: r.by_hand || (byHand && !r.added) };
          if (add && r.added) this._toast("Added to Music Assistant");
          this._render();
        }, (e) => { st[p.entry_id] = { error: e?.message || String(e) }; this._render(); });
    };
    const name = cur?.player_name || "the panel's player";
    const text = cur == null ? "Checking Music Assistant…"
      : cur.error ? cur.error
      : !cur.available ? "Music Assistant is not set up in Home Assistant: alarms with music ring their own sound."
      : cur.added ? "Music Assistant plays on this panel."
      : cur.sendspin ? "Music Assistant finds and pairs the panel by itself, a few seconds after the panel or Music Assistant starts."
      : cur.by_hand ? `Add it in Music Assistant: Settings → Providers → Home Assistant MediaPlayers, choose “${name}” among its players and save.`
      : "Music Assistant does not know this panel's player yet.";
    const acts = !cur || !cur.available || cur.added || cur.error ? null
      : cur.sendspin ? this._btn("Check again", () => ask(false), { cls: "small ghost" })
      : cur.by_hand ? h("div", { style: { display: "flex", gap: "8px", flexWrap: "nowrap", flex: "none" } },
        cur.ma_panel ? this._btn("Open Music Assistant", () => { window.history.pushState(null, "", `/${cur.ma_panel}`); window.dispatchEvent(new CustomEvent("location-changed")); }, { cls: "small primary" }) : null,
        this._btn("Check again", () => ask(false), { cls: "small ghost" }))
      : this._btn("Add to Music Assistant", () => ask(true), { cls: "small primary" });
    return h("div", { class: "group" }, this._row("Music for alarms", acts,
      { cls: "first", sub: text, stack: Boolean(cur?.by_hand), info: "Alarms with music ask Music Assistant to play on the panel. If it cannot, the alarm rings its own sound after 12 seconds." }));
  }

  _editThermostat(p, page, i) {
    return [this._card("Thermostats", { info: "The first one opens on the page; a swipe up shows every one of them as rooms." },
      this._entityList(p, i, ["climate"], { most: 8, addLabel: "Add a thermostat" }))];
  }

  _editQuick(p, page, i) {
    const ents = page.entities || [];
    const o = page.options || {};
    const tapes = Array.isArray(o.tapes) ? o.tapes : [];
    // the mixing desk's tapes and the flight deck's plates both show a short label
    const desk = [1, 2].includes(page.variant || 0);
    // the flight deck's painted group frames: automatic (by kind), off (one frame with the room) or own groups
    const deck = (page.variant || 0) === 2;
    const groups = ["off", "custom"].includes(o.groups) ? o.groups : "auto";
    const starts = Array.isArray(o.group_starts) ? o.group_starts : [];
    const gnames = Array.isArray(o.group_names) ? o.group_names : [];
    const cols = (page.variant || 0) === 0 ? 2 : 4;
    const slots = [];
    const max = TILES_MAX;
    for (let k = 0; k < Math.min(max, ents.length + 1); k++) {
      const id = ents[k];
      const gname = deck && groups === "custom" && k < 8 && (k === 0 || starts[k]) ? (gnames[k] || "").trim() : "";
      const open = () => { if (Date.now() - (this._dragEnded || 0) > 300) this._openSheet({ kind: "tile", panel: p.entry_id, page: i, slot: k }); };
      slots.push(h("button", { class: `tslot${id ? " full" : " add"}`, "data-i": k, onclick: open },
        id ? [h("span", { class: "handle", "aria-label": `Drag to reorder ${this._ename(id)}`, onpointerdown: (e) => this._startTileDrag(e, p, i) }, icon("drag")),
          icon(this._eicon(id)), h("span", { class: "t" }, h("b", null, this._ename(id)), desk && tapes[k] ? h("span", { class: "tape" }, tapes[k]) : null,
            gname ? h("small", null, `Group: ${gname}`) : null)] : [icon("plus"), "Add"]));
    }
    return [this._card("Tiles", {
      info: `Tap a tile to choose what it controls${desk ? " and the text on its label" : ""}; drag it by its handle to move it. A page takes ${TILES_MAX} tiles: more go on another Quick controls page. A long press on the panel opens the details (brightness, colour, position).`,
      acts: this._btn("Fill from the room", () => this._fillTiles(p, i), { cls: "small", icon: "home-outline", disabled: !this._pareaId(p) || ents.length >= max, title: this._pareaId(p) ? null : "The panel has no area in Home Assistant" }),
    }, h("div", { class: "tilegrid", style: { gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` } }, slots)),
    deck ? this._card("Groups", {
      info: "Painted frames over the switches. Automatic: neighbouring controls of one kind share a frame (lights, shutters, climate …). Off: one frame over the top row with the panel's room. Custom: tap a tile to start a group there and name it; a group without a name has no frame.",
    }, this._row("Groups", this._seg([["auto", "Automatic"], ["off", "Off"], ["custom", "Custom"]], groups, (v) => this._setPageOption(p, i, "groups", v === "auto" ? undefined : v), { label: "Groups" }), { cls: "first" })) : null].filter(Boolean);
  }

  // A tile dragged by its handle to another place: the tiles and what each keeps (its label, its group) in the new order.
  _startTileDrag(e, p, i) {
    this._startDrag(e, ".tslot", { grid: true, fixed: (el) => el.classList.contains("add") }, (all) => {
      const page = this._pages(p)[i];
      const ents = [...(page.entities || [])];
      // (the "Add" slot stays last)
      const order = all.filter((k) => k < ents.length);
      const align = (list, empty) => {
        const out = Array.isArray(list) ? [...list] : [];
        while (out.length < ents.length) out.push(empty);
        out.length = ents.length;
        return out;
      };
      const options = { ...(page.options || {}) };
      for (const [key, empty] of [["tapes", ""], ["group_starts", false], ["group_names", ""]]) {
        const list = align(page.options?.[key], empty);
        const next = order.map((k) => list[k]);
        if (next.some(Boolean)) options[key] = next;
        else delete options[key];
      }
      this._setPage(p, i, { entities: order.map((k) => ents[k]), options }, { toast: `New order sent to ${this._pname(p)}` });
    });
  }

  _fillTiles(p, i) {
    const area = this._pareaId(p);
    const page = this._pages(p)[i];
    const ents = [...(page.entities || [])];
    const cands = this._ents(["light", "cover", "climate", "switch", "fan", "scene"], { p }).filter((e) => e.area === area && !ents.includes(e.id));
    const room = Math.max(0, TILES_MAX - ents.length);
    const add = cands.slice(0, room).map((e) => e.id);
    if (!add.length) { this._toast(`Nothing more to add from ${this._areaName(area)}`); return; }
    this._setPage(p, i, { entities: [...ents, ...add] }, { toast: `${plural(add.length, "tile", "tiles")} added from ${this._areaName(area)}` });
  }

  _editCamera(p, page, i) {
    const o = page.options || {};
    return [
      this._card("Cameras", { info: "The order here is the order on the panel." }, this._entityList(p, i, ["camera"], { most: 8, addLabel: "Add a camera" })),
      this._card("Picture", {},
        this._row("Opens on", this._seg([["one", "One camera"], ["all", "All cameras"]], o.view === "all" ? "all" : "one", (v) => this._setPageOption(p, i, "view", v), { label: "Opens on" })),
        this._row("Fit", this._seg([["whole", "Whole picture"], ["fill", "Fill the screen"]], o.fit === "whole" ? "whole" : "fill", (v) => this._setPageOption(p, i, "fit", v), { label: "Fit" }))),
      this._card("Visitor alert", { info: "When this sensor turns on, the first camera's picture opens over any page and the panel rings (also in do not disturb, if the doorbell is allowed to come through there). With a door chosen, holding the button on the picture opens it. With a speaker at the door, the microphone on the picture offers the replies; a tap on one says it on that speaker." },
        this._row("Opens when", this._entityButton(o.ring || "", { title: "Opens when", domains: ["binary_sensor"], none: "Never", p, onPick: (v) => this._setPageOption(p, i, "ring", v || undefined) })),
        this._row("Door to open", this._entityButton(o.lock || "", { title: "Door to open", domains: ["lock", "cover"], none: "None", p, onPick: (v) => this._setPageOption(p, i, "lock", v || undefined) })),
        ...this._doorReplies(p, page, i)),
    ];
  }

  // The replies at the door (spec 38.1): the speaker (at first the media player on a camera's own device), up to four
  // replies, each one line on the panel, and the language they are spoken in.
  _doorReplies(p, page, i) {
    const o = page.options || {};
    const speaker = typeof o.speaker === "string" ? o.speaker : this._doorSpeaker(page.entities || []);
    const rows = [this._row("Speaker at the door", this._entityButton(speaker, { title: "Speaker at the door", domains: ["media_player"], none: "None", p, onPick: (v) => this._setPageOption(p, i, "speaker", v || "") }))];
    if (!speaker) return rows;
    const list = Array.isArray(o.replies) ? o.replies.filter((t) => typeof t === "string").slice(0, DOOR_REPLIES_MAX) : [...DOOR_REPLIES];
    const save = (next) => this._setPageOption(p, i, "replies", next);
    if (!this._doorFont && document.fonts) this._doorFont = document.fonts.load(DOOR_FONT).then(() => this._render(), () => {});
    const field = (text, k) => {
      const note = h("small", { class: "rlong", hidden: this._doorFits(text) }, "Too long for one line on the panel");
      const input = h("input", {
        class: "inp", type: "text", value: text, maxlength: 100, placeholder: "Reply", "aria-label": `Reply ${k + 1}`,
        oninput: (e) => { note.hidden = this._doorFits(e.target.value); },
        onchange: (e) => { const next = [...list]; next[k] = e.target.value.trim(); save(next); },
        onkeydown: (e) => { if (e.key === "Enter") e.target.blur(); },
      });
      return h("div", { class: "rfield" },
        h("div", { class: "rline" }, input, h("button", { class: "ib", "aria-label": `Remove reply ${k + 1}`, onclick: () => save(list.filter((_, j) => j !== k)) }, icon("close"))),
        note);
    };
    rows.push(this._row("Replies", h("div", { class: "replies" }, list.map(field),
      list.length < DOOR_REPLIES_MAX ? this._btn("Add a reply", () => {
        save([...list, ""]);
        setTimeout(() => this.shadowRoot?.querySelector(".replies .rfield:last-of-type input")?.focus(), 80);
      }, { cls: "small", icon: "plus" }) : null), { stack: true }));
    const engine = p.settings?.tts?.engine || "";
    rows.push(this._row("Language", this._dropdown([["", "Same as Home Assistant"], ...this._ttsLanguages(engine)], o.language || "",
      (v) => this._setPageOption(p, i, "language", v || undefined), { label: "Language", disabled: !engine }),
    { sub: engine ? null : "Home Assistant has no text-to-speech" }));
    return rows;
  }

  // the media player on the first camera's device that has one ("" for none), as the integration picks it
  _doorSpeaker(cameras) {
    const reg = Object.values(this._entityReg);
    for (const cam of cameras) {
      const dev = this._entityReg[cam]?.device_id;
      const hit = dev && reg.find((e) => e.device_id === dev && e.entity_id.startsWith("media_player.") && !e.disabled_by);
      if (hit) return hit.entity_id;
    }
    return "";
  }

  // a reply fits one line on the panel: the panel's font and the width its line has there
  _doorFits(text) {
    const c = this._measure || (this._measure = document.createElement("canvas").getContext("2d"));
    c.font = DOOR_FONT;
    return c.measureText(String(text || "").trim()).width <= DOOR_LINE_PX;
  }

  // the languages of a text-to-speech engine, by name ([code, name]); fetched once per engine
  _ttsLanguages(engine) {
    if (!engine) return [];
    this._tts = this._tts || {};
    if (!this._tts[engine]) {
      this._tts[engine] = { list: [] };
      this._hass.callWS({ type: "tts/engine/get", engine_id: engine }).then((r) => {
        const codes = Array.isArray(r?.provider?.supported_languages) ? r.provider.supported_languages : [];
        let names = null;
        try { names = new Intl.DisplayNames([this._hass.locale?.language || this._hass.language || "en"], { type: "language" }); } catch (e) { names = null; }
        const named = codes.map((c) => { let n = c; try { n = (names && names.of(String(c).replace("_", "-"))) || c; } catch (e) { n = c; } return [c, n]; });
        const seen = {};
        for (const [, n] of named) seen[n] = (seen[n] || 0) + 1;
        this._tts[engine].list = named.map(([c, n]) => [c, seen[n] > 1 ? `${n} (${c})` : n]).sort((a, b) => a[1].localeCompare(b[1]));
        this._render();
      }, () => {});
    }
    return this._tts[engine].list;
  }

  _editCalendar(p, page, i) {
    const o = page.options || {};
    const ents = page.entities || [];
    const all = this._ents(["calendar"], { p });
    const wall = Array.isArray(o.wall_calendars) ? o.wall_calendars : ents;
    const hidden = Array.isArray(o.hidden_calendars) ? o.hidden_calendars.filter((x) => ents.includes(x)) : [];
    return [this._card("Calendars", { info: "The calendars the page shows." },
      all.length ? this._multi({ label: "Calendars", key: `cals:${p.entry_id}:${i}` }, all.map((e) => this._mi(ents.includes(e.id), e.name, () => {
        const next = ents.includes(e.id) ? ents.filter((x) => x !== e.id) : [...ents, e.id];
        if (next.length > 8) { this._toast("At most 8 calendars"); return; }
        const opts = { ...o };
        if (Array.isArray(opts.wall_calendars)) opts.wall_calendars = opts.wall_calendars.filter((x) => next.includes(x));
        if (Array.isArray(opts.hidden_calendars)) opts.hidden_calendars = opts.hidden_calendars.filter((x) => next.includes(x));
        this._setPage(p, i, { entities: next, options: opts });
      }))) : h("div", { class: "empty" }, "No calendars in Home Assistant."),
      // shown on the panel: also chosen with the panel's chip; at least one stays on
      ents.length > 1 ? this._row("Shown on the panel", null, { stack: true }) : null,
      ents.length > 1 ? this._multi({ label: "Shown on the panel", key: `shown:${p.entry_id}:${i}` }, ents.map((id) => this._mi(!hidden.includes(id), this._ename(id), () => {
        const next = hidden.includes(id) ? hidden.filter((x) => x !== id) : [...hidden, id];
        if (next.length >= ents.length) { this._toast("At least one calendar stays on the panel"); return; }
        this._setPageOption(p, i, "hidden_calendars", next.length ? next : undefined);
      }))) : null,
      (page.variant || 0) === 3 && ents.length ? this._row("Written on the wall calendar", null, { stack: true, info: "Calendars with many daily entries (routines) would fill every day; leave them off here." }) : null,
      (page.variant || 0) === 3 && ents.length ? this._multi({ label: "Written on the wall calendar", key: `wall:${p.entry_id}:${i}` }, ents.map((id) => this._mi(wall.includes(id), this._ename(id), () => {
        const next = wall.includes(id) ? wall.filter((x) => x !== id) : [...wall, id];
        this._setPageOption(p, i, "wall_calendars", ents.filter((x) => next.includes(x)));
      }))) : null)];
  }

  _editTodo(p, page, i) {
    return [this._card("Lists", { info: "Only the lists chosen here appear on the panel; the first one opens." }, this._entityList(p, i, ["todo"], { most: 8, addLabel: "Add a list" }))];
  }

  _editInfo(p, page, i) {
    const o = page.options || {};
    if ((page.variant || 0) !== 2) return [];
    return [this._card("Options", {}, this._row("Finish", null, { stack: true }),
      h("div", { class: "below" }, this._swatches(PAGER_FINISH, o.finish || "graphite", (v) => this._setPageOption(p, i, "finish", v))))];
  }

  // -- weather page: layout editor ------------------------------------------------------

  _items(page) {
    return clone(Array.isArray(page.options?.items) ? page.options.items : page.kind === "weather" ? WEATHER_DEFAULT_ITEMS : DEFAULT_ITEMS);
  }

  _saveItems(p, i, items, opts) {
    const page = this._pages(p)[i];
    const clean = items.map((w) => ({
      kind: w.kind, col: w.col, row: w.row, w: w.w, h: w.h,
      style: w.kind === 0 || w.kind === 7 ? w.style || 0 : 0,
      accent: w.accent || "", weight: w.weight || 600, tap: w.tap !== false, opts: w.opts || 0,
      ...(Number.isInteger(w.bg) && w.bg >= 0 ? { bg: w.bg } : {}),
      ...(typeof w.neon === "boolean" ? { neon: w.neon } : {}),
      ...(w.kind === 8 && w.entity ? { entity: w.entity } : {}),
      ...(w.kind === 10 && Array.isArray(w.people) && w.people.length ? { people: w.people } : {}),
    }));
    return this._setPage(p, i, { options: { ...(page.options || {}), items: clean } }, opts);
  }

  _fits(items, self, x, y, w, hh) {
    if (x < 0 || y < 0 || x + w > 4 || y + hh > 4) return false;
    return !this._overlaps(items, self, x, y, w, hh).length;
  }

  // Inside the 4 x 4 grid (widgets may lie on top of each other).
  _inGrid(x, y, w, hh) {
    return x >= 0 && y >= 0 && x + w <= 4 && y + hh <= 4;
  }

  // The other widgets a place covers (indices).
  _overlaps(items, self, x, y, w, hh) {
    return items.map((o, k) => (k !== self && x < o.col + o.w && o.col < x + w && y < o.row + o.h && o.row < y + hh ? k : -1)).filter((k) => k >= 0);
  }

  _freeSpot(items, w, hh) {
    for (let y = 0; y <= 4 - hh; y++) for (let x = 0; x <= 4 - w; x++) if (this._fits(items, -1, x, y, w, hh)) return { x, y };
    return null;
  }

  // Forward / Back (the later widget lies on top): the place in the list just above the next widget it lies on, or
  // just below the one before; -1 when there is none that way.
  _layerTo(items, k, dir) {
    const w = items[k];
    const over = this._overlaps(items, k, w.col, w.row, w.w, w.h);
    if (dir > 0) { const up = over.filter((o) => o > k); return up.length ? Math.min(...up) : -1; }
    const down = over.filter((o) => o < k);
    return down.length ? Math.max(...down) : -1;
  }

  _layer(p, i, k, dir) {
    const items = this._items(this._pages(p)[i]);
    const to = this._layerTo(items, k, dir);
    if (to < 0) return;
    const [w] = items.splice(k, 1);
    items.splice(to, 0, w);
    this._ui.sel = to;
    this._saveItems(p, i, items);
  }

  _contentWidth() {
    const W = this.clientWidth || window.innerWidth;
    const size = this.getAttribute("size");
    if (size === "wide") return Math.min(820, W - 248 - (this.hasAttribute("pvbig") ? 566 : 392)) - 68;
    if (size === "medium") return Math.min(820, W - 336) - 48;
    return W - 32;
  }

  _editHome(p, page, i) {
    const items = this._items(page);
    const size = Math.max(220, Math.min(400, this._contentWidth() - 46));
    // the canvas is the panel's whole screen (480 × 480); the widget grid sits in it as on the panel:
    // from (16, 44), cells 112 × 98
    const g = { gx: (16 * size) / 480, gy: (44 * size) / 480, cw: (112 * size) / 480, ch: (98 * size) / 480 };
    const sel = this._ui.sel;
    const weather = page.kind === "weather";
    const bg = this._bgOf(p, page);
    const lightGround = BACKGROUNDS[bg].theme === PLAIN_THEME && this._plainMode(p) === "light";
    // the panel shows this page while its settings are open: the live picture is the background, so
    // what lies where (and on what) is seen as it really is
    const live = this._livePage(p, i) && this._picOfLivePage(p);
    const canvas = h("div", { class: `hcanvas${lightGround ? " light" : ""}${live ? " live" : ""}`, style: { width: `${size}px`, height: `${size}px` } },
      live ? h("img", { class: "art", src: this._pics[p.entry_id].url, "data-pic": p.entry_id, alt: "", draggable: "false" }) : svgNode(bgArt(bg, this._plainMode(p))));
    for (let k = 0; k <= 4; k++) {
      canvas.append(h("i", { class: "gridline", style: { left: `${g.gx + k * g.cw}px`, top: `${g.gy}px`, height: `${4 * g.ch}px`, borderLeftWidth: "1px" } }));
      canvas.append(h("i", { class: "gridline", style: { top: `${g.gy + k * g.ch}px`, left: `${g.gx}px`, width: `${4 * g.cw}px`, borderTopWidth: "1px" } }));
    }
    items.forEach((w, k) => {
      const meta = WIDGETS[w.kind] || WIDGETS[0];
      const label = w.kind === 0 ? `${meta.name} · ${CLOCK_STYLES[w.style || 0] || CLOCK_STYLES[0]}` : meta.name;
      const box = h("div", {
        class: `wbox${k === sel ? " sel" : ""}${this._overlaps(items, k, w.col, w.row, w.w, w.h).length ? " over" : ""}`, role: "button", tabindex: "0", "aria-label": label,
        style: { left: `${g.gx + w.col * g.cw + 3}px`, top: `${g.gy + w.row * g.ch + 3}px`, width: `${w.w * g.cw - 6}px`, height: `${w.h * g.ch - 6}px` },
        onpointerdown: (e) => this._startWidgetDrag(e, p, i, k, g, "move"),
        onkeydown: (e) => { if (e.key === "Enter") this._openSheet({ kind: "widget", panel: p.entry_id, page: i, item: k }); },
      }, icon(meta.icon), w.h * g.ch > 56 || w.w > 1 ? h("span", null, label) : null,
      h("div", { class: "rs", onpointerdown: (e) => { e.stopPropagation(); this._startWidgetDrag(e, p, i, k, g, "size"); } }));
      canvas.append(box);
    });
    canvas.addEventListener("pointerdown", (e) => { if (e.target === canvas || e.target.closest(".art")) { if (this._ui.sel != null) { this._ui.sel = null; this._render(); } } });
    const sw = sel != null ? items[sel] : null;
    const hasKind = (kind) => items.some((w) => w.kind === kind);
    const ents = page.entities || [];
    const cals = this._ents(["calendar"], { p });
    return [
      this._card("Layout", {
        info: "Drag a widget to move it and its corner to resize it; tap it for its settings. Widgets snap to the 4 × 4 grid and may lie on top of each other (the later one on top): check those on the panel. The panel shows the new layout at once.",
        acts: this._btn("Add a widget", () => this._openSheet({ kind: "addWidget", panel: p.entry_id, page: i }), { cls: "small", icon: "plus" }),
      },
      h("div", { class: "wbar" }, sw
        ? [h("span", { class: "tag sig" }, (WIDGETS[sw.kind] || WIDGETS[0]).name),
          this._btn("Settings", () => this._openSheet({ kind: "widget", panel: p.entry_id, page: i, item: sel }), { cls: "small", icon: "tune-variant" }),
          this._overlaps(items, sel, sw.col, sw.row, sw.w, sw.h).length ? [
            this._btn("Back", () => this._layer(p, i, sel, -1), { cls: "small ghost", icon: "arrange-send-backward", disabled: this._layerTo(items, sel, -1) < 0 }),
            this._btn("Forward", () => this._layer(p, i, sel, 1), { cls: "small ghost", icon: "arrange-bring-forward", disabled: this._layerTo(items, sel, 1) < 0 })] : null,
          this._btn("Remove", () => this._removeWidget(p, i, sel), { cls: "small ghost", icon: "delete-outline" })]
        : h("span", { class: "note" }, `${plural(items.length, "widget", "widgets")} · grid 4 × 4`)),
      h("div", { class: "editor-wrap" }, canvas),
      this._overlapNote(items),
      Array.isArray(page.options?.items) ? null : h("p", { class: "note", style: { margin: "0 0 14px" } }, "The panel's own layout. Any change here saves it for this page."),
      this._row("Allow editing on the panel", this._toggle(Boolean(this._settings(p).home_edit), (v) => this._options(p, { home_edit: v }, { optimistic: (s) => { s.home_edit = v; } }), { label: "Allow editing on the panel" }),
        { info: "Long press on the panel, then hold the lock." })),
      this._card("Data", { info: weather ? "Where the widgets of this page get the weather. Without a choice here they use the panel's weather."
        : "Where the widgets of this page get their data. Without a choice here they use the panel's weather and player." },
        this._pageEntityRow(p, i, "Weather from", "weather", "The panel's weather"),
        !weather && hasKind(7) ? this._pageEntityRow(p, i, "Player", "media_player", "The panel's player") : null,
        !weather && hasKind(3) ? this._row("Calendars for the next event", null, { stack: true }) : null,
        !weather && hasKind(3) ? (cals.length ? this._multi({ label: "Calendars", key: `hcals:${p.entry_id}:${i}` }, cals.map((e) => this._mi(ents.includes(e.id), e.name, () => {
          const next = ents.includes(e.id) ? ents.filter((x) => x !== e.id) : [...ents, e.id];
          this._setPage(p, i, { entities: next.slice(0, 16) });
        }))) : h("div", { class: "empty" }, "No calendars in Home Assistant.")) : null),
    ];
  }

  // Widgets that lie on each other: a warning to look at them on the panel.
  _overlapNote(items) {
    const pairs = [];
    items.forEach((w, k) => this._overlaps(items, k, w.col, w.row, w.w, w.h).filter((o) => o > k)
      .forEach((o) => pairs.push(`${(WIDGETS[w.kind] || WIDGETS[0]).name} and ${(WIDGETS[items[o].kind] || WIDGETS[0]).name}`)));
    if (!pairs.length) return null;
    return h("div", { class: "warnline", role: "note" }, icon("alert-outline"),
      h("span", null, `${pairs.join(", ")} lie on each other. Check on the panel that both read well.`),
      h("span", { class: "rinfo" }, this._infoBtn("overlap", "The widget added later lies on top. Select one and use Forward or Back to change that.")));
  }

  _removeWidget(p, i, k) {
    const items = this._items(this._pages(p)[i]);
    const [gone] = items.splice(k, 1);
    this._ui.sel = null;
    this._saveItems(p, i, items, {
      toast: `${(WIDGETS[gone.kind] || WIDGETS[0]).name} removed`,
      undo: () => { const back = this._items(this._pages(p)[i]); if (this._fits(back, -1, gone.col, gone.row, gone.w, gone.h)) { back.splice(k, 0, gone); this._saveItems(p, i, back); } },
    });
  }

  _startWidgetDrag(e, p, i, k, g, mode) {
    e.preventDefault();
    const items = this._items(this._pages(p)[i]);
    const wd = items[k];
    const box = e.currentTarget.closest(".wbox");
    const sx = e.clientX;
    const sy = e.clientY;
    let nx = wd.col;
    let ny = wd.row;
    let nw = wd.w;
    let nh = wd.h;
    let moved = false;
    const min = widgetMin(wd);
    let tooSmall = false;
    this._dragging = true;
    const place = (ok) => {
      box.classList.toggle("bad", !ok);
      Object.assign(box.style, { left: `${g.gx + nx * g.cw + 3}px`, top: `${g.gy + ny * g.ch + 3}px`, width: `${nw * g.cw - 6}px`, height: `${nh * g.ch - 6}px` });
    };
    const move = (ev) => {
      const dx = ev.clientX - sx;
      const dy = ev.clientY - sy;
      if (Math.abs(dx) + Math.abs(dy) > 4) moved = true;
      if (mode === "move") { nx = Math.round(wd.col + dx / g.cw); ny = Math.round(wd.row + dy / g.ch); }
      else {
        const rw = Math.round(wd.w + dx / g.cw);
        const rh = Math.round(wd.h + dy / g.ch);
        if (rw < min[0] || rh < min[1]) tooSmall = true;
        nw = Math.max(min[0], rw);
        nh = Math.max(min[1], rh);
      }
      nx = Math.max(0, Math.min(4 - nw, nx));
      ny = Math.max(0, Math.min(4 - nh, ny));
      nw = Math.min(nw, 4 - nx);
      nh = Math.min(nh, 4 - ny);
      place(this._inGrid(nx, ny, nw, nh));
      box.classList.toggle("over", this._overlaps(items, k, nx, ny, nw, nh).length > 0);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      this._dragging = false;
      this._ui.sel = k;
      if (!moved) { this._render(); return; }
      if (tooSmall) {
        const name = wd.kind === 0 ? `The ${CLOCK_STYLES[wd.style || 0] || "clock"} clock` : (WIDGETS[wd.kind] || WIDGETS[0]).name;
        this._toast(`${name} needs at least ${min[0]} × ${min[1]} cells`);
      }
      if (this._inGrid(nx, ny, nw, nh) && (nx !== wd.col || ny !== wd.row || nw !== wd.w || nh !== wd.h)) {
        const over = this._overlaps(items, k, nx, ny, nw, nh);
        Object.assign(wd, { col: nx, row: ny, w: nw, h: nh });
        this._saveItems(p, i, items, over.length ? { toast: `It lies on ${over.map((o) => (WIDGETS[items[o].kind] || WIDGETS[0]).name).join(" and ")}: check on the panel that both read well` } : {});
      } else {
        this._render();
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  }

  // -- alerts -----------------------------------------------------------------------

  // "Automation" trigger: the screen's name in automations (editable, readable) and the two actions to use,
  // each with a copy button.
  _statusActions(r, set) {
    const name = r.alias || r.id;
    const show = `action: panelos.show_status\ndata:\n  screen: ${name}`;
    const clear = `action: panelos.clear_status\ndata:\n  screen: ${name}`;
    const copy = (text) => this._btn("Copy", () => {
      navigator.clipboard?.writeText(text).then(() => this._toast("Copied"), () => this._toast("Select the text and copy it", null, true));
    }, { cls: "small ghost", icon: "content-copy" });
    const taken = new Set(((this._data.house || {}).status || []).filter((x) => x.id !== r.id).flatMap((x) => [x.alias, x.id]).filter(Boolean));
    return h("div", { class: "group" },
      this._row("Name in automations", this._text(r.alias || "", (v) => {
        const slug = v.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "").replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "").slice(0, 40);
        if (slug && taken.has(slug)) { this._toast(`Another screen is already called ${slug}`, null, true); return; }
        set({ alias: slug });
      }, { max: 40, label: "Name in automations", cls: "short", placeholder: r.id }),
        { cls: "first", info: "Your automations turn this screen on and off by this name. Letters, numbers and _ only." }),
      this._row("Show it", copy(show), { sub: show.replace(/\n/g, "  ").replace(/ {2,}/g, " ") , stack: false }),
      this._row("Close it", copy(clear), { sub: clear.replace(/\n/g, "  ").replace(/ {2,}/g, " ") }),
      h("p", { class: "note", style: { margin: "6px 0 0" } }, "Add these actions to an automation or a script (Settings → Automations): the first shows this screen on its panels, the second closes it."));
  }

  _statusName(x) {
    return (x.own || x.cat === "custom") && x.headline ? x.headline : (STATUS_CAT[x.cat] || STATUS_CAT.custom).name;
  }

  _stLead(x) {
    const cat = STATUS_CAT[x.cat] || STATUS_CAT.custom;
    const picto = x.picto || cat.pictos[0];
    return h("span", { class: "stlead", style: { background: x.colour || cat.c } },
      picto.startsWith("mdi:") ? icon(picto) : h("img", { src: `${this._static}/picto/${picto}-144.png`, alt: "" }));
  }

  _trigText(x) {
    const t = x.trigger || {};
    if (t.kind === "after") {
      const o = (this._data.house.status || []).find((s) => s.id === t.after);
      return `After “${o ? this._statusName(o) : "another screen"}” ends · ${x.countdown || 10} min`;
    }
    if (t.kind === "service") return `From an automation · ID ${x.id}`;
    if (!t.entity) return "No device chosen yet";
    const ops = { on: "is on", off: "is off", above: `above ${t.value}`, below: `below ${t.value}`, equals: `is ${t.value}` };
    return `${this._ename(t.entity)} ${ops[t.op] || ""}${x.countdown ? ` · closes after ${x.countdown} min` : ""}`;
  }

  _safetyClass(id) {
    return this._hass?.states?.[id]?.attributes?.device_class || "";
  }

  _suggestions() {
    const house = this._data.house || {};
    const listed = new Set((house.critical || []).map((c) => c.entity_id));
    const dismissed = new Set(house.dismissed || []);
    return this._ents(["binary_sensor"], { filter: (st) => Boolean(SAFETY[st.attributes?.device_class]) }).filter((e) => !listed.has(e.id) && !dismissed.has(e.id));
  }

  _viewAlerts(p) {
    const house = this._data.house || {};
    const active = house.active || { critical: [], status: [] };
    const sugg = this._suggestions();
    const crit = (house.critical || []).map((c) => {
      const cls = SAFETY[this._safetyClass(c.entity_id)];
      const hidden = (c.off || []).includes(p.entry_id);
      const on = active.critical.includes(c.entity_id);
      const sub = [cls ? cls.name : "Sensor", this._areaName(this._earea(c.entity_id)), c.sound === false ? "silent" : null, hidden ? `not on ${this._pname(p)}` : null, on ? "on now" : null].filter(Boolean).join(" · ");
      return this._row(this._ename(c.entity_id), this._toggle(c.on, (v) => this._saveHouse((hs) => { hs.critical.find((x) => x.entity_id === c.entity_id).on = v; }), { label: "Counts as critical" }),
        { lead: h("span", { class: `lead${on ? " on" : ""}` }, icon(this._eicon(c.entity_id))), sub, click: () => this._openSheet({ kind: "crit", id: c.entity_id, panel: p.entry_id }), keep: true });
    });
    const own = (house.own_critical || []).map((c) => {
      const hidden = (c.off || []).includes(p.entry_id);
      const on = active.critical.includes(`own:${c.id}`);
      const sub = [`Own · ${this._ownCritWhen(c)}`, c.sound === false ? "silent" : null, hidden ? `not on ${this._pname(p)}` : null, on ? "on now" : null].filter(Boolean).join(" · ");
      return this._row(this._ownCritName(c), this._toggle(c.on, (v) => this._saveHouse((hs) => { hs.own_critical.find((x) => x.id === c.id).on = v; }), { label: "Counts as critical" }),
        { lead: h("span", { class: `lead${on ? " on" : ""}` }, icon("alert-octagon-outline")), sub, click: () => this._openSheet({ kind: "ownCrit", id: c.id, panel: p.entry_id }), keep: true });
    });
    const rules = (house.status || []).map((x) => {
      const here = x.panels.includes(p.entry_id);
      return this._row(this._statusName(x), this._toggle(here, (v) => this._saveHouse((hs) => {
        const r = hs.status.find((s) => s.id === x.id);
        r.panels = v ? [...new Set([...r.panels, p.entry_id])] : r.panels.filter((e) => e !== p.entry_id);
      }, { toast: `${v ? "Shown on" : "No longer on"} ${this._pname(p)}` }), { label: "On this panel" }), {
        lead: this._stLead(x),
        sub: [this._trigText(x), active.status.includes(x.id) ? "on now" : null, x.on ? null : "switched off",
          this._panels.length > 1 ? `On: ${this._panels.filter((q) => x.panels.includes(q.entry_id)).map((q) => this._pname(q)).join(" · ") || "none"}` : null].filter(Boolean).join(" · "),
        click: () => this._openSheet({ kind: "status", id: x.id, panel: p.entry_id, pv: p.entry_id }), dim: !here, keep: true,
      });
    });
    const dnd = { ...DEFAULT_DND, ...(this._settings(p).dnd || {}) };
    dnd.allow = { ...DND_DEFAULT_ALLOW, ...(dnd.allow || {}) };
    const changed = Object.keys(DND_DEFAULT_ALLOW).some((k) => dnd.allow[k] !== DND_DEFAULT_ALLOW[k]);
    const setDnd = (patch) => this._options(p, { dnd: patch }, { optimistic: (s) => { s.dnd = { ...dnd, ...patch, allow: { ...dnd.allow, ...(patch.allow || {}) } }; } });
    const pops = { ...DEFAULT_POPUPS, ...(this._settings(p).popups || {}) };
    const setPops = (patch) => this._options(p, { popups: patch }, { optimistic: (s) => { s.popups = { ...pops, ...patch }; } });
    const firstSensor = (house.critical || []).find((c) => c.on && !(c.off || []).includes(p.entry_id));
    const firstOwn = (house.own_critical || []).find((c) => c.on && !(c.off || []).includes(p.entry_id));
    const first = firstSensor ? firstSensor.entity_id : firstOwn ? `own:${firstOwn.id}` : null;
    return [
      this._hero("Alerts and status", [h("span", null, this._pname(p)), this._syncLine(p)]),
      this._card("Critical alerts", {
        sub: "Shared by every panel",
        info: "Critical alerts take over the screen of every panel they are shown on, with sound, even in do not disturb, until they end. Sensors are suggested by their device class (smoke, gas, water leak…); with Add, any device or an automation can start one. Only the ones in this list count.",
        acts: [sugg.length ? this._btn(`${sugg.length} suggested`, () => this._openSheet({ kind: "critSuggest", panel: p.entry_id }), { cls: "small", icon: "shield-alert-outline" }) : null,
          this._btn("Add", () => this._newOwnCrit(p), { cls: "small", icon: "plus" })],
      }, crit.length || own.length ? [...crit, ...own] : h("div", { class: "empty" }, "No critical alerts yet."),
      h("div", { class: "below", style: { display: "flex", gap: "8px", flexWrap: "wrap", paddingTop: "6px" } },
        this._btn("Test on this panel", () => this._test(p, "critical", first), { cls: "small", icon: "alert-octagon-outline", disabled: !first || !p.connected }))),
      this._card("Status screens", {
        sub: "Shared · on or off for this panel",
        info: `A status screen covers the panel while something is going on (toilet occupied, someone sleeping, robot vacuum at work). Each one lists the panels it appears on.${this._has(p, "images") ? "" : ` ${this._pname(p)} shows the text and the colour, without the picture.`}`,
        acts: this._btn("Add", () => this._newStatus(p), { cls: "small", icon: "plus" }),
      }, rules.length ? rules : h("div", { class: "empty" }, "No status screens yet."), h("div", { style: { height: "8px" } })),
      this._card("Do not disturb", { info: "Critical alerts always come through. What is not ticked here waits quietly; new notifications go into the list without a sound." },
        this._row("When", this._seg([["off", "Off"], ["on", "On"], ["schedule", "On a schedule"]], dnd.mode, (v) => setDnd({ mode: v }), { label: "Do not disturb" })),
        dnd.mode === "schedule" ? this._row("From – to", this._times(dnd.from, dnd.to, (f, t) => setDnd({ from: f, to: t }))) : null,
        this._row("Comes through", null, { stack: true, sub: changed ? "Changed from the default" : "The default" }),
        h("div", { class: "group", style: { marginBottom: "12px" } }, DND_ITEMS.map((it) => h("label", { class: `opt${it.locked ? " disabled" : ""}` },
          h("input", { type: "checkbox", checked: it.locked ? true : dnd.allow[it.id], disabled: Boolean(it.locked), onchange: (e) => setDnd({ allow: { [it.id]: e.target.checked } }) }),
          this._ckBox(it.locked ? true : dnd.allow[it.id], it.name),
          h("span", { class: "t" }, h("span", null, it.name), it.locked ? h("small", null, "Always") : null)))),
        changed ? h("div", { class: "below" }, this._btn("Back to the default", () => setDnd({ allow: { ...DND_DEFAULT_ALLOW } }), { cls: "small ghost", icon: "restore" })) : null),
      this._card("New notifications", { info: "The window shows in the middle, in the notifications page's look. Until answered keeps the newest in front until it is confirmed or dismissed on the panel." },
        this._row("Show", this._seg([["strip", "Strip at the top"], ["card", "Window"], ["off", "Only in the list"]], pops.kind, (v) => setPops({ kind: v }), { label: "New notifications" })),
        this._has(p, "speaker") ? this._row("Sound", this._toggle(pops.sound, (v) => setPops({ sound: v }), { label: "Sound" })) : null,
        pops.kind === "card" ? this._row("Until answered", this._toggle(Boolean(pops.until), (v) => setPops({ until: v }), { label: "Until answered" })) : null,
        pops.kind !== "off" ? this._more(`pop:${p.entry_id}`, this._row("Stays for", this._select([["5", "5 s"], ["10", "10 s"], ["30", "30 s"], ["60", "1 min"]], String(pops.seconds), (v) => setPops({ seconds: Number(v) }), { label: "Stays for", disabled: pops.kind === "card" && Boolean(pops.until) }))) : null),
    ];
  }

  _newStatus(p) {
    const id = `s${Date.now().toString(36).slice(-5)}${Math.random().toString(36).slice(2, 5)}`;
    this._fresh = { kind: "status", id, adding: true };
    this._saveHouse((hs) => {
      hs.status.push({ id, cat: "custom", trigger: { kind: "entity", entity: "", op: "on", value: "", after: "" }, own: true, headline: "", line: "", picto: "zip-face", colour: "#0b7285", button: true, button_label: "", countdown: 0, panels: [p.entry_id], on: true });
    });
    this._fresh.adding = false;
    this._openSheet({ kind: "status", id, panel: p.entry_id, pv: p.entry_id, fresh: true });
  }

  // -- voice ----------------------------------------------------------------------------

  // A voice satellite's wake words as the satellite itself has them (its select entities can lag behind). Asked
  // again when the satellite or its wake word selects change in Home Assistant (_watchSat), at most every 30 s while
  // the Voice section is shown, and 10 s after a failed try. A failed try keeps the last answer; without one the rows
  // say that it could not be read (never "No wake word"). The page is drawn again only when the answer changed.
  _loadSatCfg(sat, force = false) {
    const had = this._satCfg[sat];
    if (this._satCfgLoading === sat) {
      if (force) this._satCfgAgain = true;
      return;
    }
    if (!force && had && Date.now() - had.at < (had.failed ? 10000 : 30000)) return;
    this._satCfgLoading = sat;
    const words = (c) => JSON.stringify([c?.available_wake_words || [], c?.active_wake_words || [], Boolean(c?.failed && !c?.active_wake_words)]);
    this._hass.callWS({ type: "assist_satellite/get_configuration", entity_id: sat })
      .then((cfg) => { this._satCfg[sat] = { ...cfg, at: Date.now() }; })
      .catch(() => { this._satCfg[sat] = { ...(had && had.active_wake_words ? had : {}), failed: true, at: Date.now() }; })
      .finally(() => {
        this._satCfgLoading = null;
        if (this._satCfgAgain) {
          this._satCfgAgain = false;
          this._loadSatCfg(sat, true);
          return;
        }
        if (words(had) !== words(this._satCfg[sat])) this._render();
      });
  }

  // While the Voice section is shown: a change of the satellite or of its wake word selects (on the panel, through
  // the selects, another browser) asks the satellite again, once at once and once 2 s later (the satellite takes the
  // new words in the background); otherwise the answer is renewed when it is old (_loadSatCfg).
  _watchSat(hass) {
    const w = this._satWatch;
    if (!w || this._route().view !== "voice") return;
    const now = w.ids.map((id) => hass?.states?.[id]);
    const changed = now.some((s, k) => s !== w.states[k]);
    w.states = now;
    if (changed) {
      this._loadSatCfg(w.sat, true);
      clearTimeout(this._satAgainTimer);
      this._satAgainTimer = setTimeout(() => this._loadSatCfg(w.sat, true), 2000);
    } else this._loadSatCfg(w.sat);
  }

  // The voice satellite's device (ESPHome), shown as connected via the panel's device.
  _voiceDevices(p) {
    if (!p.device_id) return [];
    return Object.values(this._devices).filter((d) => d.via_device_id === p.device_id);
  }

  _viewVoice(p) {
    const st = this._settings(p);
    const devices = this._voiceDevices(p);
    const ids = new Set(devices.map((d) => d.id));
    const ents = Object.values(this._entityReg).filter((e) => ids.has(e.device_id) && !e.hidden && !e.hidden_by && !e.disabled_by && /^(select|switch|number)\./.test(e.entity_id) && this._hass.states[e.entity_id]);
    const sat = Object.values(this._entityReg).find((e) => ids.has(e.device_id) && e.entity_id.startsWith("assist_satellite."))?.entity_id;
    // the satellite and its wake word selects: a change of one asks the satellite again (_watchSat)
    if (sat) {
      const watch = [sat, ...ents.map((e) => e.entity_id).filter((x) => /^select\..*_wake_word(_2)?$/.test(x))];
      if (this._satWatch?.sat !== sat || this._satWatch.ids.join() !== watch.join()) this._satWatch = { sat, ids: watch, states: watch.map((x) => this._hass.states[x]) };
    } else this._satWatch = null;
    const control = (e) => {
      const id = e.entity_id;
      const s = this._hass.states[id];
      const domain = id.split(".")[0];
      const call = (service, data) => this._hass.callService(domain, service, { entity_id: id, ...data }).catch((err) => this._toast(err.message || String(err), null, true));
      if (domain === "switch") return this._toggle(s.state === "on", (v) => call(v ? "turn_on" : "turn_off", {}), { label: this._ename(id), disabled: s.state === "unavailable" });
      // the two wake word selects: what the satellite really listens for (its own configuration), by name
      const wk = /_wake_word(_2)?$/.exec(id);
      if (domain === "select" && wk && sat) {
        const cfg = this._satCfg[sat];
        this._loadSatCfg(sat);
        if (!cfg || !cfg.active_wake_words) return h("span", { class: "muted" }, cfg?.failed ? "Could not be read from the panel" : "…");
        const k = wk[1] ? 1 : 0;
        const names = Object.fromEntries((cfg.available_wake_words || []).map((w) => [w.id, w.wake_word]));
        const active = cfg.active_wake_words || [];
        const opts = [["", "No wake word"], ...(cfg.available_wake_words || []).map((w) => [w.id, w.wake_word])];
        return this._choice(opts, names[active[k]] ? active[k] : "", (v) => {
          const next = [active[0] || "", active[1] || ""];
          next[k] = v;
          const ids = next.filter((x, i) => x && next.indexOf(x) === i);
          // shown at once; the satellite's own answer follows
          this._satCfg[sat] = { ...cfg, active_wake_words: ids, at: Date.now() };
          this._render();
          this._hass.callWS({ type: "assist_satellite/set_wake_words", entity_id: sat, wake_word_ids: ids })
            .then(() => this._loadSatCfg(sat, true))
            .catch((err) => { this._toast(err.message || String(err), null, true); this._loadSatCfg(sat, true); });
        }, { label: this._ename(id) });
      }
      // the satellite selects' own ids (assistant, finished speaking detection, wake word) by their names
      if (domain === "select") return this._choice((s.attributes.options || []).map((o) => [o, VOICE_OPTION_NAMES[o] || o]), s.state, (v) => call("select_option", { option: v }), { label: this._ename(id) });
      const a = s.attributes;
      const v = Number(s.state);
      return this._slider(Number.isNaN(v) ? a.min || 0 : v, a.min ?? 0, a.max ?? 100, a.step ?? 1, (x) => call("set_value", { value: x }), (x) => `${x}${a.unit_of_measurement ? ` ${a.unit_of_measurement}` : ""}`, { label: this._ename(id), disabled: s.state === "unavailable" });
    };
    const short = (id) => {
      const name = this._ename(id);
      const dev = devices.find((d) => d.id === this._entityReg[id]?.device_id);
      const dn = dev && (dev.name_by_user || dev.name);
      return dn && name.startsWith(`${dn} `) ? name.slice(dn.length + 1) : name;
    };
    return [
      this._hero("Voice", [h("span", null, this._pname(p)), this._syncLine(p)]),
      this._card("On the screen", { info: "While you talk to it, the assistant fills the screen in this look; when it is done, the panel goes back to the page it was on." },
        this._lookPicker(VOICE_LOOKS.map((name, k) => ({ id: k, name, pic: this._lookPic(p, `voice-${k}`, () => svgNode(ART.voice[k]())) })), st.voice_look || 0, (v) => this._options(p, { voice_look: v }, { optimistic: (s) => { s.voice_look = v; } }))),
      this._card("Music commands", { info: "Music asked for by voice on this panel (play, next, pause, volume, stop) goes to the player picked on its music page, or always to the panel's own speaker." },
        this._row("Plays on", this._choice([["selected", "Selected player"], ["self", "This panel"]], st.voice_player || "selected", (v) => this._options(p, { voice_player: v }, { optimistic: (s) => { s.voice_player = v; } }), { label: "Plays on" }), { cls: "first" })),
      this._card("Voice assistant", { info: "The panel's voice satellite is its own ESPHome device in Home Assistant, shown as connected via this panel. Its settings (assistant, wake word, volume, microphone) are its entities." },
        ents.length ? ents.map((e, k) => this._row(short(e.entity_id), control(e), { cls: k === 0 ? "first" : "", stack: e.entity_id.startsWith("select.") }))
          : h("div", { class: "empty" }, "No voice satellite is connected via this panel yet."),
        devices.length ? h("div", { class: "below", style: { paddingTop: "8px" } }, devices.map((d) => this._btn(`Open ${d.name_by_user || d.name}`, () => this._go2(`/config/devices/device/${d.id}`), { cls: "small", icon: "open-in-new" }))) : null),
    ];
  }

  // Another page of Home Assistant.
  _go2(path) {
    history.pushState(null, "", path);
    window.dispatchEvent(new CustomEvent("location-changed", { detail: { replace: false } }));
  }

  // -- display ----------------------------------------------------------------------------

  _viewDisplay(p) {
    const s = p.states || {};
    const opts = p.select_options || {};
    const light = this._has(p, "light");
    const saver = { ...DEFAULT_SCREENSAVER, ...(this._settings(p).screensaver || {}) };
    const setSaver = (patch) => {
      const done = this._options(p, { screensaver: patch }, { optimistic: (x) => { x.screensaver = { ...saver, ...patch }; } });
      // the clock's look: the panel shows its screensaver meanwhile, so the picture here shows what is set
      if (["face", "seconds", "date", "weather", "next", "weight", "width", "speed", "colour"].some((k) => k in patch)) this._showSaver(p);
      return done;
    };
    const never = s.screensaver === "Never";
    const luxFilter = (st) => st.attributes?.device_class === "illuminance";
    const whenInfo = {
      dark: "The screensaver starts only while the room is darker than the level below. In daylight the panel keeps showing its pages.",
      window: "Outside these hours the panel keeps showing its pages.",
      night: "From sunset to sunrise at your Home Assistant location. In daytime the panel keeps showing its pages.",
      any: "The screensaver starts whenever nobody has used the panel for the time above.",
    }[saver.when];
    const whenRows = [this._row("When", this._select([["dark", "When the room is dark"], ["window", "Between two times"], ["night", "Sunset to sunrise"], ["any", "Any time"]], saver.when, (v) => setSaver({ when: v }), { label: "When the screensaver may start" }), { info: whenInfo, infoKey: `r:when:${saver.when}` })];
    // Auto: what switches it (without a light sensor the panel goes by the sun)
    const themeAuto = { ...DEFAULT_THEME_AUTO, ...(this._settings(p).theme_auto || {}) };
    const setThemeAuto = (patch) => this._options(p, { theme_auto: patch }, { optimistic: (x) => { x.theme_auto = { ...themeAuto, ...patch }; } });
    const autoBy = !light && themeAuto.by === "light" ? "sun" : themeAuto.by;
    const themeRows = s.theme !== "Auto" ? [] : [
      this._row("Switches by", this._select([light ? ["light", "Light in the room"] : null, ["sun", "Sunset and sunrise"], ["times", "Set times"]].filter(Boolean), autoBy, (v) => setThemeAuto({ by: v }), { label: "Auto switches by" })),
      autoBy === "times" ? this._row("Dark from – to", this._times(themeAuto.from, themeAuto.to, (f, t) => setThemeAuto({ from: f, to: t }))) : null,
    ];
    const wake = WAKE_DISTANCES.some(([k]) => k === this._settings(p).wake_distance) ? this._settings(p).wake_distance : "medium";
    if (saver.when === "dark") {
      whenRows.push(this._row("Darker than", this._slider(saver.lux, 1, 60, 1, (v) => setSaver({ lux: v }), (v) => `${v} lx`, { label: "Darker than" }), { sub: h("span", { "data-live": `lux:${p.entry_id}` }, this._liveValue(`lux:${p.entry_id}`)) }));
      if (!light) {
        whenRows.push(this._row("Light from", this._entityButton(saver.light_entity, { title: "Light from", domains: ["sensor"], filter: luxFilter, none: "Choose a light sensor", p, onPick: (v) => setSaver({ light_entity: v }) }),
          { info: `${this._reason(p, "light")} A light sensor from Home Assistant measures the room instead.` }));
      }
    }
    if (saver.when === "window") whenRows.push(this._row("From – to", this._times(saver.from, saver.to, (f, t) => setSaver({ from: f, to: t }))));
    const styles = opts.screensaver_style || [];
    return [
      this._hero("Display", [h("span", null, this._pname(p)), this._syncLine(p)]),
      this._card("Screen", {},
        (opts.theme || []).length ? this._row("Theme", this._seg(opts.theme.map((t) => [t, t]), s.theme, (v) => this._command(p, "theme", v), { label: "Theme" }), { info: "Auto switches between dark and light by itself." }) : null,
        (opts.theme || []).length ? themeRows : null,
        light ? this._row("Automatic brightness", this._toggle(s.auto_brightness, (v) => this._command(p, "auto_brightness", v), { label: "Automatic brightness" }), { sub: s.auto_brightness ? "Follows the light in the room" : null }) : null,
        !s.auto_brightness || !light ? this._row("Brightness", this._slider(typeof s.brightness === "number" ? s.brightness : 70, 1, 100, 1, (v) => this._command(p, "brightness", v), (v) => `${v} %`, { label: "Brightness" })) : null,
        (opts.time_format || []).length ? this._row("Time format", this._seg(opts.time_format.map((t) => [t, t]), s.time_format, (v) => this._command(p, "time_format", v), { label: "Time format" }), { info: "Every clock time the panel shows: clocks, alarms, calendar, notifications. Durations stay as they are." }) : null,
        p.connected ? null : h("p", { class: "note", style: { margin: "0 0 14px" } }, "These are the panel's own settings: they change while it is online.")),
      this._card("Screensaver", {},
        (opts.screensaver || []).length ? this._row("Starts after", this._select(opts.screensaver, s.screensaver, (v) => this._command(p, "screensaver", v), { label: "Screensaver starts after" }), { info: "How long nobody has to touch the panel before the screensaver comes." }) : null,
        never ? null : whenRows,
        never || !styles.length ? null : this._row("Style", this._dropdown(styles.map((x) => [x, x]), s.screensaver_style, (x) => this._command(p, "screensaver_style", x).then(() => this._showSaver(p)), { label: "Screensaver style" })),
        never || (styles.length && s.screensaver_style !== "Clock") ? null : this._saverClock(saver, setSaver, p),
        never || !light || saver.when !== "dark" ? null : this._more(`saver:${p.entry_id}`, this._row("Light from", this._entityButton(saver.light_entity, { title: "Light from", domains: ["sensor"], filter: luxFilter, none: "This panel's sensor", p, onPick: (v) => setSaver({ light_entity: v }) })))),
      this._has(p, "proximity") ? this._card("Waking", {},
        this._row("Wake when someone comes close", this._toggle(s.proximity_wake, (v) => this._command(p, "proximity_wake", v), { label: "Wake when someone comes close" })),
        this._row("Distance", this._seg(WAKE_DISTANCES, wake, (v) => this._options(p, { wake_distance: v }, { optimistic: (x) => { x.wake_distance = v; } }), { label: "Wake distance" }),
          { info: "How close someone comes before the panel wakes: about 30 cm (near), 40 cm (medium) or 45 cm (far). Presence uses the same distance." }),
        this._row("Calibrate", this._btn("Start", () => this._proxCal(p), { cls: "small", icon: "tune-vertical", disabled: !p.connected }),
          { info: "The panel measures its sensor with nobody in front of it, then shows how far it wakes. Keep the space in front of the panel clear for about 20 seconds." })) : null,
      this._has(p, "touch") ? this._card("Touch", {},
        this._row("Child lock", this._toggle(Boolean(s.child_lock), (v) => this._command(p, "child_lock", v), { label: "Child lock" }),
          { info: "Touches do nothing until someone holds the lock on the panel for 3 seconds. Voice works as usual. Once unlocked, the panel locks again after a minute without a touch." })) : null,
    ];
  }

  async _proxCal(p) {
    try {
      await this._hass.callWS({ type: "panelos/page/proximity_calibration", entry_id: p.entry_id });
      this._toast(`Calibrating on ${this._pname(p)}: keep the space in front of it clear`);
      this._schedulePicture(900);
      this._previewSheetIfNarrow(p);
    } catch (err) {
      this._toast(err.message || String(err), null, true);
    }
  }

  // -- device ------------------------------------------------------------------------------

  _viewDevice(p) {
    const s = p.states || {};
    const d = p.device || {};
    const rssi = s.wifi_signal;
    const nbars = typeof rssi === "number" ? (rssi > -55 ? 4 : rssi > -63 ? 3 : rssi > -72 ? 2 : 1) : 0;
    const bars = h("span", { class: "bars", "aria-hidden": "true" }, [6, 9, 12, 14].map((hh, k) => h("i", { class: k < nbars ? "on" : "", style: { height: `${hh}px` } })));
    const limits = this._data.reporting || {};
    const st = this._settings(p);
    return [
      this._hero("Device", [h("span", null, this._pname(p)), this._syncLine(p)]),
      this._card("About", {},
        this._row("Model", h("span", { class: "muted wrap" }, this._pmodel(p))),
        this._row("Software", h("span", { class: "muted wrap" }, s.app_version || d.sw_version || "–")),
        this._row("In Home Assistant", p.device_id ? this._btn("Open the device", () => this._go2(`/config/devices/device/${p.device_id}`), { cls: "small", icon: "open-in-new" }) : null, { sub: "Entities, automations, logbook" }),
        this._row("Room", this._btn(this._parea(p) || "None", () => this._openSheet({ kind: "rename", panel: p.entry_id }), { cls: "small", icon: "pencil-outline", disabled: !p.device_id }), { info: "The panel's area in Home Assistant. Quick controls use it." }),
        this._row("Address", h("span", { class: "muted num wrap" }, s.ip_address || "–")),
        this._row("Panel ID", h("span", { class: "muted wrap" }, st.panel_id || "–")),
        this._row("Home Assistant user", p.panel_user != null ? this._armed(`forget:${p.entry_id}`, "Forget", "Tap again to forget", () => this._forgetUser(p), { cls: "small", icon: "account-remove-outline" }) : h("span", { class: "muted wrap" }, "–"),
          { sub: p.panel_user || null, info: "Only this user and administrators can connect as this panel. After Forget, the next user that connects as this panel is kept." }),
        this._row("What it can do", null, { stack: true, sub: p.caps_reported ? "As the panel reports it" : "Not reported yet: treated as an NSPanel Pro" }),
        h("div", { class: "below muted" }, this._caps(p).map((c) => CAP_NAMES[c] || c).join(" · "))),
      this._card("Wi-Fi", {},
        this._row("Signal", h("span", { style: { display: "inline-flex", gap: "10px", alignItems: "center" } }, bars, h("span", { class: "muted num" }, typeof rssi === "number" ? `${rssi} dBm` : "–"))),
        this._row("Roaming", this._toggle(st.wifi_roaming !== false, (v) => this._options(p, { wifi_roaming: v }, { optimistic: (x) => { x.wifi_roaming = v; } }), { label: "Roaming" }),
          { info: "The panel switches by itself to a clearly stronger access point of the same Wi-Fi network." })),
      this._card("Sensors", { info: "Every value the panel sends is a row in the recorder's database. The panel sends a sensor only when it has changed by at least this much since the value it sent last. Lower: finer history, more writes." },
        this._more(`sens:${p.entry_id}`, Object.entries(limits).map(([key, lim]) => {
          const meta = REPORTING[key] || { label: key };
          const now = p.states?.[meta.now];
          return this._row(meta.label, this._slider(st[key] ?? lim.default, lim.min, lim.max, lim.step, (v) => this._options(p, { [key]: v }, { optimistic: (x) => { x[key] = v; } }), (v) => `${lim.step < 1 ? Number(v).toFixed(1) : Math.round(v)} ${lim.unit}`, { label: meta.label }),
            { sub: typeof now === "number" ? `Now ${now} ${meta.unit}` : null });
        }))),
      this._card("Actions", {},
        h("div", { style: { display: "flex", gap: "8px", flexWrap: "wrap", padding: "10px 0 14px" } },
          this._has(p, "touch") ? this._btn("Calibrate touch", () => this._touchTest(p), { icon: "gesture-tap", disabled: !p.connected }) : null,
          this._armed(`restart:${p.entry_id}`, "Restart the app", "Tap again to restart", () => this._command(p, "restart_app", null).then((ok) => ok && this._toast(`Restarting the app on ${this._pname(p)}…`)), { icon: "restart", disabled: !p.connected }),
          this._armed(`reboot:${p.entry_id}`, "Reboot", "Tap again to reboot", () => this._command(p, "reboot", null).then((ok) => ok && this._toast(`Rebooting ${this._pname(p)}…`)), { icon: "power", disabled: !p.connected })),
        this._more(`dev:${p.entry_id}`,
          this._row(`Remove ${this._pname(p)}`, this._armed(`remove:${p.entry_id}`, "Remove", "Tap again to remove", () => this._removePanel(p), { cls: "danger small", icon: "delete-outline" }), { sub: "Its settings are deleted from Home Assistant" }))),
    ];
  }

  async _touchTest(p) {
    try {
      await this._hass.callWS({ type: "panelos/page/touch_test", entry_id: p.entry_id });
      this._toast(`The touch test is on ${this._pname(p)}: tap the points it shows`);
      this._schedulePicture(900);
      this._previewSheetIfNarrow(p);
    } catch (err) {
      this._toast(err.message || String(err), null, true);
    }
  }

  async _removePanel(p) {
    try {
      await this._hass.callApi("DELETE", `config/config_entries/entry/${p.entry_id}`);
      this._toast(`${this._pname(p)} removed`);
      this._go("/all");
    } catch (err) {
      this._toast(err.message || String(err), null, true);
    }
  }

  async _forgetUser(p) {
    try {
      await this._hass.callWS({ type: "panelos/page/forget_user", entry_id: p.entry_id });
      this._toast(`${this._pname(p)}: user forgotten`);
    } catch (err) {
      this._toast(err.message || String(err), null, true);
    }
  }

  // -- all panels ---------------------------------------------------------------------------

  _viewAll() {
    const panels = this._panels;
    const house = this._data.house || {};
    const feats = ["images", "video", "web", "mic", "speaker", "proximity", "light", "temp", "humidity"];
    const firstAlerts = panels[0] ? `${this._panelPath(panels[0], "alerts")}` : "/all";
    return [
      h("div", { class: "hero" }, h("div", { class: "t" }, h("h1", null, "Panels"), h("div", { class: "meta" }, `${plural(panels.length, "panel", "panels")} · ${panels.filter((x) => x.connected).length} online`)),
        h("div", { class: "acts" }, this._btn("Add a panel", () => this._startAdd(), { cls: "primary", icon: "plus" }))),
      h("div", { class: "pgrid" }, panels.map((x) => {
        const version = x.states?.app_version || x.device?.sw_version;
        return h("div", { class: "pcard", role: "button", tabindex: "0", onclick: () => this._go(this._panelPath(x)), onkeydown: (e) => { if (e.key === "Enter") this._go(this._panelPath(x)); } },
          h("div", { class: "stage" }, this._deviceNode(x, 164)),
          h("div", { class: "name" }, h("b", null, this._pname(x)), h("span", { class: `state${x.connected ? " on" : ""}` }, h("b"), x.connected ? "Online" : "Offline")),
          h("div", { class: "meta" }, h("span", null, [this._pmodel(x), this._parea(x)].filter(Boolean).join(" · ")), version ? h("span", null, `Version ${version}`) : null,
            h("span", null, plural(this._pages(x).filter((pg, k) => k === 0 || !pg.hidden).length, "page", "pages"))));
      }), h("button", { class: "pcard add", onclick: () => this._startAdd() }, h("div", { style: { display: "flex", flexDirection: "column", alignItems: "center", gap: "8px" } }, icon("plus"), "Add a panel"))),
      this._card("Shared by every panel", {},
        this._row("Critical alerts", this._btn("Edit", () => this._go(firstAlerts), { cls: "small" }), { sub: plural([...(house.critical || []), ...(house.own_critical || [])].filter((c) => c.on).length, "alert", "alerts"), lead: h("span", { class: "lead" }, icon("alert-octagon-outline")), keep: true, cls: "first" }),
        this._row("Status screens", this._btn("Edit", () => this._go(firstAlerts), { cls: "small" }), { sub: `${plural((house.status || []).length, "screen", "screens")}, each on the panels you choose`, lead: h("span", { class: "lead" }, icon("card-text-outline")), keep: true }),
        panels.length > 1 ? this._row("Copy settings between panels", this._btn("Copy…", () => this._openCopy(panels[0]), { cls: "small", icon: "content-copy" }), { sub: "Pages, display, voice, do not disturb", lead: h("span", { class: "lead" }, icon("content-copy")), keep: true }) : null),
      this._card("What each panel can do", { info: "As each panel reports it. Settings a panel cannot use are left out of its pages; when copying, they are skipped with the reason." },
        h("div", { class: "scroll-x" }, h("table", { class: "matrix" },
          h("thead", null, h("tr", null, h("th", null, ""), panels.map((x) => h("th", null, this._pname(x))))),
          h("tbody", null, feats.map((c) => h("tr", null, h("td", null, CAP_NAMES[c]), panels.map((x) => h("td", null, this._has(x, c) ? icon("check", "y") : icon("close", "n")))))))),
        h("div", { style: { height: "10px" } })),
    ];
  }

  // ================================================================ sheets

  // The screensaver's clock: face, parts, letters (weight, width), wandering speed, colour. Shown only for what the
  // chosen face has.
  // p: the screensaver's own settings (faces to pick from); null on the clock page (its Look card is the face).
  _saverClock(saver, setSaver, p) {
    const face = Number.isInteger(saver.face) ? saver.face : 1;
    const digits = [0, 1, 6, 7].includes(face);
    const wt = saver.weight || 600;
    const width = saver.width || 100;
    const tog = (key, label) => this._row(label, this._toggle(saver[key] !== false, (v) => setSaver({ [key]: v }), { label }));
    return h("div", { class: "sec" }, h("h3", null, "Clock"),
      p ? this._lookPicker(SAVER_FACES.map((n, k) => ({ id: k, name: n, pic: this._lookPic(p, `saver-${k}`, () => svgNode(ART.clock[k]())) })), face, (k) => setSaver({ face: k })) : null,
      h("div", { class: "group below" },
        tog("seconds", face === 7 || face === 0 || face === 6 ? "Breathing colon" : face === 1 ? "Seconds line" : "Second hand"),
        tog("date", "Date"),
        tog("weather", "Weather"),
        tog("next", "Next event"),
        this._row("Letters", this._weightSlider(wt, (v) => setSaver({ weight: v }))),
        digits ? this._row("Width", this._seg([[85, "Narrow"], [100, "Normal"], [115, "Wide"]], [85, 100, 115].reduce((a, b) => (Math.abs(b - width) < Math.abs(a - width) ? b : a)), (v) => setSaver({ width: v }), { label: "Width" })) : null,
        face === 6 ? this._row("Speed", this._slider(saver.speed || 20, 5, 60, 5, (v) => setSaver({ speed: v }), (v) => `${v} px/s`, { label: "Speed" })) : null),
      h("div", { class: "swatches below" },
        h("button", { class: `sw panel${!saver.colour ? " on" : ""}`, "aria-label": "The panel's colour", title: "The panel's colour", onclick: () => setSaver({ colour: "" }) }),
        ACCENTS.map((c) => h("button", { class: `sw${saver.colour === c ? " on" : ""}`, style: { background: c }, "aria-label": c, onclick: () => setSaver({ colour: c }) }))));
  }

  _sheet_preview(sh, p) {
    return this._sheetFrame(this._pname(p), h("div", { style: { display: "flex", justifyContent: "center", paddingTop: "4px" } },
      this._previewBlock(p, { width: Math.min((this.clientWidth || 360) - 40, 360), height: Math.min((this.clientHeight || 700) * 0.55, 420) })), null);
  }

  _sheet_more(sh, p) {
    const secs = this._sections(p).filter((s) => !["overview", "pages", "alerts", "display"].includes(s.id));
    const item = (ic, label, run, sub) => h("button", { onclick: run }, h("span", { class: "lead" }, icon(ic)), h("span", { class: "t" }, h("span", null, label), sub ? h("small", null, sub) : null), icon("chevron-right"));
    return this._sheetFrame("More", h("div", { class: "picker-list", style: { maxHeight: "none" } },
      secs.map((s) => item(s.icon, s.long || s.name, () => { this._ui.sheet = null; this._go(this._panelPath(p, s.id)); }, this._pname(p))),
      this._panels.length > 1 ? item("content-copy", "Copy settings to other panels", () => this._openCopy(p)) : null,
      p.device_id ? item("pencil-outline", `Rename ${this._pname(p)}`, () => this._openSheet({ kind: "rename", panel: p.entry_id })) : null,
      item("view-grid-outline", "All panels", () => { this._ui.sheet = null; this._go("/all"); }),
      item("plus", "Add a panel", () => this._startAdd())), null);
  }

  _sheet_panels() {
    const r = this._route();
    return this._sheetFrame("Panels", h("div", { class: "picker-list", style: { maxHeight: "none" } },
      this._panels.map((x) => h("button", { class: x.entry_id === r.panel && r.view !== "all" ? "on" : "", onclick: () => { this._ui.sheet = null; this._go(this._panelPath(x)); } },
        h("span", { class: "thumb" }, this._picture(x, { placeholder: false })),
        h("span", { class: "t" }, h("span", null, this._pname(x)), h("small", null, [this._pmodel(x), this._parea(x)].filter(Boolean).join(" · "))),
        h("span", { class: `state${x.connected ? " on" : ""}` }, h("b")))),
      h("button", { onclick: () => { this._ui.sheet = null; this._go("/all"); } }, h("span", { class: "lead" }, icon("view-grid-outline")), h("span", { class: "t" }, h("span", null, "All panels")), icon("chevron-right")),
      h("button", { onclick: () => this._startAdd() }, h("span", { class: "lead" }, icon("plus")), h("span", { class: "t" }, h("span", null, "Add a panel")), icon("chevron-right"))), null);
  }

  _sheet_rename(sh, p) {
    const dev = this._device(p) || {};
    if (sh.name == null) sh.name = dev.name_by_user || this._pname(p);
    if (sh.area == null) sh.area = dev.area_id || "";
    const areas = Object.values(this._areas).sort((a, b) => a.name.localeCompare(b.name));
    const save = async () => {
      const name = sh.name.trim();
      try {
        await this._hass.callWS({ type: "config/device_registry/update", device_id: p.device_id, name_by_user: name && name !== dev.name ? name : null, area_id: sh.area || null });
        this._ui.sheet = null;
        if (this._reg) await this._loadRegistries();
        this._toast("Saved");
        this._render();
      } catch (err) {
        this._toast(err.message || String(err), null, true);
      }
    };
    return this._sheetFrame("Rename", [
      h("div", { class: "sec" }, h("h3", null, "Name"), h("input", { class: "inp", value: sh.name, maxlength: 60, "aria-label": "Name", oninput: (e) => { sh.name = e.target.value; } })),
      h("div", { class: "sec" }, h("h3", null, "Room"), this._dropdown([["", "None"], ...areas.map((a) => [a.area_id, a.name])], sh.area, (v) => { sh.area = v; this._renderLayer(); }, { label: "Room", full: true })),
    ], [this._btn("Cancel", () => this._closeSheet(), { cls: "ghost" }), this._btn("Save", save, { cls: "primary" })], { center: true });
  }

  _sheet_catalogue(sh, p) {
    const pages = this._pages(p);
    const full = pages.length >= (this._data.max_pages || 12);
    const items = TYPE_ORDER.map((kind) => {
      const sup = this._pageSupported(p, kind);
      const count = pages.filter((x) => x.kind === kind).length;
      const t = PAGE_TYPES[kind];
      return { sup, node: h("button", { class: "cat-item", disabled: !sup.ok || full, onclick: () => this._addPage(p, kind) },
        h("div", { class: "pic" }, lookArt(kind, 0, { options: { theme: 1 } })),
        count ? h("span", { class: "tag" }, count > 1 ? `${count} added` : "Added") : null,
        h("b", null, t.name), h("small", null, sup.ok ? t.desc : sup.why)) };
    });
    const ok = items.filter((x) => x.sup.ok);
    const no = items.filter((x) => !x.sup.ok);
    return this._sheetFrame("Add a page", [
      full ? h("p", { class: "note", style: { margin: 0 } }, `${this._pname(p)} has the most pages it can take (${this._data.max_pages || 12}).`) : null,
      h("div", { class: "cat" }, ok.map((x) => x.node)),
      no.length ? h("div", { class: "sec" }, h("h3", null, `Not on ${this._pname(p)}`), h("div", { class: "cat" }, no.map((x) => x.node))) : null,
    ], null, { wide: true });
  }

  _addPage(p, kind) {
    const pages = clone(this._pages(p));
    const area = this._pareaId(p);
    const pick = (domain, n, sameArea = false) => {
      const list = this._ents([domain], { p });
      const mine = sameArea && area ? list.filter((e) => e.area === area) : [];
      return (mine.length ? mine : list).slice(0, n).map((e) => e.id);
    };
    const page = { kind, variant: 0, entities: [], options: {}, hidden: false };
    if (kind === "home") {
      page.options = { theme: 1, real_sun: true };
      const first = pages.find((x) => x.kind === "home");
      if (first) page.options.items = this._items(first);
    }
    if (kind === "timers") page.options = { functions: ["timer", "alarm", "stopwatch"] };
    if (kind === "camera") page.entities = pick("camera", 4);
    if (kind === "calendar") page.entities = pick("calendar", 3);
    if (kind === "todo") page.entities = pick("todo", 1);
    if (kind === "thermostat") page.entities = pick("climate", 1, true);
    pages.push(page);
    this._ui.sheet = null;
    this._savePages(p, pages, { toast: `${PAGE_TYPES[kind].name} added to ${this._pname(p)}` });
    this._go(`${this._panelPath(p, "pages")}/${pages.length - 1}`);
  }

  // Choose an entity: a search field and the list (the panel's area first). The list is made once per opening
  // (after the sheet is on screen, with the loading mark meanwhile), drawn in pages of PICK_PAGE rows as it is
  // scrolled, and filtered without redrawing the sheet, so the search field keeps its text and the list its place.
  _sheet_pick(sh, p) {
    const choose = (id) => {
      const back = sh.back;
      this._ui.sheet = null;
      sh.onPick(id);
      if (back) this._openSheet(back);
      else this._renderLayer();
    };
    const box = h("div", { class: "picker-list", style: { maxHeight: "none" } });
    const row = (e) => h("button", { class: e.id === sh.current ? "on" : "", onclick: () => choose(e.id) },
      h("span", { class: "lead" }, icon(this._eicon(e.id))),
      h("span", { class: "t" }, h("span", null, e.name), h("small", null, [this._areaName(e.area), e.id].filter(Boolean).join(" · "))),
      e.id === sh.current ? icon("check") : null);
    const matches = () => {
      const q = (sh.q || "").trim().toLowerCase();
      return q ? sh.items.filter((e) => e.key.includes(q)) : sh.items;
    };
    const more = () => {
      const all = matches();
      const from = box.querySelectorAll("button[data-row]").length;
      const next = all.slice(from, from + PICK_PAGE);
      box.querySelector(".more")?.remove();
      for (const e of next) {
        const b = row(e);
        b.dataset.row = "1";
        box.append(b);
      }
      if (from + next.length < all.length) box.append(h("div", { class: "more" }, `${all.length - from - next.length} more – scroll or search`));
    };
    const fill = () => {
      box.replaceChildren();
      if (sh.none && !(sh.q || "").trim()) {
        box.append(h("button", { class: !sh.current ? "on" : "", onclick: () => choose("") },
          h("span", { class: "lead" }, icon("minus")), h("span", { class: "t" }, h("span", null, sh.none)), !sh.current ? icon("check") : null));
      }
      if (!matches().length) box.append(h("div", { class: "empty" }, sh.items.length ? "Nothing matches." : "Nothing to choose from."));
      else more();
    };
    if (!sh.items) {
      box.append(h("div", { class: "pload", role: "status" }, h("span", { class: "d" }, h("i"), h("i"), h("i")), "Loading the list…"));
      // the list is made after the sheet is drawn (the loading mark shows meanwhile)
      setTimeout(() => {
        if (this._ui.sheet !== sh) return;
        sh.items = this._ents(sh.domains, { p, filter: sh.filter || null })
          .filter((e) => !(sh.exclude || []).includes(e.id))
          .map((e) => ({ ...e, key: `${e.name} ${e.id} ${this._areaName(e.area)}`.toLowerCase() }));
        // the current choice comes first, so it is in view
        const at = sh.items.findIndex((e) => e.id === sh.current);
        if (at > 0) sh.items.unshift(...sh.items.splice(at, 1));
        if (box.isConnected) fill();
        else this._renderLayer();
      }, 30);
    } else fill();
    const search = h("input", { class: "inp", type: "search", placeholder: "Search", "aria-label": "Search", value: sh.q || "", oninput: (e) => {
      sh.q = e.target.value;
      if (sh.items) fill();
    } });
    const frame = this._sheetFrame(sh.title, [search, box], null, { back: sh.back ? () => this._openSheet(sh.back) : null });
    // the next rows come when the list is scrolled near its end
    const body = frame.querySelector(".sheet-body");
    if (body) body.addEventListener("scroll", () => {
      if (sh.items && body.scrollTop + body.clientHeight > body.scrollHeight - 300 && box.querySelector(".more")) more();
    }, { passive: true });
    return frame;
  }

  _sheet_addWidget(sh, p) {
    const page = this._pages(p)[sh.page];
    if (!page) return null;
    const items = this._items(page);
    const kinds = page.kind === "weather" ? WEATHER_WIDGETS : Object.keys(WIDGETS).map(Number);
    return this._sheetFrame("Add a widget", h("div", { class: "cat" }, kinds.map((kind) => {
      const w = WIDGETS[kind];
      const spot = this._freeSpot(items, w.def[0], w.def[1]);
      const small = spot ? null : this._freeSpot(items, w.min[0], w.min[1]);
      return h("button", { class: "cat-item", onclick: () => {
        // no free place: it goes on top of the others at its smallest size (the layout then points it out)
        const at = spot || small || { x: 0, y: 0 };
        const [ww, hh] = spot ? w.def : w.min;
        const item = { kind, col: at.x, row: at.y, w: ww, h: hh, style: w.style || 0, accent: "", weight: 600, tap: page.kind !== "weather", opts: w.opts || 0 };
        items.push(item);
        this._ui.sheet = null;
        this._ui.sel = items.length - 1;
        this._saveItems(p, sh.page, items, { toast: `${w.name} added` });
        // the sensor widget needs its sensor: its settings open at once
        if (kind === 8) this._openSheet({ kind: "widget", panel: p.entry_id, page: sh.page, item: items.length - 1 });
      } }, h("div", { class: "pic wicon" }, this._widgetPreview(p, page, kind) || this._lookPic(p, `widget-${kind}`, () => icon(w.icon), { fit: "contain" })), h("b", null, w.name), h("small", null, spot ? `${w.def[0]} × ${w.def[1]}` : small ? "Smaller, to fit" : "On top of the others"));
    })), null, { wide: true });
  }

  // -- live widget previews -------------------------------------------------------------
  // The widget picker draws each widget as the panel does, at its first size, with the live content of the panel's
  // Home Assistant: the time and date (in Home Assistant's time zone, the clock running), the weather now and its
  // forecasts (the page's weather, else the panel's), the next event of the page's calendars, the music of the page's
  // player (else the panel's). A widget without data shows the panel's own picture with demo content (null here).

  // The panel's time format: its Time format setting as it reports it ("24-hour" / "12-hour"); 24-hour until known.
  _h24(p) {
    const v = String((p.states || {}).time_format || "");
    return !v.startsWith("12");
  }

  // Now in Home Assistant's time zone: hour, minute, second, weekday, day, month.
  _hassNow(at = Date.now()) {
    const opts = { hour: "2-digit", minute: "2-digit", second: "2-digit", weekday: "long", day: "numeric", month: "long", hourCycle: "h23" };
    let f;
    try {
      f = new Intl.DateTimeFormat("en-GB", { ...opts, timeZone: this._hass?.config?.time_zone || undefined });
    } catch (_e) {
      f = new Intl.DateTimeFormat("en-GB", opts);
    }
    const o = Object.fromEntries(f.formatToParts(new Date(at)).map((x) => [x.type, x.value]));
    return { h: Number(o.hour) % 24, m: Number(o.minute), s: Number(o.second), weekday: o.weekday, day: Number(o.day), month: o.month };
  }

  _clockText(p, n) {
    const hh = this._h24(p) ? String(n.h).padStart(2, "0") : String(((n.h + 11) % 12) + 1);
    return [hh, String(n.m).padStart(2, "0")];
  }

  // A weather entity's forecast (hourly / daily) once, through Home Assistant's own subscription: kept 10 minutes; the
  // picker is drawn again when it arrives. null until it is there.
  _forecast(entity, type) {
    const key = `${entity}|${type}`;
    const cache = (this._fc = this._fc || {});
    const c = cache[key];
    if (c && (c.loading || Date.now() - c.at < 600000)) return c.list || null;
    cache[key] = { list: c?.list || null, loading: true, at: Date.now() };
    let unsub = null;
    let done = false;
    const finish = (list) => {
      if (done) return;
      done = true;
      cache[key] = { list, at: Date.now() };
      if (unsub) unsub.then((u) => u()).catch(() => {});
      if (this._ui.sheet?.kind === "addWidget") this._renderLayer();
    };
    try {
      unsub = this._hass.connection.subscribeMessage((msg) => finish(Array.isArray(msg?.forecast) ? msg.forecast : []),
        { type: "weather/subscribe_forecast", entity_id: entity, forecast_type: type });
      unsub.catch(() => finish([]));
      setTimeout(() => finish(c?.list || []), 8000);
    } catch (_e) {
      finish([]);
    }
    return c?.list || null;
  }

  // The data a page's widgets show (as the panel picks it).
  _widgetData(p, page) {
    const st = this._settings(p);
    const ents = page.entities || [];
    const pick = (domain, fallback) => ents.find((e) => e.startsWith(`${domain}.`)) || fallback || "";
    const usable = (id) => { const s = id && this._hass?.states?.[id]; return s && !["unavailable", "unknown"].includes(s.state) ? s : null; };
    return {
      wx: pick("weather", st.weather_entity),
      wxState: usable(pick("weather", st.weather_entity)),
      calendars: ents.filter((e) => e.startsWith("calendar.")),
      player: usable(pick("media_player", st.media_player_entity)),
    };
  }

  // The next (or current) event of the calendars, from their own states (as app/src/cal.rs next_event).
  _nextEvent(p, calendars) {
    const tints = ["#3d9bff", "#ff7a3d", "#2fd27a", "#c86bfa", "#ffb21e", "#ff4fa0"];
    const now = Date.now();
    let best = null;
    calendars.forEach((id, i) => {
      const a = this._hass?.states?.[id]?.attributes || {};
      const start = Date.parse(String(a.start_time || "").replace(" ", "T"));
      const end = Date.parse(String(a.end_time || "").replace(" ", "T"));
      if (!a.message || Number.isNaN(start) || Number.isNaN(end) || end <= now) return;
      if (!best || start < best.start) best = { start, end, title: String(a.message), allDay: Boolean(a.all_day), tint: tints[i % tints.length] };
    });
    if (!best) return null;
    const day = (t) => { const n = this._hassNow(t); return `${n.month} ${n.day}`; };
    const hm = (t) => this._clockText(p, this._hassNow(t)).join(":");
    let rel;
    if (best.start <= now) rel = "now";
    else if (day(best.start) === day(now)) {
      const d = Math.round((best.start - now) / 60000);
      rel = d >= 60 ? `in ${Math.floor(d / 60)} h ${d % 60} min` : `in ${Math.max(1, d)} min`;
    } else if (day(best.start) === day(now + 86400000)) rel = "Tomorrow";
    else if (best.start < now + 7 * 86400000) rel = this._hassNow(best.start).weekday;
    else rel = `${this._hassNow(best.start).day} ${this._hassNow(best.start).month}`;
    return { ...best, rel, time: best.allDay ? "All day" : `${hm(best.start)} – ${hm(best.end)}` };
  }

  // The previews' colours: the panel's look (dark or light).
  _wpvInk(p) {
    const dark = this._lookMode(p) !== "light";
    return {
      bg: dark ? "#000000" : "#F5F3EF", ink: dark ? "#FFFFFF" : "#101114", card: dark ? "#1F1F1F" : "#D9D7D3",
      soft: dark ? "rgba(255,255,255,.75)" : "rgba(16,17,20,.72)", acc: "#FF4A1C", accText: dark ? "#FF8466" : "#C2401F",
      track: dark ? "rgba(255,255,255,.2)" : "rgba(16,17,20,.18)", rain: dark ? "#86B8FF" : "#3E7FD6",
    };
  }

  _widgetPreview(p, page, kind) {
    const c = this._wpvInk(p);
    // the People widget at its first size, showing everyone
    if (kind === 10) return this._peoplePreview(c, { w: WIDGETS[10].def[0], h: WIDGETS[10].def[1], opts: WIDGETS[10].opts });
    const d = this._widgetData(p, page);
    const W = WIDGETS[kind].def[0] === 4 ? 460 : 236;
    const font = 'font-family="PanelOS Text, Noto Sans, sans-serif"';
    const T = (x, y, size, fill, text, { weight = 400, anchor = "start", attrs = "" } = {}) =>
      `<text x="${x}" y="${y}" font-size="${size}" font-weight="${weight}" fill="${fill}" text-anchor="${anchor}" ${font} ${attrs}>${esc(text)}</text>`;
    const card = `<rect x="6" y="6" width="${W - 12}" height="98" rx="14" fill="${c.card}"/>`;
    const deg = (v) => (v == null || Number.isNaN(Number(v)) ? "–" : `${Math.round(Number(v))}°`);
    const now = this._hassNow();
    let body = null;
    if (kind === 0) {
      const [hh, mm] = this._clockText(p, now);
      body = `<text x="${W / 2}" y="76" font-size="56" font-weight="600" fill="${c.ink}" text-anchor="middle" ${font} data-wpv="clock"><tspan>${hh}</tspan><tspan fill="${c.acc}">:</tspan><tspan>${mm}</tspan></text>`
        + `<rect x="${W / 2 - 87}" y="90" width="174" height="3" rx="1.5" fill="${c.track}"/>`
        + `<rect x="${W / 2 - 87}" y="90" width="174" height="3" rx="1.5" fill="${c.acc}" data-wpv="sec" style="transform-box: fill-box; transform-origin: left; transform: scaleX(${(now.s / 60).toFixed(3)})"/>`;
    } else if (kind === 1) {
      body = T(18, 46, 17, c.accText, now.weekday, { weight: 500, attrs: 'data-wpv="weekday"' }) + T(18, 82, 27, c.ink, `${now.day} ${now.month}`, { weight: 600, attrs: 'data-wpv="daymonth"' });
    } else if (kind === 2) {
      const s = d.wxState;
      if (!s) return null;
      body = wxIcon(s.state, 46, 30, 40, c.ink) + T(98, 66, 40, c.ink, deg(s.attributes?.temperature), { weight: 600 }) + T(100, 90, 15, c.soft, fitText(wxName(s.state), W - 112, 15));
    } else if (kind === 3) {
      const ev = this._nextEvent(p, d.calendars);
      if (!ev) return null;
      body = card + `<rect x="22" y="26" width="4" height="58" rx="2" fill="${ev.tint}"/>`
        + T(38, 46, 14, c.soft, fitText(`${ev.rel}  ·  ${ev.time}`, W - 60, 14)) + T(38, 80, 24, c.ink, fitText(ev.title, W - 60, 24), { weight: 600 });
    } else if (kind === 4 || kind === 5 || kind === 6) {
      const s = d.wxState;
      if (!s) return null;
      const list = this._forecast(d.wx, kind === 6 ? "daily" : "hourly");
      if (!list || !list.length) return null;
      const lab = (f) => {
        const n = this._hassNow(Date.parse(f.datetime));
        if (kind === 6) return n.day === now.day && n.month === now.month ? "Today" : n.weekday.slice(0, 3);
        return this._h24(p) ? String(n.h).padStart(2, "0") : `${((n.h + 11) % 12) + 1}${n.h < 12 ? "a" : "p"}`;
      };
      const rain = (f) => (f.precipitation_probability == null ? "" : `${Math.round(f.precipitation_probability)}%`);
      if (kind === 4) {
        const pts = [{ t: s.attributes?.temperature, l: "Now", r: null }, ...list.slice(0, 5).map((f) => ({ t: f.temperature, l: lab(f), r: f.precipitation_probability }))];
        const ts = pts.map((x) => Number(x.t)).filter((v) => !Number.isNaN(v));
        if (ts.length < 2) return null;
        const lo = Math.min(...ts);
        const hi = Math.max(...ts);
        const xs = pts.map((x, i) => 34 + (i * (W - 68)) / (pts.length - 1));
        const ys = pts.map((x) => 40 + (hi === lo ? 12 : ((hi - Number(x.t)) / (hi - lo)) * 26));
        let path = `M${xs[0]} ${ys[0]}`;
        for (let i = 1; i < xs.length; i++) {
          const mx = (xs[i - 1] + xs[i]) / 2;
          path += ` C${mx} ${ys[i - 1]} ${mx} ${ys[i]} ${xs[i]} ${ys[i]}`;
        }
        body = card + `<path d="${path}" fill="none" stroke="${c.ink}" stroke-width="2.2"/>`
          + pts.map((x, i) => `<circle cx="${xs[i]}" cy="${ys[i]}" r="${i ? 3.6 : 4.4}" fill="${i ? c.ink : c.acc}"/>` + T(xs[i], ys[i] - 9, 14, c.ink, deg(x.t), { weight: 600, anchor: "middle" })
            + (x.r == null ? "" : `<rect x="${xs[i] - 9}" y="78" width="18" height="2" rx="1" fill="${c.rain}" opacity="${(0.3 + (0.7 * Math.min(100, Number(x.r) || 0)) / 100).toFixed(2)}"/>`)
            + T(xs[i], 96, 13, c.soft, x.l, { anchor: "middle" })).join("");
      } else {
        const n = kind === 5 ? 7 : 5;
        const gap = kind === 5 ? 6 : 8;
        const cw = (W - 12 - gap * (n - 1)) / n;
        const lo = (f) => (f.templow == null ? "" : deg(f.templow));
        body = list.slice(0, n).map((f, i) => {
          const x = 6 + i * (cw + gap);
          const cx = x + cw / 2;
          const temp = kind === 6 && f.templow != null
            ? `<text x="${cx}" y="76" font-size="15" text-anchor="middle" ${font}><tspan font-weight="600" fill="${c.ink}">${esc(deg(f.temperature))}</tspan><tspan fill="${c.soft}" dx="4">${esc(lo(f))}</tspan></text>`
            : T(cx, 76, 15, c.ink, deg(f.temperature), { weight: 600, anchor: "middle" });
          return `<rect x="${x}" y="8" width="${cw}" height="94" rx="12" fill="${c.card}"/>` + T(cx, 27, 13, c.soft, lab(f), { anchor: "middle" })
            + wxIcon(f.condition, cx - 10, 34, 20, c.ink) + temp + T(cx, 95, 12, c.rain, rain(f), { anchor: "middle" });
        }).join("");
      }
    } else if (kind === 7) {
      const m = d.player;
      const a = m?.attributes || {};
      if (!m || !a.media_title) return null;
      const dur = Number(a.media_duration) || 0;
      let pos = Number(a.media_position) || 0;
      if (m.state === "playing" && a.media_position_updated_at) pos += (Date.now() - Date.parse(a.media_position_updated_at)) / 1000;
      pos = Math.max(0, Math.min(dur || pos, pos));
      const mmss = (v) => `${Math.floor(v / 60)}:${String(Math.floor(v % 60)).padStart(2, "0")}`;
      const clip = `wpvc${++artId}`;
      const cover = a.entity_picture
        ? `<clipPath id="${clip}"><rect x="18" y="22" width="64" height="64" rx="6"/></clipPath><image href="${esc(a.entity_picture)}" x="18" y="22" width="64" height="64" preserveAspectRatio="xMidYMid slice" clip-path="url(#${clip})"/>`
        : `<rect x="18" y="22" width="64" height="64" rx="6" fill="${c.track}"/><path d="M53 40v18.5a5 5 0 1 1-3-4.6V44l-10 2v15a5 5 0 1 1-3-4.6V42z" fill="${c.soft}"/>`;
      const play = m.state === "playing"
        ? '<rect x="379" y="41" width="5" height="18" rx="1.5" fill="#fff"/><rect x="388" y="41" width="5" height="18" rx="1.5" fill="#fff"/>'
        : '<path d="M381 40l15 10-15 10z" fill="#fff"/>';
      body = card + cover + T(100, 43, 17, c.ink, fitText(a.media_title, 226, 17), { weight: 600 }) + T(100, 63, 14, c.soft, fitText(a.media_artist || a.media_album_name || "", 226, 14))
        + `<rect x="100" y="71" width="226" height="3" rx="1.5" fill="${c.track}"/>` + (dur ? `<rect x="100" y="71" width="${(226 * pos) / dur}" height="3" rx="1.5" fill="${c.acc}"/>` : "")
        + T(100, 92, 12, c.soft, mmss(pos)) + T(213, 92, 12, c.soft, fitText(a.friendly_name || "", 110, 12), { anchor: "middle" }) + (dur ? T(326, 92, 12, c.soft, `-${mmss(Math.max(0, dur - pos))}`, { anchor: "end" }) : "")
        + `<path d="M342 43v14M356 43l-10 7 10 7z" fill="${c.ink}" stroke="${c.ink}" stroke-width="2.4" stroke-linejoin="round"/>`
        + `<circle cx="386" cy="50" r="22" fill="${c.acc}"/>${play}`
        + `<path d="M430 43v14M416 43l10 7-10 7z" fill="${c.ink}" stroke="${c.ink}" stroke-width="2.4" stroke-linejoin="round"/>`;
    } else if (kind === 8) {
      // a sensor of Home Assistant with a number and a unit (a temperature first), as the widget shows one
      const all = Object.values(this._hass?.states || {}).filter((x) => x.entity_id.startsWith("sensor.") && x.attributes?.unit_of_measurement
        && !Number.isNaN(Number(x.state)) && x.state !== "");
      const s = all.find((x) => x.attributes?.device_class === "temperature") || all[0];
      if (!s) return null;
      const v = Number(s.state);
      const val = Math.abs(v) >= 100 ? String(Math.round(v)) : String(Math.round(v * 10) / 10);
      const name = s.attributes.friendly_name || s.entity_id;
      body = `<path transform="translate(14 34) scale(1.6)" d="M15 13V5A3 3 0 0 0 9 5V13A5 5 0 1 0 15 13M12 4A1 1 0 0 1 13 5V8H11V5A1 1 0 0 1 12 4Z" fill="${c.acc}"/>`
        + T(70, 42, 13, c.soft, fitText(name, W - 84, 13))
        + `<text x="70" y="82" ${font} fill="${c.ink}"><tspan font-size="36" font-weight="600">${esc(val)}</tspan><tspan font-size="16" fill="${c.soft}" dx="4">${esc(s.attributes.unit_of_measurement)}</tspan></text>`;
    }
    if (body == null) return null;
    const node = svgNode(`<svg viewBox="0 0 ${W} 110" preserveAspectRatio="xMidYMid meet" aria-hidden="true"><rect width="${W}" height="110" fill="${c.bg}"/>${body}</svg>`, "art wpv");
    if (kind === 0 || kind === 1) this._wpvRun(p);
    return node;
  }

  // The People widget as the panel draws it (app/ui/home.slint, PeopleW) at its size: its people (none chosen:
  // everyone) in entity id order, each as their profile picture (Profile pictures on and a picture in Home Assistant)
  // or their initials in their colour; the ones away (or unknown) fainter inside a grey ring and only with its switch
  // on; in as many rows as give the biggest heads. null when Home Assistant has no people.
  _peoplePreview(c, w) {
    const tints = ["#3b82f6", "#22a06b", "#a855f7", "#f59e0b", "#ef4444", "#06b6d4"];
    const all = Object.values(this._hass?.states || {}).filter((x) => x.entity_id.startsWith("person.")).sort((a, b) => (a.entity_id < b.entity_id ? -1 : 1));
    if (!all.length) return null;
    const pick = Array.isArray(w.people) ? w.people : [];
    const away = Boolean((w.opts || 0) & 1);
    const pics = !((w.opts || 0) & 2);
    const shown = all.map((x, i) => ({ x, tint: tints[i % tints.length] }))
      .filter(({ x }) => (!pick.length || pick.includes(x.entity_id)) && (away || x.state === "home"));
    const cw = w.w || 2;
    const ch = w.h || 1;
    const W = cw * 112;
    const H = ch * 98;
    const most = ch >= 2 ? 64 : cw < 2 ? 40 : 46;
    const gap = ch >= 2 ? 10 : 6;
    const n = shown.length;
    let body = "";
    if (n) {
      const size = (k) => { const r = Math.ceil(n / k); return Math.min(most, (W - 16 - (k - 1) * gap) / k, (H - 12 - (r - 1) * gap) / r); };
      const [c1, c2, c3, c4] = [n, Math.ceil(n / 2), Math.ceil(n / 3), Math.ceil(n / 4)];
      const beats = (a, ...others) => others.every((o) => size(a) > size(o) + 0.5);
      const cols = beats(c4, c3, c2, c1) ? c4 : beats(c3, c2, c1) ? c3 : beats(c2, c1) ? c2 : c1;
      const rows = Math.ceil(n / cols);
      const d = size(cols);
      const top = (H - (rows * d + (rows - 1) * gap)) / 2;
      body = shown.map(({ x, tint }, at) => {
        const row = Math.floor(at / cols);
        const inRow = Math.min(cols, n - row * cols);
        const cx = (W - (inRow * d + (inRow - 1) * gap)) / 2 + (at % cols) * (d + gap) + d / 2;
        const cy = top + row * (d + gap) + d / 2;
        const nm = x.attributes?.friendly_name || x.entity_id.slice(7);
        const ini = nm.split(/\s+/).filter(Boolean).map((part) => part[0]).slice(0, 2).join("").toUpperCase();
        const home = x.state === "home";
        // away: a grey ring at the head's edge, the head inside it with a gap, fainter
        const rw = home ? 0 : Math.max(2, d * 0.06);
        const r = d / 2 - (home ? 0 : rw + Math.max(1.5, d * 0.05));
        const fade = home ? 1 : 0.45;
        const ring = home ? "" : `<circle cx="${cx}" cy="${cy}" r="${d / 2 - rw / 2}" fill="none" stroke="#8e8e93" stroke-width="${rw}"/>`;
        const pic = pics ? x.attributes?.entity_picture : "";
        if (pic) {
          const clip = `wpvc${++artId}`;
          return ring + `<clipPath id="${clip}"><circle cx="${cx}" cy="${cy}" r="${r}"/></clipPath>`
            + `<image href="${esc(pic)}" x="${cx - r}" y="${cy - r}" width="${2 * r}" height="${2 * r}" preserveAspectRatio="xMidYMid slice" clip-path="url(#${clip})" opacity="${fade}"/>`;
        }
        return ring + `<circle cx="${cx}" cy="${cy}" r="${r}" fill="${tint}" fill-opacity="${fade}"/>`
          + `<text x="${cx}" y="${cy}" font-size="${(2 * r * 0.38).toFixed(1)}" font-weight="600" fill="#ffffff" fill-opacity="${home ? 1 : 0.75}" text-anchor="middle" dominant-baseline="central" font-family="PanelOS Text, Noto Sans, sans-serif">${esc(ini)}</text>`;
      }).join("");
    }
    return svgNode(`<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="xMidYMid meet" aria-hidden="true"><rect width="${W}" height="${H}" fill="${c.bg}"/>${body}</svg>`, "art wpv");
  }

  // The clock and the date of the open picker go on live (every second, in place; stops with the picker).
  _wpvRun(p) {
    if (this._wpvTimer) return;
    this._wpvTimer = setInterval(() => {
      const els = this._layer.querySelectorAll("[data-wpv]");
      if (!els.length || this._ui.sheet?.kind !== "addWidget") { clearInterval(this._wpvTimer); this._wpvTimer = null; return; }
      const n = this._hassNow();
      const [hh, mm] = this._clockText(p, n);
      els.forEach((e) => {
        const k = e.dataset.wpv;
        if (k === "clock") { const ts = e.querySelectorAll("tspan"); if (ts[0].textContent !== hh) ts[0].textContent = hh; if (ts[2].textContent !== mm) ts[2].textContent = mm; }
        else if (k === "sec") e.style.transform = `scaleX(${(n.s / 60).toFixed(3)})`;
        else if (k === "weekday" && e.textContent !== n.weekday) e.textContent = n.weekday;
        else if (k === "daymonth" && e.textContent !== `${n.day} ${n.month}`) e.textContent = `${n.day} ${n.month}`;
      });
    }, 1000);
  }

  _sheet_widget(sh, p) {
    const page = this._pages(p)[sh.page];
    const items = page ? this._items(page) : [];
    const w = items[sh.item];
    if (!w) return null;
    const meta = WIDGETS[w.kind] || WIDGETS[0];
    const set = (patch) => { Object.assign(w, patch); this._saveItems(p, sh.page, items); };
    // a switch only where it does something on the panel; where the widget's size or style hides it, it says so
    const bit = (b, label) => {
      const why = this._bitWhy(w, b);
      return this._row(label, this._toggle(Boolean((w.opts || 0) & b), (v) => set({ opts: v ? (w.opts || 0) | b : (w.opts || 0) & ~b }), { label }),
        why ? { sub: why, cls: "dim" } : {});
    };
    const body = [];
    if (w.kind === 10) {
      const pv = this._peoplePreview(this._wpvInk(p), w);
      // at its size on the panel, half again as big
      const [pw, ph] = [(w.w || 2) * 112, (w.h || 1) * 98];
      if (pv) body.push(h("div", { class: "wpv-one", style: { aspectRatio: `${pw} / ${ph}`, maxWidth: `${pw * 1.5}px` } }, pv));
      // the people it shows, in the widget's order; every one ticked is everyone (the ones added later too), the last
      // one ticked stays
      const all = this._ents(["person"]).sort((a, b) => (a.id < b.id ? -1 : 1));
      const ids = all.map((e) => e.id);
      const own = Array.isArray(w.people) ? w.people : [];
      // (read at the click: All and None tick several in a row)
      const toggle = (id) => {
        const cur = Array.isArray(w.people) ? w.people : [];
        let list = cur.length ? [...cur] : [...ids];
        if (list.includes(id)) {
          if (list.filter((x) => ids.includes(x)).length <= 1) { this._toast("At least one person stays on the widget"); return; }
          list = list.filter((x) => x !== id);
        } else list = [...list, id].sort();
        if (ids.every((x) => list.includes(x))) { delete w.people; set({}); } else set({ people: list });
      };
      body.push(h("div", { class: "sec" }, h("h3", null, "People"), all.length
        ? this._multi({ label: "People", key: `people:${p.entry_id}:${sh.page}:${sh.item}` }, all.map((e) => this._mi(!own.length || own.includes(e.id), e.name, () => toggle(e.id))))
        : h("div", { class: "empty" }, "No people in Home Assistant.")));
    }
    if (w.kind === 8) body.push(h("div", { class: "group" }, this._row("Sensor", this._entityButton(w.entity || "", { title: "Sensor", domains: ["sensor"], p, back: sh,
      onPick: (id) => { if (id) { set({ entity: id }); if (this._ui.sheet === sh) this._renderLayer(); } } }), { cls: "first" })));
    if (w.kind === 0) body.push(h("div", { class: "sec" }, h("h3", null, "Style"),
      this._lookPicker(CLOCK_STYLES.map((n, k) => {
        const m = widgetMin({ kind: 0, style: k, opts: (w.opts || 0) & ~1 });
        const ok = w.w >= m[0] && w.h >= m[1];
        return { id: k, name: n, pic: this._lookPic(p, `clockw-${k}`, () => svgNode((ART.clock[k] || ART.clock[0])()), { fit: "contain" }), disabled: !ok, why: ok ? null : `${n} needs ${m[0]} × ${m[1]} cells.` };
      }), w.style || 0, (k) => set({ style: k }), { wide: true })));
    const bits = meta.bits;
    if (w.kind === 7) {
      const scroll = Boolean((w.opts || 0) & 1);
      body.push(h("div", { class: "group" },
        this._row("Long titles", this._seg([[0, "Cut off"], [1, "Scroll"]], scroll ? 1 : 0, (v) => set({ opts: v ? (w.opts || 0) | 1 : (w.opts || 0) & ~1 }), { label: "Long titles" }), { cls: "first" }),
        scroll ? this._row("Scroll speed", this._slider(w.style || 3, 1, 5, 1, (v) => set({ style: v }), (v) => ["", "Slowest", "Slow", "Medium", "Fast", "Fastest"][v], { label: "Scroll speed" })) : null));
    }
    const rows = bits.filter(([b]) => this._bitWhy(w, b) !== false).map(([b, label]) => bit(b, label));
    // the People widget: the profile pictures or the initials only (bit 2 keeps the initials, so pictures are the default)
    if (w.kind === 10) rows.unshift(this._row("Profile pictures", this._toggle(!((w.opts || 0) & 2), (v) => set({ opts: v ? (w.opts || 0) & ~2 : (w.opts || 0) | 2 }), { label: "Profile pictures" })));
    if (rows.length) { rows[0].classList.add("first"); body.push(h("div", { class: "group" }, rows)); }
    // the letters: their weight (every widget with text), and the page's neon on the synthwave background
    if (w.kind !== 5 && w.kind !== 6 && !meta.noweight) {
      const wt = w.weight || 600;
      body.push(h("div", { class: "group" },
        this._row("Letters", this._weightSlider(wt, (v) => set({ weight: v })), { cls: "first" }),
        page && BACKGROUNDS[this._bgOf(p, page)]?.theme === 8
          ? this._row("Neon letters", this._toggle(typeof w.neon === "boolean" ? w.neon : page.options?.neon !== false, (v) => set({ neon: v }), { label: "Neon letters" }))
          : null));
    }
    if (!meta.nocolour) body.push(h("div", { class: "sec" }, h("h3", null, "Colour"), h("div", { class: "swatches" },
      h("button", { class: `sw panel${!w.accent ? " on" : ""}`, "aria-label": "The panel's colour", title: "The panel's colour", onclick: () => set({ accent: "" }) }),
      ACCENTS.map((c) => h("button", { class: `sw${w.accent === c ? " on" : ""}`, style: { background: c }, "aria-label": c, onclick: () => set({ accent: c }) })))));
    // the card behind the widget: -1 = its own as designed (a glass card behind the event, the curve and
    // the music widget, none behind the others), 0..100 % = a card of that opacity
    const ownCard = [3, 4, 7].includes(w.kind) ? 12 : 0;
    const bg = Number.isInteger(w.bg) && w.bg >= 0 ? w.bg : -1;
    body.push(h("div", { class: "group" },
      this._row("Background", this._slider(bg >= 0 ? bg : ownCard, 0, 100, 5, (v) => set({ bg: v }), (v) => (bg < 0 ? `${v} % · Default` : `${v} %`), { label: "Background" }),
        { cls: "first", info: "How solid the card behind this widget is: 0 % is no card, 100 % a solid one.",
          sub: bg >= 0 ? h("button", { class: "linkbtn", onclick: () => set({ bg: -1 }) }, "Back to the default") : null }),
      page?.kind === "weather" || meta.notap ? null : this._row("Opens its page on a tap", this._toggle(w.tap !== false, (v) => set({ tap: v }), { label: "Opens its page on a tap" }))));
    return this._sheetFrame(meta.name, body, [
      this._btn("Remove", () => { this._ui.sheet = null; this._removeWidget(p, sh.page, sh.item); this._renderLayer(); }, { cls: "ghost danger", icon: "delete-outline" }),
      h("span", { class: "grow" }), this._btn("Done", () => this._closeSheet(), { cls: "primary" })]);
  }

  // Why a widget switch does nothing now: null = it works, a text = what it needs (size, style),
  // false = never in this style (then it is not offered).
  _bitWhy(w, b) {
    const st = w.style || 0;
    const wide = w.w > w.h * 1.5;
    if (w.kind === 0) {
      if (st === 7 || st === 9) return false;
      if (st === 8 || st === 10) {
        if (b !== 1) return false;
        return st === 8 && w.w < 4 ? "Shows when the widget is 4 columns wide" : null;
      }
      if (b === 1) return null;
      if ([0, 1, 6].includes(st)) return w.h >= 2 ? null : "Shows when the widget is 2 rows high";
      if (wide) return null;
      if (b === 2 && (st === 3 || st === 5)) return null;
      return "Shows when the widget is wider than it is high";
    }
    if (w.kind === 1) return w.w >= 2 ? null : "Shows when the widget is 2 columns wide";
    if (w.kind === 2) return w.h >= 2 || w.w >= 2 ? null : "Shows when the widget is 2 columns wide or 2 rows high";
    if (w.kind === 4 && b === 2) return w.h >= 2 ? null : "Shows when the widget is 2 rows high";
    return null;
  }

  _sheet_tile(sh, p) {
    const page = this._pages(p)[sh.page];
    if (!page) return null;
    const ents = [...(page.entities || [])];
    // per tile, in the tiles' order: the short label, and on the flight deck whether a group starts there and its name
    const align = (list, empty) => {
      const out = Array.isArray(list) ? [...list] : [];
      while (out.length < ents.length) out.push(empty);
      out.length = ents.length;
      return out;
    };
    const tapes = align(page.options?.tapes, "");
    const starts = align(page.options?.group_starts, false);
    const gnames = align(page.options?.group_names, "");
    const cur = ents[sh.slot] || null;
    // the mixing desk's tapes and the flight deck's plates both show a short label
    const desk = [1, 2].includes(page.variant || 0);
    const custom = (page.variant || 0) === 2 && page.options?.groups === "custom" && sh.slot < 8;
    const save = (nextEnts, nextTapes, nextStarts = starts, nextNames = gnames) => {
      const options = { ...(page.options || {}) };
      for (const [key, list] of [["tapes", nextTapes], ["group_starts", nextStarts], ["group_names", nextNames]]) {
        if (list.some(Boolean)) options[key] = list;
        else delete options[key];
      }
      return this._setPage(p, sh.page, { entities: nextEnts, options });
    };
    const domains = this._data.tile_domains || ["light", "switch", "cover", "fan", "input_boolean", "scene", "script", "sensor", "climate"];
    // the control is chosen in the common picker (search, loading mark, steady list), then this sheet comes back
    const choose = this._entityButton(cur || "", { title: cur ? "Control" : "Choose a control", domains, p, back: sh,
      exclude: ents.filter((e) => e !== cur), onPick: (id) => {
        if (!id) return;
        const ne = [...ents];
        const nt = [...tapes];
        const ns = [...starts];
        const nn = [...gnames];
        if (cur) { ne[sh.slot] = id; if (id !== cur) nt[sh.slot] = ""; } else { ne.push(id); nt.push(""); ns.push(false); nn.push(""); sh.slot = ne.length - 1; }
        save(ne, nt, ns, nn).then(() => { if (this._ui.sheet === sh) this._renderLayer(); });
      } });
    const move = (d) => {
      const k = sh.slot;
      const lists = [[...ents], [...tapes], [...starts], [...gnames]];
      for (const list of lists) list.splice(k + d, 0, list.splice(k, 1)[0]);
      sh.slot = k + d;
      save(...lists);
    };
    return this._sheetFrame(cur ? this._ename(cur) : "New tile", [
      h("div", { class: "group" }, this._row(cur ? "Control" : "Choose what it controls", choose, { cls: "first" })),
      cur && desk ? h("div", { class: "group" }, this._row("Label", this._text(tapes[sh.slot], (v) => { const nt = [...tapes]; nt[sh.slot] = v.trim().toUpperCase().slice(0, 12); save(ents, nt); }, { max: 12, placeholder: "From the name", label: "Label", cls: "short" }),
        { info: `${page.variant === 2 ? "The plate under the switch." : "The label-maker tape under the fader."} Empty: the panel makes one from the name.`, cls: "first" })) : null,
      cur && custom ? h("div", { class: "group" },
        sh.slot > 0 ? this._row("Starts a group", this._toggle(Boolean(starts[sh.slot]), (v) => {
          const ns = [...starts];
          ns[sh.slot] = v;
          save(ents, tapes, ns, gnames).then(() => { if (this._ui.sheet === sh) this._renderLayer(); });
        }, { label: "Starts a group" }), { cls: "first" }) : null,
        sh.slot === 0 || starts[sh.slot] ? this._row("Group name", this._text(gnames[sh.slot], (v) => { const nn = [...gnames]; nn[sh.slot] = v.trim().slice(0, 12); save(ents, tapes, starts, nn); },
          { max: 12, placeholder: "No frame", label: "Group name", cls: "short" }),
          { cls: sh.slot === 0 ? "first" : null, info: "The painted frame over this control and the ones after it, up to the next group. Empty: no frame." }) : null) : null,
      cur ? h("div", { style: { display: "flex", gap: "8px", flexWrap: "wrap" } },
        this._btn("Earlier", () => move(-1), { cls: "small", icon: "arrow-left", disabled: sh.slot === 0 }),
        this._btn("Later", () => move(1), { cls: "small", icon: "arrow-right", disabled: sh.slot >= ents.length - 1 })) : null,
    ], [
      cur ? this._btn("Remove", () => { const lists = [[...ents], [...tapes], [...starts], [...gnames]]; for (const list of lists) list.splice(sh.slot, 1); this._ui.sheet = null; save(...lists); this._renderLayer(); }, { cls: "ghost danger", icon: "delete-outline" }) : null,
      h("span", { class: "grow" }), this._btn("Done", () => this._closeSheet(), { cls: "primary" })]);
  }

  // The sound part of a critical alert's sheet (sensor-based and own ones alike).
  _critSound(c, set, sh, p, holdInfo) {
    const soundOn = c.sound !== false;
    return h("div", { class: "sec" }, h("h3", null, "Sound"), h("div", { class: "group" },
      this._row("Play a sound", this._toggle(soundOn, (v) => set((x) => { x.sound = v; }), { label: "Play a sound" }), { cls: "first", info: "Off: the alert only shows on the screen. Panels without a speaker always show it silently." }),
      soundOn ? this._soundRow("Alert sound", c.tone, { def: "sos", defNote: "The default for critical alerts", title: "Alert sound", crit: true, p, parent: { ...sh, fresh: false } }, (v) => set((x) => { x.tone = v; })) : null,
      soundOn ? this._row("Hold to silence", this._toggle(c.hold, (v) => set((x) => { x.hold = v; }), { label: "Hold to silence" }), { info: holdInfo }) : null));
  }

  _critShownOn(c, set) {
    return h("div", { class: "sec" }, h("h3", null, "Shown on"), this._multi({ label: "Shown on", key: `on:crit:${c.entity_id || c.id}` }, this._panels.map((q) => {
      const on = !(c.off || []).includes(q.entry_id);
      return this._mi(on, this._pname(q), () => set((x) => { x.off = on ? [...(x.off || []), q.entry_id] : (x.off || []).filter((e) => e !== q.entry_id); }));
    })));
  }

  _sheet_crit(sh, p) {
    const c = (this._data.house.critical || []).find((x) => x.entity_id === sh.id);
    if (!c) return null;
    const cls = SAFETY[this._safetyClass(c.entity_id)];
    const set = (fn) => this._saveHouse((hs) => fn(hs.critical.find((x) => x.entity_id === c.entity_id)));
    return this._sheetFrame(this._ename(c.entity_id), [
      h("div", { class: "group" },
        this._row("Counts as critical", this._toggle(c.on, (v) => set((x) => { x.on = v; }), { label: "Counts as critical" }), { cls: "first", sub: [cls ? cls.name : "Sensor", this._areaName(this._earea(c.entity_id))].filter(Boolean).join(" · ") })),
      this._critSound(c, set, sh, p, "On: the sound stops after holding the button on the panel; the alert stays until the sensor clears. Off: it sounds until the sensor clears."),
      this._critShownOn(c, set),
    ], [
      this._btn("Remove from the list", () => {
        const gone = clone(c);
        this._closeSheet();
        this._saveHouse((hs) => { hs.critical = hs.critical.filter((x) => x.entity_id !== c.entity_id); }, { toast: `${this._ename(c.entity_id)} removed`, undo: () => this._saveHouse((hs) => { hs.critical.push(gone); }) });
      }, { cls: "ghost danger", icon: "delete-outline" }),
      h("span", { class: "grow" }),
      this._btn(`Test on ${this._pname(p)}`, () => { this._closeSheet(); this._test(p, "critical", c.entity_id); }, { icon: "alert-octagon-outline", disabled: !p.connected }),
      this._btn("Done", () => this._closeSheet(), { cls: "primary" })]);
  }

  // An own critical alert: started by a device's state or by an automation (panelos.critical_alert).
  _ownCritName(c) {
    const t = c.trigger || {};
    return c.headline || (t.kind === "entity" && t.entity ? this._ename(t.entity) : "Critical alert");
  }

  _ownCritWhen(c) {
    const t = c.trigger || {};
    if (t.kind === "service") return "from an automation";
    if (!t.entity) return "no device chosen yet";
    const d = t.entity.split(".")[0];
    const name = this._ename(t.entity);
    if (d === "sensor") return t.value !== "" && t.value != null ? `${name} ${t.op === "below" ? "below" : "above"} ${t.value}` : `${name}, no value yet`;
    if (d === "lock") return `${name} ${t.op === "off" ? "locked" : "unlocked"}`;
    return `${name} ${t.op === "off" ? "off" : "on"}`;
  }

  _newOwnCrit(p) {
    const ids = new Set((this._data.house?.own_critical || []).map((c) => c.id));
    let n = 1;
    while (ids.has(`alert_${n}`)) n++;
    const id = `alert_${n}`;
    this._fresh = { kind: "own_critical", id, adding: true };
    this._saveHouse((hs) => {
      hs.own_critical.push({ id, on: true, hold: true, off: [], sound: true, tone: "auto", trigger: { kind: "entity", entity: "", op: "on", value: "" }, headline: "", line: "" });
    });
    this._fresh.adding = false;
    this._openSheet({ kind: "ownCrit", id, panel: p.entry_id, fresh: true });
  }

  _sheet_ownCrit(sh, p) {
    const c = (this._data.house.own_critical || []).find((x) => x.id === sh.id);
    if (!c) return null;
    const t = c.trigger || { kind: "entity", entity: "", op: "on", value: "" };
    const set = (fn) => this._saveHouse((hs) => fn(hs.own_critical.find((x) => x.id === c.id)));
    const setT = (patch) => set((x) => { x.trigger = { ...t, ...patch }; });
    const domain = t.entity ? t.entity.split(".")[0] : "";
    const ops = domain === "sensor" ? [["above", "Above"], ["below", "Below"]] : domain === "lock" ? [["on", "Unlocked"], ["off", "Locked"]] : [["on", "On"], ["off", "Off"]];
    const back = { ...sh, fresh: false };
    const numeric = (st) => st.entity_id.split(".")[0] !== "sensor" || !Number.isNaN(Number(st.state)) || Boolean(st.attributes?.unit_of_measurement);
    return this._sheetFrame(sh.fresh ? "New critical alert" : this._ownCritName(c), [
      h("div", { class: "group" },
        this._row("Counts as critical", this._toggle(c.on, (v) => set((x) => { x.on = v; }), { label: "Counts as critical" }), { cls: "first", sub: `Own · ${this._ownCritWhen(c)}` })),
      h("div", { class: "sec" }, h("h3", null, "Starts when"),
        this._seg([["entity", "A device"], ["service", "Automation"]], t.kind, (v) => setT({ kind: v }), { label: "Starts when" }),
        t.kind === "service"
          ? h("div", { class: "yaml" }, `action: panelos.critical_alert\ndata:\n  alert: ${c.id}\n\n# ends it\naction: panelos.clear_critical_alert\ndata:\n  alert: ${c.id}`)
          : h("div", { class: "group" },
            this._row("Device", this._entityButton(t.entity, { title: "Device", domains: OWN_CRIT_DOMAINS, filter: numeric, none: null, p, back,
              onPick: (id) => { const d = id.split(".")[0]; setT({ entity: id, op: d === "sensor" ? "above" : "on", value: "" }); } }), { cls: "first" }),
            t.entity ? this._row("Alarm when", h("div", { style: { display: "flex", gap: "8px", alignItems: "center", flexWrap: "wrap", justifyContent: "flex-end" } },
              this._seg(ops, ops.some(([v]) => v === t.op) ? t.op : ops[0][0], (v) => setT({ op: v }), { label: "Alarm when" }),
              domain === "sensor" ? this._text(t.value, (v) => setT({ value: v.trim() }), { max: 60, label: "Value", cls: "short", placeholder: String(this._hass.states[t.entity]?.state ?? "") }) : null),
            { sub: `Now: ${this._liveValue(`ha:${t.entity}`)}` }) : null)),
      h("div", { class: "sec" }, h("h3", null, "Text"), h("div", { class: "group" },
        this._row("Headline", this._text(c.headline, (v) => set((x) => { x.headline = v.trim(); }), { max: 30, label: "Headline", placeholder: "What happened" }), { cls: "first", stack: true }),
        this._row("Line", this._text(c.line, (v) => set((x) => { x.line = v.trim(); }), { max: 60, label: "Line", placeholder: "What to do" }), { stack: true }))),
      this._critSound(c, set, sh, p, "On: the sound stops after holding the button on the panel; the alert stays until it ends. Off: it sounds until it ends."),
      this._critShownOn(c, set),
    ], [
      this._btn("Remove", () => {
        const gone = clone(c);
        const at = (this._data.house.own_critical || []).findIndex((x) => x.id === c.id);
        this._closeSheet();
        this._saveHouse((hs) => { hs.own_critical = hs.own_critical.filter((x) => x.id !== c.id); }, { toast: `${this._ownCritName(c)} removed`,
          undo: () => this._saveHouse((hs) => { if (!hs.own_critical.some((x) => x.id === gone.id)) hs.own_critical.splice(Math.max(0, Math.min(at, hs.own_critical.length)), 0, gone); }) });
      }, { cls: "ghost danger", icon: "delete-outline" }),
      h("span", { class: "grow" }),
      // a test needs something that starts it (a new alert without a device is not kept)
      this._btn(`Test on ${this._pname(p)}`, () => { this._closeSheet(); this._test(p, "critical", `own:${c.id}`); }, { icon: "alert-octagon-outline", disabled: !p.connected || (t.kind === "entity" && !t.entity) }),
      this._btn("Done", () => this._closeSheet(), { cls: "primary" })]);
  }

  // Pick a sound: the PanelOS sounds (the place's default marked), the uploaded ones (upload, delete).
  _sheet_sound(sh, p) {
    const cur = this._soundKey(sh.value, sh.def);
    const pick = (k) => {
      sh.value = k === `lib:${sh.def}` ? "auto" : k;
      sh.onpick(sh.value);
      this._renderLayer();
    };
    const item = (k, name, sub, extra = null) => h("div", { class: `snd${k === cur ? " on" : ""}`, onclick: () => pick(k) },
      this._playBtn(`sheet:${k}`, k, name),
      h("button", { class: "t", role: "radio", "aria-checked": k === cur ? "true" : "false", onclick: (e) => { e.stopPropagation(); pick(k); } }, h("span", null, name), sub ? h("small", null, sub) : null),
      extra,
      k === cur ? h("span", { class: "chk" }, icon("check")) : null);
    const lib = SOUND_LIB.filter((s) => sh.crit || !s.crit);
    const own = this._ownSounds();
    const up = this._ui.uploading;
    const file = h("input", { type: "file", hidden: true, accept: "audio/mpeg,audio/wav,audio/x-wav,audio/ogg,audio/flac,audio/x-flac,.mp3,.wav,.ogg,.flac",
      onchange: (e) => { const f = e.target.files && e.target.files[0]; e.target.value = ""; this._uploadSound(f, (v) => pick(v)); } });
    const leave = () => {
      this._stopSound();
      if (sh.parent) this._openSheet(sh.parent);
      else this._closeSheet();
    };
    return this._sheetFrame(sh.title, [
      h("div", { class: "sec" }, h("h3", null, "PanelOS sounds"),
        h("div", { class: "sndlist", role: "radiogroup", "aria-label": "PanelOS sounds" }, lib.map((s) => item(`lib:${s.id}`, s.name, s.id === sh.def ? sh.defNote : null, s.id === sh.def ? h("span", { class: "tag" }, "Default") : null)))),
      h("div", { class: "sec" }, h("h3", null, "Your sounds", h("span", { class: "rinfo" }, this._infoBtn("snd:own", "Uploaded once, usable on every panel. Each panel that uses a sound keeps a copy, so it rings even when Home Assistant is away. MP3, WAV, OGG or FLAC, up to 10 MB."))),
        own.length || up ? h("div", { class: "sndlist", role: "radiogroup", "aria-label": "Your sounds" },
          own.map((s) => item(`own:${s.id}`, s.name, this._soundSize(s),
            h("button", { class: "snddel", "aria-label": `Delete ${s.name}`, title: "Delete", onclick: (e) => { e.stopPropagation(); this._deleteSound(s, sh); } }, icon("delete-outline")))),
          up ? h("div", { class: "snd busy", "aria-busy": "true" }, h("span", { class: "lead" }, icon("loading", "spin")), h("span", { class: "t" }, h("span", null, up.name), h("small", null, "Uploading…"))) : null) : null,
        file,
        this._btn("Upload a sound", () => file.click(), { cls: "small upl", icon: "upload", disabled: Boolean(up) })),
      sh.ma ? this._musicSection(sh, item) : null,
    ], [h("span", { class: "grow" }), this._btn("Done", leave, { cls: "primary" })], { back: sh.parent ? leave : null });
  }

  // Alarms only: music from Music Assistant, played on the panel's speaker and getting louder.
  _musicSection(sh, item) {
    const m = (this._ui.ma = this._ui.ma || { query: null, items: [], loading: false, available: true, error: "" });
    const load = (query) => {
      m.query = query;
      m.loading = true;
      const asked = query;
      this._hass.callWS({ type: "panelos/page/ma", query }).then((r) => {
        if (m.query !== asked) return;
        m.items = r.items || [];
        m.available = r.available !== false;
        m.error = "";
        this._ui.maNames = this._ui.maNames || {};
        for (const x of m.items) this._ui.maNames[`ma:${x.uri}`] = x.name;
      }).catch((e) => { if (m.query === asked) { m.items = []; m.error = (e && e.message) || "Music Assistant did not answer."; } })
        .finally(() => { if (m.query === asked) { m.loading = false; this._renderLayer(); } });
    };
    if (m.query === null) load("");
    const defName = (SOUND_LIB.find((s) => s.id === sh.def) || {}).name || "The default sound";
    const search = h("input", { class: "inp masearch", type: "search", value: m.query || "", placeholder: "Search Music Assistant", "aria-label": "Search Music Assistant",
      oninput: (e) => { const q = e.target.value; clearTimeout(this._maTimer); this._maTimer = setTimeout(() => load(q.trim()), 400); } });
    return h("div", { class: "sec" }, h("h3", null, "Music", h("span", { class: "rinfo" }, this._infoBtn("snd:ma", `From Music Assistant, played on this panel's speaker and getting louder. If the music cannot start, ${defName} rings instead. Music Assistant plays on the panel once the panel's media player is among its players.`))),
      !m.available ? h("div", { class: "empty" }, "Music Assistant is not set up in Home Assistant.") : [
        search,
        m.error ? h("div", { class: "empty" }, m.error) : null,
        m.loading && !m.items.length ? h("div", { class: "empty" }, "Looking…") : null,
        !m.loading && !m.error && !m.items.length ? h("div", { class: "empty" }, m.query ? "Nothing found." : "No playlists or radio stations in Music Assistant.") : null,
        m.items.length ? h("div", { class: "sndlist", role: "radiogroup", "aria-label": "Music" }, m.items.map((x) => item(`ma:${x.uri}`, x.name, x.kind))) : null,
      ]);
  }

  _sheet_alarm(sh, p) {
    const alarms = Array.isArray(p.alarms) ? p.alarms : [];
    const isNew = sh.idx < 0 || !alarms[sh.idx];
    const save = () => {
      const m = /^(\d{1,2}):(\d{2})/.exec(sh.t || "");
      if (!m) { this._toast("Choose a time first.", null, true); return; }
      // the whole list goes to the panel: never without the alarms it has
      if (!Array.isArray(p.alarms)) { this._toast("The panel shows its alarms here once it is connected.", null, true); return; }
      const k = this._soundKey(sh.sound, "birds");
      const tone = k === "lib:birds" ? "auto" : k;
      const a = { h: Number(m[1]), m: Number(m[2]), days: [...sh.days], label: (sh.label || "").trim().slice(0, 20), on: true, tone, tone_name: this._soundInfo(tone, "birds").name };
      const list = clone(alarms);
      if (isNew) list.push(a);
      else list[sh.idx] = { ...list[sh.idx], ...a };
      this._closeSheet();
      this._saveAlarms(p, list, { toast: `Alarm set for ${this._hhmm(a)}` });
    };
    const remove = () => {
      if (!Array.isArray(p.alarms)) { this._toast("The panel shows its alarms here once it is connected.", null, true); return; }
      const list = clone(alarms);
      const [gone] = list.splice(sh.idx, 1);
      this._closeSheet();
      this._saveAlarms(p, list, { toast: "Alarm deleted", undo: () => { const back = clone(p.alarms || []); back.splice(Math.min(sh.idx, back.length), 0, gone); this._saveAlarms(p, back); } });
    };
    return this._sheetFrame(isNew ? "New alarm" : "Alarm", [
      h("div", { style: { display: "flex", justifyContent: "center" } },
        h("input", { class: "inp alarm-time", type: "time", value: sh.t, "aria-label": "Time", oninput: (e) => { sh.t = e.target.value; } })),
      h("div", { class: "sec" }, h("h3", null, "Repeat"), this._weekdays(sh.days, (k) => { sh.days[k] = !sh.days[k]; this._renderLayer(); })),
      h("div", { class: "group" },
        this._row("Name", h("input", { class: "inp short", value: sh.label, maxlength: 20, placeholder: "Alarm", "aria-label": "Name", oninput: (e) => { sh.label = e.target.value; }, onkeydown: (e) => { if (e.key === "Enter") e.target.blur(); } }), { cls: "first" }),
        this._soundRow("Sound", sh.sound, { def: "birds", defNote: "The default for alarms", title: "Alarm sound", ma: true, p, parent: sh }, (v) => { sh.sound = v; })),
    ], [
      isNew ? null : this._btn("Delete", remove, { cls: "ghost danger", icon: "delete-outline" }),
      h("span", { class: "grow" }),
      this._btn("Cancel", () => this._closeSheet(), { cls: "ghost" }),
      this._btn("Save", save, { cls: "primary" })], { center: true });
  }

  _sheet_critSuggest(sh, p) {
    const sugg = this._suggestions();
    return this._sheetFrame("Suggested", [
      h("p", { class: "note", style: { margin: 0 } }, "Sensors whose device class is about safety. Only the ones you add count as critical."),
      h("div", { class: "group" }, sugg.length ? sugg.map((e, k) => this._row(e.name, h("div", { style: { display: "flex", gap: "6px", flexWrap: "nowrap", flex: "none" } },
        this._btn("Not this", () => this._saveHouse((hs) => { hs.dismissed.push(e.id); },
          { toast: `${e.name} is not suggested now`, undo: () => this._saveHouse((hs) => { hs.dismissed = hs.dismissed.filter((x) => x !== e.id); }) }), { cls: "small ghost" }),
        this._btn("Add", () => this._saveHouse((hs) => { hs.critical.push({ entity_id: e.id, on: true, hold: true, off: [] }); }, { toast: `${e.name} is critical now` }), { cls: "small primary", icon: "plus" })),
      { lead: h("span", { class: "lead" }, icon(this._eicon(e.id))), sub: [SAFETY[this._safetyClass(e.id)]?.name, this._areaName(e.area)].filter(Boolean).join(" · "), cls: k === 0 ? "first" : "" }))
        : h("div", { class: "empty", style: { paddingTop: "14px" } }, "Nothing more to suggest.")),
    ], [h("span", { class: "grow" }), this._btn("Done", () => this._closeSheet(), { cls: "primary" })]);
  }

  _sheet_status(sh, p) {
    const house = this._data.house || {};
    const r = (house.status || []).find((x) => x.id === sh.id);
    if (!r) return null;
    const cat = STATUS_CAT[r.cat] || STATUS_CAT.custom;
    const t = r.trigger || { kind: "entity", entity: "", op: "on", value: "", after: "" };
    const set = (patch) => this._saveHouse((hs) => Object.assign(hs.status.find((x) => x.id === r.id), patch));
    const setT = (patch) => set({ trigger: { ...t, ...patch } });
    const pv = this._panelById(sh.pv) || p;
    const own = r.own || r.cat === "custom";
    const domain = t.entity ? t.entity.split(".")[0] : "";
    const numeric = t.entity && !Number.isNaN(Number(this._hass.states[t.entity]?.state)) && ["sensor", "number", "input_number"].includes(domain);
    const binary = ["binary_sensor", "input_boolean", "switch", "light", "fan", "automation"].includes(domain);
    const ops = numeric ? [["above", "Above"], ["below", "Below"]] : binary || !domain ? [["on", "Is on"], ["off", "Is off"]] : [["equals", "Is"]];
    const others = (house.status || []).filter((x) => x.id !== r.id && x.trigger?.kind !== "after");
    const pictos = [...cat.pictos.map((x) => ({ id: x, img: `${this._static}/picto/${x}-144.png` })), { id: `mdi:${cat.mdi}`, mdi: cat.mdi }];
    const trigDomains = ["binary_sensor", "input_boolean", "switch", "sensor", "light", "fan", "vacuum", "lock", "media_player", "cover", "climate", "input_select", "person", "device_tracker"];
    const back = { ...sh, fresh: false };
    const connectedForPv = this._panels.filter((q) => q.connected);
    const picto = r.picto || cat.pictos[0];
    const preview = h("div", { class: "stick" },
      h("div", { class: "stpv", style: { background: r.colour || cat.c } },
        picto.startsWith("mdi:") ? icon(picto) : h("img", { src: `${this._static}/picto/${picto}-144.png`, alt: "" }),
        h("b", null, own && r.headline ? r.headline : cat.name),
        h("small", null, own ? r.line || "" : "The panel picks a line for this kind"),
        r.button ? h("span", { class: "bt" }, r.button_label || "Button text from the panel") : null,
        r.countdown || t.kind === "after" ? h("small", null, `Closes after ${r.countdown || 10} min`) : null),
      h("p", { class: "note", style: { margin: 0, textAlign: "center" } }, "A sketch; the panel draws it in its own style."),
      connectedForPv.length > 1 ? h("div", { class: "sec", style: { alignSelf: "stretch" } }, h("h3", null, "Show now on"), this._choice(connectedForPv.map((q) => [q.entry_id, this._pname(q)]), pv.entry_id, (v) => { sh.pv = v; this._renderLayer(); }, { label: "Show now on" })) : null);
    const form = h("div", { style: { display: "flex", flexDirection: "column", gap: "18px", minWidth: 0 } },
      h("div", { class: "sec" }, h("h3", null, "When"),
        this._seg([["entity", "A device"], ["after", "After a screen"], ["service", "Automation"]], t.kind, (v) => {
          setT({ kind: v, after: v === "after" ? (others[0] || {}).id || "" : t.after });
          // the first time it is started by automations it gets a readable name (e.g. toilet_occupied)
          if (v === "service" && !r.alias) {
            const base = this._statusName(r).toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "").replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "").slice(0, 34) || "screen";
            const used = new Set(((this._data.house || {}).status || []).flatMap((x) => [x.alias, x.id]).filter(Boolean));
            let alias = base;
            for (let n = 2; used.has(alias); n++) alias = `${base}_${n}`;
            set({ alias });
          }
        }, { label: "Trigger" }),
        t.kind === "entity" ? h("div", { class: "group" },
          this._row("Device", this._entityButton(t.entity, { title: "Device", domains: trigDomains, p, back,
            onPick: (id) => { const d = id.split(".")[0]; const st = this._hass.states[id]; const num = ["sensor", "number", "input_number"].includes(d) && !Number.isNaN(Number(st?.state)); const bin = ["binary_sensor", "input_boolean", "switch", "light", "fan", "automation"].includes(d); setT({ entity: id, op: num ? "above" : bin ? "on" : "equals", value: num ? String(st.state) : bin ? "" : String(st?.state || "") }); } }), { cls: "first" }),
          t.entity ? this._row("State", h("div", { style: { display: "flex", gap: "8px", alignItems: "center", flexWrap: "wrap", justifyContent: "flex-end" } },
            this._seg(ops, ops.some(([v]) => v === t.op) ? t.op : ops[0][0], (v) => setT({ op: v }), { label: "State" }),
            ["above", "below", "equals"].includes(t.op) ? this._text(t.value, (v) => setT({ value: v }), { max: 60, label: "Value", cls: "short", placeholder: String(this._hass.states[t.entity]?.state ?? "") }) : null),
          { sub: `Now: ${this._liveValue(`ha:${t.entity}`)}` }) : null) : null,
        t.kind === "after" ? h("div", { class: "group" }, others.length
          ? this._row("Screen", this._dropdown(others.map((x) => [x.id, this._statusName(x)]), t.after, (v) => setT({ after: v }), { label: "Screen" }), { cls: "first" })
          : h("div", { class: "empty", style: { paddingTop: "12px" } }, "Add another status screen first.")) : null,
        t.kind === "service" ? this._statusActions(r, set) : null),
      h("div", { class: "sec" }, h("h3", null, "Kind"), h("div", { class: "catgrid" }, STATUS_CATS.map((c) => h("button", {
        class: `catbtn${c.id === r.cat ? " on" : ""}`, "aria-pressed": c.id === r.cat ? "true" : "false",
        onclick: () => set({ cat: c.id, colour: c.c, picto: c.pictos[0], own: c.id === "custom" ? true : r.cat === "custom" ? false : r.own, countdown: c.countdown && !r.countdown ? 10 : r.countdown }),
      }, h("span", { class: "pc", style: { background: c.c } }, h("img", { src: `${this._static}/picto/${c.pictos[0]}-144.png`, alt: "" })), c.name)))),
      h("div", { class: "sec" }, h("h3", null, "Text", h("span", { class: "rinfo" }, this._infoBtn("st:text", "PanelOS texts: the panel shows a different headline and line from its own set for this kind each time. Own text: always yours."))),
        r.cat !== "custom" ? this._seg([["panelos", "PanelOS texts"], ["own", "Own text"]], r.own ? "own" : "panelos", (v) => set({ own: v === "own" }), { label: "Text" }) : null,
        own ? h("div", { class: "group" },
          this._row("Headline", this._text(r.headline, (v) => set({ headline: v.trim() }), { max: 40, label: "Headline" }), { cls: "first", stack: true }),
          this._row("Line", this._text(r.line, (v) => set({ line: v.trim() }), { max: 80, label: "Line" }), { stack: true })) : null),
      this._has(pv, "images") ? h("div", { class: "sec" }, h("h3", null, "Picture"), h("div", { class: "swatches" }, pictos.map((x) => h("button", {
        class: `picto${picto === x.id ? " on" : ""}`, style: { background: r.colour || cat.c }, "aria-label": x.mdi ? "Icon" : x.id, onclick: () => set({ picto: x.id }),
      }, x.img ? h("img", { src: x.img, alt: "" }) : icon(x.mdi))))) : null,
      h("div", { class: "sec" }, h("h3", null, "Colour"), h("div", { class: "swatches" }, STATUS_COLOURS.map((c) => h("button", { class: `sw${(r.colour || cat.c).toLowerCase() === c.toLowerCase() ? " on" : ""}`, style: { background: c }, "aria-label": c, onclick: () => set({ colour: c }) })))),
      h("div", { class: "group" },
        this._row("Button", this._toggle(r.button, (v) => set({ button: v }), { label: "Button" }), { cls: "first", sub: r.button ? "Closes the screen on that panel" : "No button: it ends by itself" }),
        r.button ? this._row("Button text", this._text(r.button_label, (v) => set({ button_label: v.trim() }), { max: 24, label: "Button text", placeholder: "From the panel" }), { stack: true }) : null,
        this._row("Closes by itself", this._select([...(t.kind === "after" ? [] : [["0", "Never"]]), ["5", "After 5 min"], ["10", "After 10 min"], ["15", "After 15 min"], ["30", "After 30 min"]], String(r.countdown || (t.kind === "after" ? 10 : 0)), (v) => set({ countdown: Number(v) }), { label: "Closes by itself" }), { info: t.kind === "after" ? "The screen shows the time left and closes when it runs out." : "Never: the screen stays while the trigger lasts, or until it is closed on the panel. With a time: the screen shows the time left and closes when it runs out." }),
        this._row("In use", this._toggle(r.on, (v) => set({ on: v }), { label: "In use" }), { sub: r.on ? null : "Switched off: never shown" })),
      h("div", { class: "sec" }, h("h3", null, "Shown on"), this._multi({ label: "Shown on", key: `on:status:${r.id}` }, this._panels.map((q) => {
        const on = r.panels.includes(q.entry_id);
        return this._mi(on, this._pname(q), () => set({ panels: on ? r.panels.filter((x) => x !== q.entry_id) : [...r.panels, q.entry_id] }));
      }))));
    return this._sheetFrame(sh.fresh ? "New status screen" : this._statusName(r), h("div", { class: "split" }, form, preview), [
      this._armed(`del:${r.id}`, "Delete", "Tap again to delete", () => {
        const gone = clone(r);
        this._ui.sheet = null;
        this._saveHouse((hs) => { hs.status = hs.status.filter((x) => x.id !== r.id); }, { toast: "Status screen deleted", undo: () => this._saveHouse((hs) => { hs.status.push(gone); }) });
        this._renderLayer();
      }, { cls: "ghost danger", icon: "delete-outline" }),
      h("span", { class: "grow" }),
      this._btn("Show now", () => { this._ui.sheet = null; this._renderLayer(); this._test(pv, "status", r.id); }, { icon: "eye-outline", disabled: !pv.connected }),
      this._btn("Done", () => this._closeSheet(), { cls: "primary" })], { wide: true });
  }

  // -- copy settings --------------------------------------------------------------------------

  _openCopy(from, pageIndex = null) {
    const others = this._panels.filter((x) => x.entry_id !== from.entry_id).map((x) => x.entry_id);
    this._openSheet({ kind: "copy", from: from.entry_id, to: others.slice(0, 1), page: pageIndex,
      what: pageIndex != null ? { pages: true } : { pages: true, display: true, voice: true, dnd: true, popups: true, dashboard: true } });
  }

  // What copying does on one panel: [y copied / a adapted / n skipped, text, why].
  _copyPlan(from, to, what, pageIndex) {
    const out = [];
    const tn = this._pname(to);
    if (what.pages || pageIndex != null) {
      const src = pageIndex != null ? [this._pages(from)[pageIndex]] : this._pages(from);
      for (const pg of src) {
        if (!pg) continue;
        const t = PAGE_TYPES[pg.kind] || { name: pg.kind };
        const sup = this._pageSupported(to, pg.kind);
        if (!sup.ok) { out.push(["n", `${t.name} page`, sup.why]); continue; }
        const look = this._lookSupported(to, pg.kind, pg.variant || 0);
        if (!look.ok) { out.push(["a", `${t.name}: ${(LOOKS[pg.kind] || [])[pg.variant] || "look"} → ${(LOOKS[pg.kind] || ["the first look"])[0]}`, look.why]); continue; }
        out.push(["y", `${t.name} page, look and options${pg.hidden ? " (hidden)" : ""}`]);
      }
      if (pageIndex != null && this._pages(to).length >= (this._data.max_pages || 12)) out.splice(0, out.length, ["n", "The page", `${tn} has no room for more pages.`]);
    }
    if (pageIndex == null) {
      if (what.display) {
        const s = { ...DEFAULT_SCREENSAVER, ...(from.settings.screensaver || {}) };
        if (s.when === "dark" && !s.light_entity && !this._has(to, "light")) {
          const ha = this._ents(["sensor"], { p: to, filter: (st) => st.attributes?.device_class === "illuminance" }).find((e) => e.area && e.area === this._pareaId(to));
          out.push(["a", ha ? `Screensaver in the dark: light from ${ha.name}` : "Screensaver: sunset to sunrise instead of in the dark", this._reason(to, "light")]);
        } else out.push(["y", "When the screensaver may start"]);
        out.push(["y", this._has(to, "proximity") ? "What switches the Auto theme, wake distance" : "What switches the Auto theme"]);
        if (!to.connected) out.push(["n", "Theme, brightness, screensaver timing and style, waking, child lock", `${tn} is offline; they are the panel's own settings.`]);
        else out.push(["y", "Theme, brightness, screensaver timing and style, waking, child lock"]);
      }
      if (what.voice) out.push(this._has(to, "mic") ? ["y", "Voice look"] : ["n", "Voice look", this._reason(to, "mic")]);
      if (what.dnd) out.push(["y", "Do not disturb"]);
      if (what.popups) out.push(!this._has(to, "speaker") && from.settings.popups?.sound !== false ? ["a", "New notifications, without sound", this._reason(to, "speaker")] : ["y", "New notifications"]);
      if (what.dashboard) out.push(this._has(to, "web") ? ["y", "Dashboard"] : ["n", "Dashboard", this._reason(to, "web")]);
    }
    return out;
  }

  async _applyCopy(from, to, what, pageIndex) {
    const changes = {};
    const max = this._data.max_pages || 12;
    const adapt = (pg) => {
      if (!this._pageSupported(to, pg.kind).ok) return null;
      const c = clone(pg);
      if (!this._lookSupported(to, c.kind, c.variant || 0).ok) c.variant = 0;
      return c;
    };
    if (pageIndex != null) {
      const c = adapt(this._pages(from)[pageIndex]);
      if (c && this._pages(to).length < max) changes.pages = [...clone(this._pages(to)), { ...c, hidden: false }];
    } else if (what.pages) {
      const list = this._pages(from).map(adapt).filter(Boolean).slice(0, max);
      if (list.length) changes.pages = list;
    }
    if (pageIndex == null) {
      if (what.display) {
        const s = { ...DEFAULT_SCREENSAVER, ...(from.settings.screensaver || {}) };
        if (s.when === "dark" && !s.light_entity && !this._has(to, "light")) {
          const ha = this._ents(["sensor"], { p: to, filter: (st) => st.attributes?.device_class === "illuminance" }).find((e) => e.area && e.area === this._pareaId(to));
          if (ha) s.light_entity = ha.id;
          else s.when = "night";
        }
        changes.screensaver = s;
        changes.theme_auto = clone({ ...DEFAULT_THEME_AUTO, ...(from.settings.theme_auto || {}) });
        if (this._has(to, "proximity")) changes.wake_distance = from.settings.wake_distance || "medium";
      }
      if (what.voice && this._has(to, "mic")) {
        changes.voice_look = from.settings.voice_look || 0;
        changes.voice_player = from.settings.voice_player || "selected";
      }
      if (what.dnd) changes.dnd = clone(from.settings.dnd || DEFAULT_DND);
      if (what.popups) {
        changes.popups = clone(from.settings.popups || DEFAULT_POPUPS);
        if (!this._has(to, "speaker")) changes.popups.sound = false;
      }
      if (what.dashboard && this._has(to, "web")) {
        const st = from.settings;
        Object.assign(changes, { dashboard: st.dashboard || "", show_header: Boolean(st.show_header), show_sidebar: Boolean(st.show_sidebar), zoom: st.zoom ?? 100, loading_screen: st.loading_screen ?? true, skip_unused_resources: st.skip_unused !== false, keep_resources: [...(st.keep_resources || [])] });
      }
    }
    let ok = true;
    if (Object.keys(changes).length) ok = await this._options(to, changes, { optimistic: (s) => { if (changes.pages) s.all_pages = changes.pages; } });
    if (ok && pageIndex == null && what.display && to.connected) {
      const fs = from.states || {};
      for (const key of ["theme", "auto_brightness", "brightness", "screensaver", "screensaver_style", "time_format"]) {
        if (fs[key] != null && (key !== "auto_brightness" || this._has(to, "light"))) await this._command(to, key, fs[key]);
      }
      if (fs.proximity_wake != null && this._has(to, "proximity")) await this._command(to, "proximity_wake", fs.proximity_wake);
      if (fs.child_lock != null && this._has(to, "touch")) await this._command(to, "child_lock", fs.child_lock);
    }
    // the backgrounds' switches of the panel (rain on the glass, lightning) go with its pages
    if (ok && what.pages && to.connected) {
      const fs = from.states || {};
      for (const key of ["rain_on_glass", "lightning"]) {
        if (typeof fs[key] === "boolean") await this._command(to, key, fs[key]);
      }
    }
    return ok;
  }

  _sheet_copy(sh) {
    const from = this._panelById(sh.from);
    if (!from) return null;
    const targets = this._panels.filter((x) => x.entry_id !== from.entry_id);
    const page = sh.page != null ? this._pages(from)[sh.page] : null;
    const res = { y: ["check", "y"], a: ["alert-outline", "a"], n: ["close", "n"] };
    const items = [["pages", "Pages, their looks and options"], ["display", "Display and screensaver"], ["voice", "Voice look"], ["dnd", "Do not disturb"], ["popups", "New notifications"], ["dashboard", "Dashboard"]];
    const tos = sh.to.map((id) => this._panelById(id)).filter(Boolean);
    return this._sheetFrame(page ? `Copy ${(PAGE_TYPES[page.kind] || { name: page.kind }).name}` : "Copy settings", [
      h("div", { class: "sec" }, h("h3", null, "From"), this._choice(this._panels.map((x) => [x.entry_id, this._pname(x)]), from.entry_id, (v) => {
        sh.from = v;
        sh.to = sh.to.filter((x) => x !== v);
        if (!sh.to.length) { const other = this._panels.find((x) => x.entry_id !== v); if (other) sh.to = [other.entry_id]; }
        sh.page = null;
        this._renderLayer();
      })),
      h("div", { class: "sec" }, h("h3", null, "To"), this._multi({ label: "To", key: "copy:to" }, targets.map((x) => this._mi(sh.to.includes(x.entry_id), this._pname(x), () => {
        sh.to = sh.to.includes(x.entry_id) ? sh.to.filter((e) => e !== x.entry_id) : [...sh.to, x.entry_id];
        this._renderLayer();
      })))),
      page ? null : h("div", { class: "sec" }, h("h3", null, "What"), h("div", { class: "group" }, items.map(([k, l]) => h("label", { class: "opt" },
        h("input", { type: "checkbox", checked: Boolean(sh.what[k]), onchange: (e) => { sh.what[k] = e.target.checked; this._renderLayer(); } }), this._ckBox(Boolean(sh.what[k]), l), h("span", { class: "t" }, h("span", null, l)))))),
      page ? null : sh.what.pages ? h("p", { class: "note", style: { margin: 0 } }, "Copying the pages replaces the pages of the panels it goes to.") : null,
      tos.map((to) => {
        const plan = this._copyPlan(from, to, sh.what, sh.page);
        const copied = plan.filter((x) => x[0] !== "n").length;
        const skipped = plan.length - copied;
        return h("div", { class: "sec" }, h("h3", null, `On ${this._pname(to)}`, h("span", { class: "faint", style: { fontWeight: 400, fontSize: "13px" } }, ` · ${copied} copied${skipped ? `, ${skipped} skipped` : ""}`)),
          h("div", { class: "group" }, plan.length ? plan.map(([k, text, why]) => h("div", { class: "result" }, icon(res[k][0], res[k][1]), h("div", null, text, why ? h("small", null, why) : null))) : h("div", { class: "empty", style: { paddingTop: "12px" } }, "Nothing chosen.")));
      }),
    ], [this._btn("Cancel", () => this._closeSheet(), { cls: "ghost" }),
      this._btn(tos.length ? `Copy to ${tos.length === 1 ? this._pname(tos[0]) : `${tos.length} panels`}` : "Choose a panel", async () => {
        this._ui.sheet = null;
        this._renderLayer();
        let ok = true;
        for (const to of tos) ok = (await this._applyCopy(from, to, sh.what, sh.page)) && ok;
        if (ok) this._toast(`Copied to ${tos.map((x) => this._pname(x)).join(" and ")}`);
      }, { cls: "primary", disabled: !tos.length })], { wide: true });
  }

  // -- add a panel (the integration's config flow) ---------------------------------------------

  _startAdd() {
    const used = new Set(this._panels.map((x) => x.settings?.panel_id));
    this._openSheet({ kind: "add", step: 1, panelId: used.has("nspanel-pro") ? "" : "nspanel-pro", start: "defaults", copyFrom: this._panels[0]?.entry_id || null, error: null, busy: false });
  }

  async _finishAdd(sh) {
    const panelId = sh.panelId.trim();
    if (!panelId) { sh.error = "Enter the panel's ID."; this._renderLayer(); return; }
    sh.busy = true;
    sh.error = null;
    this._renderLayer();
    try {
      const flow = await this._hass.callApi("POST", "config/config_entries/flow", { handler: "panelos", show_advanced_options: false });
      let result = flow;
      if (flow.type === "form") result = await this._hass.callApi("POST", `config/config_entries/flow/${flow.flow_id}`, { panel_id: panelId });
      if (result.type === "abort") throw new Error(result.reason === "already_configured" ? "This panel is already set up." : `Not added (${result.reason}).`);
      if (result.type !== "create_entry") throw new Error("Home Assistant asked for more than this page can answer; add the panel in Settings → Devices and services.");
      const entryId = result.result?.entry_id;
      this._ui.sheet = null;
      this._renderLayer();
      this._toast(`${panelId} added`);
      let fresh = null;
      for (let k = 0; k < 30 && entryId; k++) {
        fresh = this._panelById(entryId);
        if (fresh) break;
        await new Promise((res) => setTimeout(res, 300));
      }
      const src = this._panelById(sh.copyFrom);
      if (fresh && sh.start === "copy" && src) await this._applyCopy(src, fresh, { pages: true, display: true, voice: true, dnd: true, popups: true, dashboard: true }, null);
      if (fresh) this._go(this._panelPath(fresh));
    } catch (err) {
      sh.busy = false;
      sh.error = err.body?.message || err.message || String(err);
      this._renderLayer();
    }
  }

  _sheet_add(sh) {
    const steps = h("div", { class: "steps" }, [1, 2].map((k) => h("i", { class: k <= sh.step ? "on" : "" })));
    let body;
    let foot;
    if (sh.step === 1) {
      body = [steps,
        h("div", { class: "sec" }, h("h3", null, "Panel ID", h("span", { class: "rinfo" }, this._infoBtn("add:id", "The panel's network name, as its PanelOS settings show it. The panel then finds its settings here by this name."))),
          h("input", { class: "inp", value: sh.panelId, maxlength: 64, "aria-label": "Panel ID", oninput: (e) => { sh.panelId = e.target.value; const nb = this._layer.querySelector("[data-next]"); if (nb) nb.disabled = !sh.panelId.trim(); }, onkeydown: (e) => { if (e.key === "Enter" && sh.panelId.trim()) { sh.step = 2; this._renderLayer(); } } })),
        h("p", { class: "note", style: { margin: 0 } }, "Start PanelOS on the panel and connect it to Wi-Fi and Home Assistant first.")];
      foot = [this._btn("Cancel", () => this._closeSheet(), { cls: "ghost" }), this._btn("Next", () => { if (sh.panelId.trim()) { sh.step = 2; this._renderLayer(); } }, { cls: "primary", disabled: !sh.panelId.trim(), attrs: { "data-next": "1" } })];
    } else {
      body = [steps,
        h("div", { class: "sec" }, h("h3", null, "Start with"), h("div", { class: "group" },
          h("label", { class: "opt" }, h("input", { type: "radio", name: "start", checked: sh.start === "defaults", onchange: () => { sh.start = "defaults"; this._renderLayer(); } }), this._ckBox(sh.start === "defaults", "PanelOS defaults", { round: true }), h("span", { class: "t" }, h("span", null, "PanelOS defaults"), h("small", null, "The usual pages, ready to change"))),
          this._panels.length ? h("label", { class: "opt" }, h("input", { type: "radio", name: "start", checked: sh.start === "copy", onchange: () => { sh.start = "copy"; this._renderLayer(); } }), this._ckBox(sh.start === "copy", "A copy of another panel", { round: true }), h("span", { class: "t" }, h("span", null, "A copy of another panel"), h("small", null, "Pages, display, voice, do not disturb, notifications, dashboard"))) : null)),
        sh.start === "copy" ? h("div", { class: "sec" }, h("h3", null, "Copy from"), this._choice(this._panels.map((x) => [x.entry_id, this._pname(x)]), sh.copyFrom, (v) => { sh.copyFrom = v; this._renderLayer(); }, { label: "Copy from" })) : null,
        sh.error ? h("p", { class: "note", role: "alert", style: { margin: 0, color: "var(--bad)" } }, sh.error) : null];
      foot = [this._btn("Back", () => { sh.step = 1; this._renderLayer(); }, { cls: "ghost" }), this._btn(sh.busy ? "Adding…" : `Add ${sh.panelId.trim()}`, () => this._finishAdd(sh), { cls: "primary", icon: "check", disabled: sh.busy })];
    }
    return this._sheetFrame("Add a panel", body, foot, { center: true });
  }
}

// a tab left open over an update loads the new script beside the old one: the element stays the one
// already defined until the page is reloaded
if (!customElements.get("panelos-page")) customElements.define("panelos-page", PanelOSPage);
