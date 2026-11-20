"""Constants for the PanelOS integration."""

DOMAIN = "panelos"

CONF_PANEL_ID = "panel_id"
CONF_WEATHER = "weather_entity"
CONF_MEDIA_PLAYER = "media_player_entity"
CONF_TILES = "tile_entities"
CONF_SHOW_CONVERSATION = "show_conversation"
# The voice view's look: 0 plain, 1 terminal, 2 comic, 3 oscilloscope.
CONF_VOICE_LOOK = "voice_look"
# The player music commands said to the panel act on: the one picked on its music page (default) or the panel itself.
CONF_VOICE_PLAYER = "voice_player"
VOICE_PLAYERS = ("selected", "self")

# Dashboard mode: the Home Assistant dashboard in the panel's web view.
CONF_DASHBOARD = "dashboard"
CONF_SHOW_HEADER = "show_header"
CONF_SHOW_SIDEBAR = "show_sidebar"
# Card resources (Lovelace resources): Home Assistant loads every resource on
# every dashboard, used or not. With "skip unused" on (default) the panel's
# web view leaves out the ones the dashboard it shows does not need; worked
# out again whenever the dashboard or its configuration changes. Resources
# in "keep" are always loaded (when the guess misses one).
CONF_SKIP_UNUSED = "skip_unused_resources"
CONF_KEEP_RESOURCES = "keep_resources"
# Size of the dashboard in the web view, percent.
CONF_ZOOM = "zoom"
CONF_LOADING_SCREEN = "loading_screen"
ZOOM_RANGE = (50, 150, 5)
# The dashboard chosen on the panel itself (its settings or its Dashboard
# entity) applies while this is selected.
DASHBOARD_FROM_PANEL = ""

# How much the panel's recorded sensors must change before a new value is
# sent (each value is a row in the recorder's database).
CONF_ILLUMINANCE_CHANGE = "illuminance_change"
CONF_SOC_TEMPERATURE_CHANGE = "soc_temperature_change"
CONF_WIFI_SIGNAL_CHANGE = "wifi_signal_change"
# key: (default, min, max, step, unit)
REPORTING = {
    CONF_ILLUMINANCE_CHANGE: (15, 1, 100, 1, "%"),
    CONF_SOC_TEMPERATURE_CHANGE: (1.0, 0.1, 10, 0.1, "°C"),
    CONF_WIFI_SIGNAL_CHANGE: (3, 1, 20, 1, "dB"),
}

DEFAULT_PANEL_ID = "nspanel-pro"

# Entities the assistant mode's quick tiles can show.
TILE_DOMAINS = ("light", "switch", "cover", "fan", "input_boolean", "scene", "script", "sensor", "climate")

# The panel's pages, in the order they are swiped: each a kind, its look
# (variant), the entities it shows and its own switches. The same kind may
# come more than once (several weather pages); the first page is the start
# page. A page can be hidden: it keeps its settings here but is not sent.
CONF_PAGES = "pages"
MAX_PAGES = 12
# kind: (number of looks, entity domains, how many entities). "home" is the
# weather page (clock, weather and widgets on a living background); its
# entities are what its widgets show (weather, calendars, player).
PAGE_KINDS = {
    "home": (1, ("weather", "calendar", "media_player"), 10),
    "weather": (2, ("weather",), 1),
    "clock": (8, (), 0),
    "music": (5, ("media_player",), 1),
    "timers": (5, (), 0),
    "thermostat": (4, ("climate",), 8),
    "quick": (3, TILE_DOMAINS, 16),
    "camera": (1, ("camera",), 8),
    "calendar": (4, ("calendar",), 8),
    "todo": (5, ("todo",), 8),
    "info": (4, (), 0),
}
DEFAULT_PAGES = [{"kind": kind} for kind in ("home", "weather", "music", "timers")]

# When the screensaver may start (the idle time is the panel's own
# "screensaver" setting): when the room is dark (darker than `lux`, from the
# panel's light sensor or `light_entity`), between two times, from sunset to
# sunrise ("night") or any time.
CONF_SCREENSAVER = "screensaver"
SCREENSAVER_WHEN = ("dark", "window", "night", "any")
# 3 lx: measured on the panel's own sensor, a lamp-lit evening room reads
# 5-26 lx and a dark room about 1 lx.
DEFAULT_SCREENSAVER = {"when": "dark", "lux": 3, "from": "22:00", "to": "06:00", "light_entity": ""}

# What switches the panel's Auto theme between dark and light: the light in the room (the panel's sensor; Home
# Assistant's sun without one), the sun (dark from sunset to sunrise) or set times (dark from - to).
CONF_THEME_AUTO = "theme_auto"
THEME_AUTO_BY = ("light", "sun", "times")
DEFAULT_THEME_AUTO = {"by": "light", "from": "20:00", "to": "07:00"}

# Wi-Fi roaming: the panel moves by itself to a clearly stronger access point of its network (default on).
CONF_WIFI_ROAMING = "wifi_roaming"

# How close someone comes before the proximity sensor wakes the panel: near (about 30 cm), medium (45 cm) or far
# (55 cm; noisy readings shorten it).
CONF_WAKE_DISTANCE = "wake_distance"
WAKE_DISTANCES = ("near", "medium", "far")
DEFAULT_WAKE_DISTANCE = "medium"

# Do not disturb: off, on, or on a schedule; what still comes through
# (critical alerts always do).
CONF_DND = "dnd"
DND_MODES = ("off", "on", "schedule")
DND_ALLOW = ("alarms", "timers", "status", "ring", "voice", "calls")
DEFAULT_DND = {"mode": "off", "from": "22:00", "to": "07:00", "allow": dict.fromkeys(DND_ALLOW, True)}

# New notifications: a strip at the top, a card, or only into the list.
CONF_POPUPS = "popups"
POPUP_KINDS = ("strip", "card", "off")
DEFAULT_POPUPS = {"kind": "card", "sound": True, "seconds": 10, "until": False}

# What a panel can do, as it reports it (device report "caps"); until a panel
# reports them it counts as an NSPanel Pro.
CONF_CAPS = "caps"
DEFAULT_CAPS = ("touch", "images", "web", "video", "mic", "speaker", "proximity", "light")

# Critical alerts and status screens are shared by every panel (one store for
# the integration, each item with its panels).
HOUSE_STORE = "panelos.house"
# PanelOS's own notifications (notify entities, panelos.notify), kept until answered
NOTES_STORE = "panelos.notes"
# Status screen kinds; the panel has its own texts and pictures for each.
STATUS_CATS = (
    "occupied", "aftermath", "bathroom", "meeting", "focus", "sleeping", "movie",
    "gaming", "vacuum", "cooking", "workout", "keepout", "custom",
)
# Device classes whose binary sensors are suggested as critical alerts.
SAFETY_CLASSES = ("smoke", "carbon_monoxide", "gas", "moisture", "problem", "tamper", "safety")
SERVICE_SHOW_STATUS = "show_status"
SERVICE_CLEAR_STATUS = "clear_status"
SERVICE_CRITICAL_ALERT = "critical_alert"
SERVICE_CLEAR_CRITICAL_ALERT = "clear_critical_alert"

# Sounds: the designed ones the panel has (ids as in the sound values "lib:<id>"), and the
# uploaded ones ("own:<id>"), converted to 16-bit PCM WAV for the panels and kept here.
SOUND_LIB = ("chimes", "marimba", "birds", "bell", "digital", "synth", "sos", "warning")
SOUND_DIR = "panelos/sounds"
SOUND_UPLOAD_MAX = 10 * 1024 * 1024
# the longest part of an uploaded sound kept (seconds; it loops while it rings)
SOUND_KEEP_SECONDS = 60

SERVICE_SHOW_VIEW = "show_view"
SERVICE_NOTIFY = "notify"
# a button of a PanelOS notification pressed on a panel
EVENT_NOTIFICATION_ACTION = "panelos_notification_action"
VIEWS = ["home", "dashboard", "weather", "media", "camera"]

# Internal signals; the panels follow them through panelos/subscribe.
SIGNAL_CONFIG_UPDATED = "panelos_config_updated"
SIGNAL_SHOW = "panelos_show"

# What the panel last told about itself (name, model, version, MAC), kept in
# the entry so the device is complete before the panel connects.
CONF_DEVICE = "device"

# The panel's own device: its entities are fed by panelos/report and send
# their commands back through panelos/subscribe. The voice satellite stays
# on the ESPHome device, which is linked to this one ("Connected via").
PLATFORMS = ["binary_sensor", "button", "notify", "number", "select", "sensor", "switch"]

# Entity keys per platform, as the panel names them in panelos/report and in
# the commands it gets back.
ENTITY_KEYS = {
    "switch": ("screen", "auto_brightness", "proximity_wake", "child_lock"),
    "select": ("dashboard", "theme", "screensaver", "screensaver_style", "display_mode"),
    "number": ("brightness",),
    "sensor": ("illuminance", "soc_temperature", "wifi_signal", "ip_address", "app_version"),
    "binary_sensor": ("presence",),
    "button": ("restart_app", "reboot"),
}

# Until PanelOS 0.0.3 these entities were on the panel's ESPHome device: the
# ESPHome platform and entity name of each, to take their registry entries
# over (entity ids, history and settings stay).
ESPHOME_ENTITIES = {
    "screen": ("switch", "Screen"),
    "auto_brightness": ("switch", "Automatic brightness"),
    "proximity_wake": ("switch", "Wake on approach"),
    "dashboard": ("select", "Dashboard"),
    "theme": ("select", "Theme"),
    "screensaver": ("select", "Screensaver after"),
    "screensaver_style": ("select", "Screensaver style"),
    "display_mode": ("select", "Display mode"),
    "brightness": ("number", "Brightness"),
    "illuminance": ("sensor", "Illuminance"),
    "soc_temperature": ("sensor", "SoC temperature"),
    "wifi_signal": ("sensor", "Wi-Fi signal"),
    "ip_address": ("text_sensor", "IP address"),
    "app_version": ("text_sensor", "App version"),
    "presence": ("binary_sensor", "Presence"),
    "restart_app": ("button", "Restart app"),
    "reboot": ("button", "Reboot"),
}

SIGNAL_LINKS = "panelos_links"
# Something the sidebar page shows changed (a report, a link).
SIGNAL_PAGE = "panelos_page"
