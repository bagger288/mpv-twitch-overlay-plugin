#!/usr/bin/env python3
"""
Twitch Stream & Chat Overlay Launcher (Tkinter GUI)
- Scans records/ for recorded channels and sessions.
- Displays metadata: parts count, duration, size, session integrity.
- Launches mpv non-blockingly with multi-part playlist, twitch_chat.lua overlay,
  custom initial offset, and emote options.
- Includes Import Tab for bestlogs.lurkology.com and Live Recording Tab.
"""

import datetime
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import threading
import tkinter as tk
from tkinter import ttk, messagebox, filedialog
from typing import Dict, List, Optional, Tuple

SCRIPT_DIR = Path(__file__).resolve().parent
CONFIG_FILE = SCRIPT_DIR / "config.json"
LUA_SCRIPT = SCRIPT_DIR / "twitch_chat.lua"
RUST_DLL = SCRIPT_DIR / "twitch_chat.dll"
RUST_DLL_TARGET = SCRIPT_DIR / "rust_plugin" / "target" / "release" / "twitch_chat.dll"


def load_config() -> dict:
    if CONFIG_FILE.exists():
        try:
            with open(CONFIG_FILE, "r", encoding="utf-8") as f:
                return json.load(f)
        except Exception:
            pass
    return {
        "streamlink_path": "streamlink",
        "mpv_path": "mpv",
        "python_path": "python",
        "default_chat_enabled": True,
        "default_emotes_enabled": True,
        "default_offset_ms": 0,
        "records_dir": "records",
        "emote_cache_dir": "emote_cache",
    }


def save_config(cfg: dict):
    try:
        with open(CONFIG_FILE, "w", encoding="utf-8") as f:
            json.dump(cfg, f, ensure_ascii=False, indent=2)
    except Exception as e:
        print(f"Error saving config: {e}")


def format_duration(seconds: float) -> str:
    if seconds <= 0:
        return "0m 00s"
    h = int(seconds // 3600)
    m = int((seconds % 3600) // 60)
    s = int(seconds % 60)
    if h > 0:
        return f"{h}h {m:02d}m {s:02d}s"
    return f"{m}m {s:02d}s"


def format_bytes(num_bytes: int) -> str:
    if num_bytes < 1024 * 1024:
        return f"{num_bytes / 1024:.1f} KB"
    elif num_bytes < 1024 * 1024 * 1024:
        return f"{num_bytes / (1024 * 1024):.1f} MB"
    else:
        return f"{num_bytes / (1024 * 1024 * 1024):.2f} GB"


def parse_channel_from_folder(folder_name: str) -> Tuple[str, Optional[datetime.datetime]]:
    """
    Extracts channel name from folder name format: <channel>_<dd-mm-yyyy_hh-mm-ss>
    Channel name can contain underscores!
    """
    pattern = r"^(.*)_(\d{2}-\d{2}-\d{4}_\d{2}-\d{2}-\d{2})$"
    m = re.match(pattern, folder_name)
    if m:
        channel = m.group(1)
        ts_str = m.group(2)
        try:
            dt = datetime.datetime.strptime(ts_str, "%d-%m-%Y_%H-%M-%S")
            return channel, dt
        except ValueError:
            pass
    return folder_name, None


class TwitchRecordsApp:
    def __init__(self, root: tk.Tk):
        self.root = root
        self.root.title("Twitch Stream & Chat Overlay Launcher")
        self.root.geometry("980x640")
        self.root.minsize(800, 500)

        self.config = load_config()
        self.records_base = SCRIPT_DIR / self.config.get("records_dir", "records")
        self.records_base.mkdir(parents=True, exist_ok=True)

        self.channels_data: Dict[str, List[dict]] = {}
        self.selected_channel: Optional[str] = None
        self.active_recording_proc: Optional[subprocess.Popen] = None

        self._apply_theme()
        self._build_ui()
        self.refresh_records()

    def _apply_theme(self):
        self.style = ttk.Style()
        try:
            self.style.theme_use("clam")
        except Exception:
            pass

        # Dark sleek Twitch-inspired palette
        self.bg_color = "#18181b"
        self.card_bg = "#1f1f23"
        self.input_bg = "#26262c"
        self.input_focus = "#303038"
        self.accent_purple = "#9146ff"
        self.text_color = "#efeff1"
        self.text_dim = "#adadb8"

        self.root.configure(bg=self.bg_color)
        self.root.protocol("WM_DELETE_WINDOW", self._on_close)

        # Global Tk option fallbacks for any non-ttk or sub-widgets
        self.root.option_add("*TEntry*fieldBackground", self.input_bg)
        self.root.option_add("*TEntry*foreground", "#ffffff")
        self.root.option_add("*TEntry*insertColor", "#ffffff")
        self.root.option_add("*TCombobox*fieldBackground", self.input_bg)
        self.root.option_add("*TCombobox*foreground", "#ffffff")
        self.root.option_add("*TCombobox*Listbox*Background", self.input_bg)
        self.root.option_add("*TCombobox*Listbox*Foreground", "#ffffff")
        self.root.option_add("*Entry*background", self.input_bg)
        self.root.option_add("*Entry*foreground", "#ffffff")
        self.root.option_add("*Entry*insertBackground", "#ffffff")
        self.root.option_add("*Text*background", "#141417")
        self.root.option_add("*Text*foreground", self.text_color)
        self.root.option_add("*Text*insertBackground", "#ffffff")

        # Base TTK styles
        self.style.configure(".", background=self.bg_color, foreground=self.text_color, font=("Segoe UI", 10))
        self.style.configure("TLabel", background=self.bg_color, foreground=self.text_color)
        self.style.configure("TFrame", background=self.bg_color)
        self.style.configure("Card.TFrame", background=self.card_bg)
        self.style.configure("TNotebook", background=self.bg_color, borderwidth=0)
        self.style.configure("TNotebook.Tab", background=self.card_bg, foreground=self.text_color, padding=[12, 6])
        self.style.map("TNotebook.Tab", background=[("selected", self.accent_purple)])
        self.style.configure("Accent.TButton", background=self.accent_purple, foreground="#ffffff", font=("Segoe UI", 10, "bold"), padding=[12, 6])
        self.style.map("Accent.TButton", background=[("active", "#772ce8")])

        # Crisp Dark Text Entries (fixes white-on-white bug!)
        self.style.configure(
            "TEntry",
            fieldbackground=self.input_bg,
            foreground="#ffffff",
            insertcolor="#ffffff",
            bordercolor="#44444e",
            lightcolor=self.accent_purple,
            darkcolor="#44444e",
            padding=5,
        )
        self.style.map(
            "TEntry",
            fieldbackground=[("focus", self.input_focus), ("disabled", "#1c1c20")],
            foreground=[("disabled", "#70707a"), ("focus", "#ffffff")],
        )

        # Crisp Dark Combobox (fixes white-on-white bug!)
        self.style.configure(
            "TCombobox",
            fieldbackground=self.input_bg,
            background=self.card_bg,
            foreground="#ffffff",
            arrowcolor="#ffffff",
            bordercolor="#44444e",
            padding=5,
        )
        self.style.map(
            "TCombobox",
            fieldbackground=[("readonly", self.input_bg), ("focus", self.input_focus)],
            foreground=[("readonly", "#ffffff"), ("focus", "#ffffff")],
            selectbackground=[("readonly", self.accent_purple)],
            selectforeground=[("readonly", "#ffffff")],
        )

        # Standard Buttons
        self.style.configure(
            "TButton",
            background="#2d2d35",
            foreground="#ffffff",
            padding=[10, 5],
        )
        self.style.map(
            "TButton",
            background=[("active", "#3b3b44"), ("pressed", "#444450")],
            foreground=[("active", "#ffffff")],
        )

        # Checkbuttons
        self.style.configure("TCheckbutton", background=self.bg_color, foreground=self.text_color)
        self.style.map("TCheckbutton", background=[("active", self.bg_color)], foreground=[("active", "#ffffff")])

        # Treeview table
        self.style.configure(
            "Treeview",
            background=self.card_bg,
            foreground=self.text_color,
            fieldbackground=self.card_bg,
            rowheight=26,
        )
        self.style.map("Treeview", background=[("selected", self.accent_purple)], foreground=[("selected", "#ffffff")])
        self.style.configure("Treeview.Heading", background="#2b2b30", foreground="#ffffff", font=("Segoe UI", 9, "bold"))

    def _build_ui(self):
        # Header bar
        header = ttk.Frame(self.root, padding=12)
        header.pack(fill=tk.X)

        title = tk.Label(
            header,
            text="Twitch Stream & Chat Player",
            font=("Segoe UI", 15, "bold"),
            bg=self.bg_color,
            fg="#bf94ff",
        )
        title.pack(side=tk.LEFT)

        subtitle = tk.Label(
            header,
            text="mpv Overlay • 7TV/BTTV/FFZ Emotes • Badges • Log Importer",
            font=("Segoe UI", 9),
            bg=self.bg_color,
            fg=self.text_dim,
        )
        subtitle.pack(side=tk.LEFT, padx=12, pady=4)

        # Tabbed Notebook
        self.notebook = ttk.Notebook(self.root)
        self.notebook.pack(fill=tk.BOTH, expand=True, padx=12, pady=(0, 12))

        # Tab 1: Recordings & Player
        self.tab_records = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(self.tab_records, text=" 📺 Записи каналов ")

        # Tab 2: Importer (bestlogs)
        self.tab_import = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(self.tab_import, text=" 📥 Импорт логов ")

        # Tab 3: Live Recorder
        self.tab_record = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(self.tab_record, text=" 🔴 Запись стрима ")

        # Tab 4: Settings
        self.tab_settings = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(self.tab_settings, text=" ⚙️ Настройки ")

        self._build_records_tab()
        self._build_import_tab()
        self._build_recorder_tab()
        self._build_settings_tab()

    def _build_records_tab(self):
        paned = tk.PanedWindow(self.tab_records, orient=tk.HORIZONTAL, bg=self.card_bg, sashwidth=4)
        paned.pack(fill=tk.BOTH, expand=True)

        # Left pane: Channels List
        left_frame = ttk.Frame(paned, padding=8)
        paned.add(left_frame, minsize=200, width=240)

        ch_header = ttk.Label(left_frame, text="Каналы", font=("Segoe UI", 11, "bold"))
        ch_header.pack(anchor=tk.W, pady=(0, 6))

        self.channel_listbox = tk.Listbox(
            left_frame,
            bg=self.card_bg,
            fg=self.text_color,
            selectbackground=self.accent_purple,
            selectforeground="#ffffff",
            highlightthickness=0,
            borderwidth=0,
            font=("Segoe UI", 10),
        )
        self.channel_listbox.pack(fill=tk.BOTH, expand=True)
        self.channel_listbox.bind("<<ListboxSelect>>", self._on_channel_selected)

        # Right pane: Recordings table & launch bar
        right_frame = ttk.Frame(paned, padding=8)
        paned.add(right_frame, minsize=500)

        rec_header_frame = ttk.Frame(right_frame)
        rec_header_frame.pack(fill=tk.X, pady=(0, 6))

        self.rec_title_lbl = ttk.Label(rec_header_frame, text="Выберите канал", font=("Segoe UI", 11, "bold"))
        self.rec_title_lbl.pack(side=tk.LEFT)

        btn_refresh = ttk.Button(rec_header_frame, text="🔄 Обновить", command=self.refresh_records)
        btn_refresh.pack(side=tk.RIGHT)

        btn_open_folder = ttk.Button(rec_header_frame, text="📁 Папка records", command=self._open_records_folder)
        btn_open_folder.pack(side=tk.RIGHT, padx=6)

        # Treeview for recordings
        columns = ("date", "parts", "duration", "size", "chat", "emotes")
        self.rec_tree = ttk.Treeview(right_frame, columns=columns, show="headings", selectmode="browse")
        self.rec_tree.heading("date", text="Дата и время начала")
        self.rec_tree.heading("parts", text="Части")
        self.rec_tree.heading("duration", text="Длительность")
        self.rec_tree.heading("size", text="Размер")
        self.rec_tree.heading("chat", text="Чат")
        self.rec_tree.heading("emotes", text="Эмоуты")

        self.rec_tree.column("date", width=180)
        self.rec_tree.column("parts", width=60, anchor=tk.CENTER)
        self.rec_tree.column("duration", width=100, anchor=tk.CENTER)
        self.rec_tree.column("size", width=80, anchor=tk.E)
        self.rec_tree.column("chat", width=60, anchor=tk.CENTER)
        self.rec_tree.column("emotes", width=60, anchor=tk.CENTER)

        tree_scroll = ttk.Scrollbar(right_frame, orient=tk.VERTICAL, command=self.rec_tree.yview)
        self.rec_tree.configure(yscrollcommand=tree_scroll.set)

        self.rec_tree.pack(side=tk.TOP, fill=tk.BOTH, expand=True)
        tree_scroll.pack(side=tk.RIGHT, fill=tk.Y)
        self.rec_tree.bind("<Double-1>", lambda e: self.launch_player())

        # Chat appearance & sizing bar
        chat_style_bar = ttk.Frame(right_frame, padding=(0, 6, 0, 0))
        chat_style_bar.pack(fill=tk.X, side=tk.BOTTOM)

        ttk.Label(chat_style_bar, text="Шрифт:").pack(side=tk.LEFT)
        self.combo_font = ttk.Combobox(
            chat_style_bar,
            values=["Segoe UI", "Arial", "Tahoma", "Verdana", "DejaVu Sans", "Consolas"],
            width=12,
            state="readonly",
        )
        self.combo_font.set(self.config.get("chat_font_family", "Segoe UI"))
        self.combo_font.pack(side=tk.LEFT, padx=4)

        ttk.Label(chat_style_bar, text="Размер:").pack(side=tk.LEFT, padx=(6, 0))
        self.combo_font_sz = ttk.Combobox(
            chat_style_bar,
            values=["13", "14", "15", "16", "17", "18", "20", "22", "24", "28"],
            width=4,
        )
        self.combo_font_sz.set(str(self.config.get("chat_font_size", 17)))
        self.combo_font_sz.pack(side=tk.LEFT, padx=4)

        ttk.Label(chat_style_bar, text="ШxВ:").pack(side=tk.LEFT, padx=(6, 0))
        self.entry_chat_w = ttk.Entry(chat_style_bar, width=5)
        self.entry_chat_w.insert(0, str(self.config.get("chat_width", 420)))
        self.entry_chat_w.pack(side=tk.LEFT, padx=2)

        ttk.Label(chat_style_bar, text="x").pack(side=tk.LEFT)
        self.entry_chat_h = ttk.Entry(chat_style_bar, width=5)
        self.entry_chat_h.insert(0, str(self.config.get("chat_height", 600)))
        self.entry_chat_h.pack(side=tk.LEFT, padx=2)

        ttk.Label(chat_style_bar, text="X,Y:").pack(side=tk.LEFT, padx=(6, 0))
        self.entry_chat_x = ttk.Entry(chat_style_bar, width=5)
        self.entry_chat_x.insert(0, str(self.config.get("chat_x", 24)))
        self.entry_chat_x.pack(side=tk.LEFT, padx=2)

        self.entry_chat_y = ttk.Entry(chat_style_bar, width=5)
        self.entry_chat_y.insert(0, str(self.config.get("chat_y", 60)))
        self.entry_chat_y.pack(side=tk.LEFT, padx=2)

        ttk.Label(chat_style_bar, text="Фон:").pack(side=tk.LEFT, padx=(6, 0))
        self.combo_bg_opacity = ttk.Combobox(
            chat_style_bar,
            values=["100%", "85%", "75%", "60%", "40%", "20%", "0%"],
            width=5,
            state="readonly",
        )
        cur_op = self.config.get("chat_bg_opacity", 75)
        if isinstance(cur_op, float) and cur_op <= 1.0:
            cur_op = int(round(cur_op * 100))
        self.combo_bg_opacity.set(f"{cur_op}%" if f"{cur_op}%" in ["100%", "85%", "75%", "60%", "40%", "20%", "0%"] else "75%")
        self.combo_bg_opacity.pack(side=tk.LEFT, padx=2)

        def set_corner(pos):
            if pos == "TL":
                self.entry_chat_x.delete(0, tk.END); self.entry_chat_x.insert(0, "24")
                self.entry_chat_y.delete(0, tk.END); self.entry_chat_y.insert(0, "60")
            elif pos == "BL":
                self.entry_chat_x.delete(0, tk.END); self.entry_chat_x.insert(0, "24")
                self.entry_chat_y.delete(0, tk.END); self.entry_chat_y.insert(0, "440")
            elif pos == "TR":
                self.entry_chat_x.delete(0, tk.END); self.entry_chat_x.insert(0, "1470")
                self.entry_chat_y.delete(0, tk.END); self.entry_chat_y.insert(0, "60")
            elif pos == "BR":
                self.entry_chat_x.delete(0, tk.END); self.entry_chat_x.insert(0, "1470")
                self.entry_chat_y.delete(0, tk.END); self.entry_chat_y.insert(0, "440")

        ttk.Button(chat_style_bar, text="↖ ВЛ", width=4, command=lambda: set_corner("TL")).pack(side=tk.LEFT, padx=(6, 1))
        ttk.Button(chat_style_bar, text="↙ НЛ", width=4, command=lambda: set_corner("BL")).pack(side=tk.LEFT, padx=1)
        ttk.Button(chat_style_bar, text="↗ ВП", width=4, command=lambda: set_corner("TR")).pack(side=tk.LEFT, padx=1)
        ttk.Button(chat_style_bar, text="↘ НП", width=4, command=lambda: set_corner("BR")).pack(side=tk.LEFT, padx=1)

        # Launch Bar at bottom
        launch_bar = ttk.Frame(right_frame, padding=(0, 10, 0, 0))
        launch_bar.pack(fill=tk.X, side=tk.BOTTOM)

        self.var_chat = tk.BooleanVar(value=self.config.get("default_chat_enabled", True))
        chk_chat = ttk.Checkbutton(launch_bar, text="Включить чат", variable=self.var_chat)
        chk_chat.pack(side=tk.LEFT, padx=(0, 10))

        self.var_emotes = tk.BooleanVar(value=self.config.get("default_emotes_enabled", True))
        chk_emotes = ttk.Checkbutton(launch_bar, text="Эмоуты и значки (Режим 2)", variable=self.var_emotes)
        chk_emotes.pack(side=tk.LEFT, padx=(0, 14))

        ttk.Label(launch_bar, text="Начальный сдвиг (сек):").pack(side=tk.LEFT)
        self.entry_offset = ttk.Entry(launch_bar, width=6)
        self.entry_offset.insert(0, str(self.config.get("default_offset_ms", 0) / 1000.0))
        self.entry_offset.pack(side=tk.LEFT, padx=6)

        ttk.Label(launch_bar, text="Движок:").pack(side=tk.LEFT, padx=(10, 2))
        self.var_engine = tk.StringVar(value=self.config.get("chat_engine", "Auto"))
        self.combo_engine_bar = ttk.Combobox(
            launch_bar,
            textvariable=self.var_engine,
            values=["Auto", "Rust", "Lua"],
            width=6,
            state="readonly",
        )
        self.combo_engine_bar.pack(side=tk.LEFT, padx=2)

        self.var_start_zero = tk.BooleanVar(value=False)
        chk_start_zero = ttk.Checkbutton(launch_bar, text="С начала (с 00:00)", variable=self.var_start_zero)
        chk_start_zero.pack(side=tk.LEFT, padx=(8, 2))

        btn_launch = ttk.Button(launch_bar, text="▶ Смотреть в mpv", style="Accent.TButton", command=self.launch_player)
        btn_launch.pack(side=tk.RIGHT)

        btn_open_session = ttk.Button(launch_bar, text="📂 Открыть сессию", command=self._open_session_folder)
        btn_open_session.pack(side=tk.RIGHT, padx=8)

    def _build_import_tab(self):
        frm = ttk.Frame(self.tab_import, padding=12)
        frm.pack(fill=tk.BOTH, expand=True)

        lbl_desc = tk.Label(
            frm,
            text="Импорт чата из архива bestlogs.lurkology.com для существующих видео",
            font=("Segoe UI", 11, "bold"),
            bg=self.bg_color,
            fg="#bf94ff",
        )
        lbl_desc.pack(anchor=tk.W, pady=(0, 12))

        # Channel input
        r1 = ttk.Frame(frm)
        r1.pack(fill=tk.X, pady=4)
        ttk.Label(r1, text="Канал Twitch:", width=18).pack(side=tk.LEFT)
        self.imp_channel_entry = ttk.Entry(r1, width=30)
        self.imp_channel_entry.pack(side=tk.LEFT, padx=6)

        # Video files
        r2 = ttk.Frame(frm)
        r2.pack(fill=tk.X, pady=4)
        ttk.Label(r2, text="Видео файл(ы):", width=18).pack(side=tk.LEFT)
        self.imp_video_entry = ttk.Entry(r2, width=50)
        self.imp_video_entry.pack(side=tk.LEFT, padx=6)
        btn_browse_vid = ttk.Button(r2, text="Обзор...", command=self._browse_import_video)
        btn_browse_vid.pack(side=tk.LEFT)

        # Video start time & Auto-detection
        r3 = ttk.Frame(frm)
        r3.pack(fill=tk.X, pady=4)
        ttk.Label(r3, text="Старт стрима:", width=18).pack(side=tk.LEFT)
        self.imp_time_entry = ttk.Entry(r3, width=22)
        self.imp_time_entry.insert(0, "")
        self.imp_time_entry.pack(side=tk.LEFT, padx=6)
        self.var_auto_detect_start = tk.BooleanVar(value=True)
        self.chk_auto_detect = ttk.Checkbutton(
            r3,
            text="⚡ Автоопределение по кучности сообщений (< 1 мин между репликами)",
            variable=self.var_auto_detect_start,
        )
        self.chk_auto_detect.pack(side=tk.LEFT, padx=6)

        # Source mode
        r4 = ttk.Frame(frm)
        r4.pack(fill=tk.X, pady=4)
        ttk.Label(r4, text="Логи bestlogs (.json):", width=18).pack(side=tk.LEFT)
        self.imp_log_entry = ttk.Entry(r4, width=50)
        self.imp_log_entry.pack(side=tk.LEFT, padx=6)
        btn_browse_log = ttk.Button(r4, text="Выбрать файл(ы)...", command=self._browse_import_logs)
        btn_browse_log.pack(side=tk.LEFT)

        # Download option
        r5 = ttk.Frame(frm)
        r5.pack(fill=tk.X, pady=4)
        ttk.Label(r5, text="Или скачать за дату:", width=18).pack(side=tk.LEFT)
        self.imp_date_entry = ttk.Entry(r5, width=15)
        self.imp_date_entry.insert(0, datetime.datetime.now().strftime("%Y-%m-%d"))
        self.imp_date_entry.pack(side=tk.LEFT, padx=6)
        ttk.Label(r5, text="(например: 2026-10-28 или 2026-10-28,2026-10-29)", foreground=self.text_dim).pack(side=tk.LEFT)

        # Action button
        btn_run_import = ttk.Button(frm, text="🚀 Создать запись из логов", style="Accent.TButton", command=self._run_import)
        btn_run_import.pack(anchor=tk.W, pady=16)

        # Status output
        self.imp_status = tk.Text(frm, height=10, bg=self.card_bg, fg=self.text_color, font=("Consolas", 9))
        self.imp_status.pack(fill=tk.BOTH, expand=True)

    def _build_recorder_tab(self):
        frm = ttk.Frame(self.tab_record, padding=12)
        frm.pack(fill=tk.BOTH, expand=True)

        lbl = tk.Label(frm, text="Запись стрима и чата в реальном времени", font=("Segoe UI", 11, "bold"), bg=self.bg_color, fg="#bf94ff")
        lbl.pack(anchor=tk.W, pady=(0, 12))

        r1 = ttk.Frame(frm)
        r1.pack(fill=tk.X, pady=4)
        ttk.Label(r1, text="Канал Twitch:", width=16).pack(side=tk.LEFT)
        self.rec_chan_entry = ttk.Entry(r1, width=25)
        self.rec_chan_entry.pack(side=tk.LEFT, padx=6)

        ttk.Label(r1, text="Качество:").pack(side=tk.LEFT, padx=(12, 4))
        self.rec_qual_combo = ttk.Combobox(r1, values=["best", "1080p60", "720p60", "480p", "worst"], width=10)
        self.rec_qual_combo.set("best")
        self.rec_qual_combo.pack(side=tk.LEFT)

        r2 = ttk.Frame(frm)
        r2.pack(fill=tk.X, pady=8)
        self.btn_toggle_record = ttk.Button(r2, text="🔴 Начать запись", style="Accent.TButton", command=self._toggle_recording)
        self.btn_toggle_record.pack(side=tk.LEFT)

        self.rec_status_lbl = ttk.Label(r2, text="Готов к записи", foreground=self.text_dim)
        self.rec_status_lbl.pack(side=tk.LEFT, padx=12)

        # Recorder log console
        self.rec_console = tk.Text(frm, height=14, bg=self.card_bg, fg=self.text_color, font=("Consolas", 9))
        self.rec_console.pack(fill=tk.BOTH, expand=True, pady=(8, 0))

    def _build_settings_tab(self):
        frm = ttk.Frame(self.tab_settings, padding=12)
        frm.pack(fill=tk.BOTH, expand=True)

        lbl = tk.Label(frm, text="Настройки путей и Twitch API", font=("Segoe UI", 11, "bold"), bg=self.bg_color, fg="#bf94ff")
        lbl.pack(anchor=tk.W, pady=(0, 12))

        def add_field(label_text, cfg_key, width=40):
            row = ttk.Frame(frm)
            row.pack(fill=tk.X, pady=4)
            ttk.Label(row, text=label_text, width=22).pack(side=tk.LEFT)
            entry = ttk.Entry(row, width=width)
            entry.insert(0, str(self.config.get(cfg_key, "")))
            entry.pack(side=tk.LEFT, padx=6)
            return entry

        self.cfg_streamlink = add_field("Путь к Streamlink:", "streamlink_path")
        self.cfg_mpv = add_field("Путь к mpv:", "mpv_path")
        self.cfg_python = add_field("Путь к Python:", "python_path")
        self.cfg_client_id = add_field("Twitch Client ID:", "twitch_client_id")
        self.cfg_token = add_field("Twitch App/User Token:", "twitch_app_token")
        self.cfg_records_dir = add_field("Папка records:", "records_dir")

        lbl_chat = tk.Label(frm, text="Вид и оформление чата по умолчанию", font=("Segoe UI", 11, "bold"), bg=self.bg_color, fg="#bf94ff")
        lbl_chat.pack(anchor=tk.W, pady=(16, 8))

        self.cfg_chat_font = add_field("Шрифт чата:", "chat_font_family")
        self.cfg_chat_fs = add_field("Размер шрифта (px):", "chat_font_size")
        self.cfg_chat_w = add_field("Ширина чата (px):", "chat_width")
        self.cfg_chat_h = add_field("Высота чата (px):", "chat_height")
        self.cfg_chat_x = add_field("Позиция X (px):", "chat_x")
        self.cfg_chat_y = add_field("Позиция Y (px):", "chat_y")
        self.cfg_chat_bg_opacity = add_field("Прозрачность фона (%):", "chat_bg_opacity")

        r_eng = ttk.Frame(frm)
        r_eng.pack(fill=tk.X, pady=4)
        ttk.Label(r_eng, text="Движок чата по умолчанию:", width=26).pack(side=tk.LEFT)
        self.cfg_engine = ttk.Combobox(
            r_eng,
            values=["Auto (Rust DLL, иначе Lua)", "Rust (Высокая производительность)", "Lua (Классический совместимый)"],
            width=36,
            state="readonly",
        )
        cur_e = str(self.config.get("chat_engine", "Auto")).lower()
        if cur_e == "rust":
            self.cfg_engine.current(1)
        elif cur_e == "lua":
            self.cfg_engine.current(2)
        else:
            self.cfg_engine.current(0)
        self.cfg_engine.pack(side=tk.LEFT, padx=6)

        btn_save = ttk.Button(frm, text="💾 Сохранить настройки", style="Accent.TButton", command=self._save_settings_from_tab)
        btn_save.pack(anchor=tk.W, pady=16)

    def _save_settings_from_tab(self):
        self.config["streamlink_path"] = self.cfg_streamlink.get().strip()
        self.config["mpv_path"] = self.cfg_mpv.get().strip()
        self.config["python_path"] = self.cfg_python.get().strip()
        self.config["twitch_client_id"] = self.cfg_client_id.get().strip()
        self.config["twitch_app_token"] = self.cfg_token.get().strip()
        self.config["records_dir"] = self.cfg_records_dir.get().strip()

        self.config["chat_font_family"] = self.cfg_chat_font.get().strip() or "Segoe UI"
        try:
            self.config["chat_font_size"] = int(self.cfg_chat_fs.get().strip())
        except ValueError:
            pass
        try:
            self.config["chat_width"] = int(self.cfg_chat_w.get().strip())
        except ValueError:
            pass
        try:
            self.config["chat_height"] = int(self.cfg_chat_h.get().strip())
        except ValueError:
            pass
        try:
            self.config["chat_x"] = int(self.cfg_chat_x.get().strip())
        except ValueError:
            pass
        try:
            self.config["chat_y"] = int(self.cfg_chat_y.get().strip())
        except ValueError:
            pass
        try:
            op_str = self.cfg_chat_bg_opacity.get().strip().replace("%", "")
            self.config["chat_bg_opacity"] = int(op_str)
        except ValueError:
            pass

        idx = self.cfg_engine.current()
        if idx == 1:
            self.config["chat_engine"] = "Rust"
        elif idx == 2:
            self.config["chat_engine"] = "Lua"
        else:
            self.config["chat_engine"] = "Auto"
        if hasattr(self, "var_engine"):
            self.var_engine.set(self.config["chat_engine"])

        save_config(self.config)
        messagebox.showinfo("Сохранено", "Настройки успешно сохранены в config.json!")

    def refresh_records(self):
        self.channel_listbox.delete(0, tk.END)
        self.rec_tree.delete(*self.rec_tree.get_children())
        self.channels_data = {}

        if not self.records_base.exists():
            return

        for p in self.records_base.iterdir():
            if p.is_dir():
                ch_name, dt = parse_channel_from_folder(p.name)
                session_file = None
                chat_file = None
                emotes_file = None
                video_parts = []
                total_bytes = 0

                for item in p.iterdir():
                    if item.is_file():
                        total_bytes += item.stat().st_size
                        if item.name.endswith(".session.json"):
                            session_file = item
                        elif item.name.endswith(".chat.jsonl"):
                            chat_file = item
                        elif item.name.endswith(".emotes.json"):
                            emotes_file = item
                        elif item.name.endswith(".ts") or item.name.endswith(".mp4") or item.name.endswith(".mkv"):
                            video_parts.append(item)

                # Read session duration & info
                duration_sec = 0
                parts_count = len(video_parts)
                if session_file and session_file.exists():
                    try:
                        with open(session_file, "r", encoding="utf-8") as sf:
                            sdata = json.load(sf)
                            ch_name = sdata.get("channel", ch_name)
                            s_parts = sdata.get("parts", [])
                            if s_parts:
                                parts_count = len(s_parts)
                                for sp in s_parts:
                                    s_ms = sp.get("start_ms")
                                    e_ms = sp.get("end_ms")
                                    if s_ms and e_ms:
                                        duration_sec += max(0, (e_ms - s_ms) / 1000.0)
                    except Exception:
                        pass

                rec_info = {
                    "dir": p,
                    "channel": ch_name,
                    "date": dt,
                    "date_str": dt.strftime("%d.%m.%Y %H:%M:%S") if dt else p.name,
                    "parts_count": parts_count,
                    "duration_sec": duration_sec,
                    "size_bytes": total_bytes,
                    "has_chat": bool(chat_file),
                    "has_emotes": bool(emotes_file),
                    "has_session": bool(session_file),
                    "video_parts": sorted(video_parts, key=lambda x: x.name),
                }

                if ch_name not in self.channels_data:
                    self.channels_data[ch_name] = []
                self.channels_data[ch_name].append(rec_info)

        # Sort channels and insert into listbox
        sorted_channels = sorted(self.channels_data.keys(), key=lambda s: s.lower())
        for ch in sorted_channels:
            # Sort channel recordings newest first
            self.channels_data[ch].sort(
                key=lambda r: r["date"].timestamp() if r["date"] else 0,
                reverse=True,
            )
            count = len(self.channels_data[ch])
            self.channel_listbox.insert(tk.END, f"{ch} ({count})")

        if sorted_channels:
            self.channel_listbox.select_set(0)
            self._on_channel_selected(None)

    def _on_channel_selected(self, event):
        sel = self.channel_listbox.curselection()
        if not sel:
            return
        idx = sel[0]
        sorted_channels = sorted(self.channels_data.keys(), key=lambda s: s.lower())
        if idx >= len(sorted_channels):
            return

        ch = sorted_channels[idx]
        self.selected_channel = ch
        self.rec_title_lbl.config(text=f"Записи канала: {ch}")
        self.rec_tree.delete(*self.rec_tree.get_children())

        for rec in self.channels_data.get(ch, []):
            dur_str = format_duration(rec["duration_sec"])
            sz_str = format_bytes(rec["size_bytes"])
            chat_status = "✓" if rec["has_chat"] else "✗"
            emotes_status = "✓" if rec["has_emotes"] else "✗"

            item_id = self.rec_tree.insert(
                "",
                tk.END,
                values=(rec["date_str"], rec["parts_count"], dur_str, sz_str, chat_status, emotes_status),
            )
            # Store folder path in tree item tags/reference
            self.rec_tree.item(item_id, tags=(str(rec["dir"]),))

        children = self.rec_tree.get_children()
        if children:
            self.rec_tree.selection_set(children[0])

    def _get_selected_recording(self) -> Optional[dict]:
        sel = self.rec_tree.selection()
        if not sel:
            return None
        tags = self.rec_tree.item(sel[0], "tags")
        if not tags:
            return None
        folder_path = Path(tags[0])
        for rec in self.channels_data.get(self.selected_channel, []):
            if rec["dir"].resolve() == folder_path.resolve():
                return rec
        return None

    def launch_player(self, force_script=None):
        rec = self._get_selected_recording()
        if not rec:
            messagebox.showwarning("Внимание", "Пожалуйста, выберите запись для воспроизведения!")
            return

        video_parts = rec["video_parts"]
        if not video_parts:
            # Check if any .ts files exist
            video_parts = sorted(list(rec["dir"].glob("*.ts")), key=lambda x: x.name)

        if not video_parts:
            messagebox.showerror("Ошибка", f"В папке {rec['dir'].name} не найдено видеофайлов (.ts)!")
            return

        mpv_bin = self.config.get("mpv_path", "mpv")
        chat_on = "yes" if self.var_chat.get() else "no"
        emotes_on = "yes" if self.var_emotes.get() else "no"

        # Calculate IPC socket path
        is_win = platform.system() == "Windows"
        ipc_socket = r"\\.\pipe\mpv_socket" if is_win else "/tmp/mpv_socket"

        # Read initial offset
        try:
            offset_sec = float(self.entry_offset.get().strip())
        except ValueError:
            offset_sec = 0.0

        # Read chat layout & styling from GUI
        font_val = self.combo_font.get().strip() or "Segoe UI"
        try:
            fs_val = int(self.combo_font_sz.get().strip())
        except ValueError:
            fs_val = 17
        try:
            w_val = int(self.entry_chat_w.get().strip())
        except ValueError:
            w_val = 420
        try:
            h_val = int(self.entry_chat_h.get().strip())
        except ValueError:
            h_val = 600
        try:
            x_val = int(self.entry_chat_x.get().strip())
        except ValueError:
            x_val = 24
        try:
            y_val = int(self.entry_chat_y.get().strip())
        except ValueError:
            y_val = 60

        try:
            op_str = self.combo_bg_opacity.get().replace("%", "").strip()
            op_pct = int(op_str)
        except (ValueError, AttributeError):
            op_pct = 75
        op_val = max(0.0, min(1.0, op_pct / 100.0))

        # Update and save to config
        self.config["chat_font_family"] = font_val
        self.config["chat_font_size"] = fs_val
        self.config["chat_width"] = w_val
        self.config["chat_height"] = h_val
        self.config["chat_x"] = x_val
        self.config["chat_y"] = y_val
        self.config["chat_bg_opacity"] = op_pct
        if hasattr(self, "var_engine"):
            self.config["chat_engine"] = self.var_engine.get().strip()
        save_config(self.config)

        session_dir_posix = rec["dir"].as_posix()
        script_opts = (
            f"twitch_session={session_dir_posix},"
            f"twitch_chat={chat_on},"
            f"twitch_emotes={emotes_on},"
            f"twitch_font={font_val},"
            f"twitch_font_size={fs_val},"
            f"twitch_width={w_val},"
            f"twitch_height={h_val},"
            f"twitch_x={x_val},"
            f"twitch_y={y_val},"
            f"twitch_bg_opacity={op_val}"
        )

        # Detect script based on selected mode or fallback
        engine_mode = self.var_engine.get().strip() if hasattr(self, "var_engine") else self.config.get("chat_engine", "Auto")
        chosen_script = LUA_SCRIPT
        is_rust = False

        if force_script is not None:
            chosen_script = force_script
            is_rust = (chosen_script == RUST_DLL or chosen_script == RUST_DLL_TARGET)
        elif engine_mode == "Lua":
            chosen_script = LUA_SCRIPT
            is_rust = False
        elif engine_mode == "Rust":
            if RUST_DLL.exists():
                chosen_script = RUST_DLL
                is_rust = True
            elif RUST_DLL_TARGET.exists():
                chosen_script = RUST_DLL_TARGET
                is_rust = True
            else:
                messagebox.showwarning(
                    "Файл не найден",
                    f"Rust-плагин twitch_chat.dll не найден по пути:\n{RUST_DLL}\nПереключаемся на twitch_chat.lua."
                )
                chosen_script = LUA_SCRIPT
                is_rust = False
        else:  # Auto
            if RUST_DLL.exists():
                chosen_script = RUST_DLL
                is_rust = True
            elif RUST_DLL_TARGET.exists():
                chosen_script = RUST_DLL_TARGET
                is_rust = True
            else:
                chosen_script = LUA_SCRIPT
                is_rust = False

        mpv_log_path = SCRIPT_DIR / "mpv.log"
        cmd = [
            mpv_bin,
            f"--script={str(chosen_script.resolve())}",
            f"--script-opts={script_opts}",
            f"--input-ipc-server={ipc_socket}",
            "--keep-open=yes",
            f"--log-file={str(mpv_log_path.resolve())}",
        ]
        if hasattr(self, "var_start_zero") and self.var_start_zero.get():
            cmd.extend(["--start=0", "--no-resume-playback"])

        if is_rust:
            print(f"[Launcher] Using native high-performance Rust plugin: {chosen_script.resolve()}")
        else:
            print(f"[Launcher] Using Lua script: {chosen_script}")

        # Add all video parts as playlist
        for vp in video_parts:
            cmd.append(str(vp.resolve()))

        # Ensure emotes metadata and used chat images are prepared before starting mpv
        if self.var_emotes.get():
            self._ensure_session_emotes(rec["dir"])

        print("Launching mpv command:", cmd)
        try:
            proc = subprocess.Popen(cmd, cwd=str(SCRIPT_DIR))

            # Monitor process for early termination / crash
            def _check_early_crash(p=proc, record=rec, used_rust=is_rust):
                import time
                time.sleep(1.2)
                code = p.poll()
                if code is not None and code != 0:
                    err_lines = []
                    if mpv_log_path.exists():
                        try:
                            with open(mpv_log_path, "r", encoding="utf-8", errors="ignore") as lf:
                                err_lines = lf.readlines()[-35:]
                        except Exception:
                            pass
                    details = "".join(err_lines).strip()
                    print(f"[Launcher] MPV exited prematurely with code {code}!\n{details}")
                    msg = f"Плеер mpv завершился с кодом ошибки {code} сразу после запуска.\n"
                    if details:
                        msg += f"\nЛог mpv (последние строки):\n{details}\n"
                    if used_rust:
                        msg += "\nХотите запустить эту запись с классическим скриптом twitch_chat.lua?"
                        if messagebox.askyesno("Сбой запуска mpv", msg):
                            self.root.after(100, lambda: self.launch_player(force_script=LUA_SCRIPT))
                    else:
                        messagebox.showerror("Сбой запуска mpv", msg)

            threading.Thread(target=_check_early_crash, daemon=True).start()
        except FileNotFoundError:
            messagebox.showerror("Ошибка mpv", f"Плеер mpv не найден по пути: '{mpv_bin}'. Укажите корректный путь во вкладке 'Настройки'!")
        except Exception as e:
            messagebox.showerror("Ошибка запуска", str(e))

    def _ensure_session_emotes(self, session_dir: Path):
        try:
            emotes_files = list(session_dir.glob("*.emotes.json"))
            chat_files = list(session_dir.glob("*.chat.jsonl"))
            session_files = list(session_dir.glob("*.session.json"))

            from emotes import EmoteManager
            mgr = EmoteManager(
                cache_dir=SCRIPT_DIR / "emote_cache",
                twitch_client_id=self.config.get("twitch_client_id", ""),
                twitch_token=self.config.get("twitch_app_token", ""),
            )

            emotes_file = emotes_files[0] if emotes_files else (session_dir / f"{session_dir.name}.emotes.json")

            if not emotes_files and session_files:
                with open(session_files[0], "r", encoding="utf-8") as sf:
                    sdata = json.load(sf)
                ch = sdata.get("channel", "")
                rid = sdata.get("room_id")
                if ch:
                    mgr.snapshot_session(emotes_file, ch, rid, download_all_images=False)

            if chat_files and emotes_file.exists():
                mgr.cache_used_emotes_from_chat(chat_files[0], emotes_file)
        except Exception as e:
            print(f"Error ensuring emotes before mpv launch: {e}")

    def _open_records_folder(self):
        path = self.records_base.resolve()
        if platform.system() == "Windows":
            os.startfile(path)
        elif platform.system() == "Darwin":
            subprocess.Popen(["open", str(path)])
        else:
            subprocess.Popen(["xdg-open", str(path)])

    def _open_session_folder(self):
        rec = self._get_selected_recording()
        if rec:
            path = rec["dir"].resolve()
            if platform.system() == "Windows":
                os.startfile(path)
            elif platform.system() == "Darwin":
                subprocess.Popen(["open", str(path)])
            else:
                subprocess.Popen(["xdg-open", str(path)])

    def _browse_import_video(self):
        files = filedialog.askopenfilenames(
            title="Выберите видео файлы стрима",
            filetypes=[("Video files", "*.ts *.mp4 *.mkv"), ("All files", "*.*")],
        )
        if files:
            self.imp_video_entry.delete(0, tk.END)
            self.imp_video_entry.insert(0, ";".join(files))

    def _browse_import_logs(self):
        files = filedialog.askopenfilenames(
            title="Выберите файлы логов bestlogs (.json)",
            filetypes=[("JSON files", "*.json"), ("All files", "*.*")],
        )
        if files:
            self.imp_log_entry.delete(0, tk.END)
            self.imp_log_entry.insert(0, ";".join(files))

    def _run_import(self):
        chan = self.imp_channel_entry.get().strip()
        time_str = self.imp_time_entry.get().strip()
        vids_str = self.imp_video_entry.get().strip()
        logs_str = self.imp_log_entry.get().strip()
        date_str = self.imp_date_entry.get().strip()

        auto_detect = self.var_auto_detect_start.get()

        if not chan:
            messagebox.showwarning("Внимание", "Укажите имя канала!")
            return

        start_dt = None
        if time_str and not auto_detect:
            try:
                start_dt = datetime.datetime.strptime(time_str, "%Y-%m-%d %H:%M:%S")
            except ValueError:
                messagebox.showerror("Ошибка", "Неверный формат даты и времени! Используйте ГГГГ-ММ-ДД ЧЧ:ММ:СС или включите автоопределение.")
                return

        target_date = None
        if date_str:
            try:
                first_d = date_str.split(",")[0].strip()
                y, m, d = first_d.split("-")
                target_date = datetime.date(int(y), int(m), int(d))
            except Exception:
                pass

        video_paths = [Path(p) for p in vids_str.split(";") if p]
        log_paths = [Path(p) for p in logs_str.split(";") if p]

        self.imp_status.insert(tk.END, f"Запуск импорта для канала {chan}...\n")
        self.imp_status.see(tk.END)

        def worker():
            from importer import fetch_day_log, import_logs_to_session
            days_data = []

            if log_paths:
                for lp in log_paths:
                    self.imp_status.insert(tk.END, f"Чтение локального файла {lp.name}...\n")
                    with open(lp, "r", encoding="utf-8") as fp:
                        days_data.append(json.load(fp))
            elif date_str:
                for d_item in date_str.split(","):
                    try:
                        y, m, d = d_item.strip().split("-")
                        self.imp_status.insert(tk.END, f"Запрос логов за {d_item} с bestlogs.lurkology.com...\n")
                        res = fetch_day_log(chan, y, m, d)
                        if res:
                            days_data.append(res)
                    except Exception as err:
                        self.imp_status.insert(tk.END, f"Ошибка загрузки: {err}\n")

            if not days_data:
                self.imp_status.insert(tk.END, "❌ Нет данных логов для импорта.\n")
                return

            if auto_detect:
                self.imp_status.insert(tk.END, "🔍 Анализ чата: поиск момента старта стрима по кучности сообщений...\n")
                self.imp_status.see(tk.END)

            try:
                session_dir = import_logs_to_session(
                    channel=chan,
                    days_data=days_data,
                    video_files=video_paths,
                    start_dt=start_dt,
                    target_date=target_date,
                    auto_detect_start=auto_detect,
                    records_dir=self.records_base,
                    emote_cache_dir=SCRIPT_DIR / "emote_cache",
                    config=self.config,
                )
                
                # Read detected info from session.json
                sess_file = session_dir / f"{session_dir.name}.session.json"
                if sess_file.exists():
                    try:
                        with open(sess_file, "r", encoding="utf-8") as sf:
                            sdata = json.load(sf)
                            det_info = sdata.get("start_detection_info")
                            if det_info:
                                self.imp_status.insert(tk.END, f"🎯 {det_info}\n")
                    except Exception:
                        pass

                self.imp_status.insert(tk.END, f"✅ Импорт завершен! Создана сессия: {session_dir.name}\n")
                self.imp_status.see(tk.END)
                self.root.after(500, self.refresh_records)
            except Exception as e:
                self.imp_status.insert(tk.END, f"❌ Ошибка: {e}\n")

        threading.Thread(target=worker, daemon=True).start()

    def _stop_active_recording(self):
        if not self.active_recording_proc or self.active_recording_proc.poll() is not None:
            self.active_recording_proc = None
            self.btn_toggle_record.config(text="🔴 Начать запись")
            self.rec_status_lbl.config(text="Запись остановлена")
            return

        pid = self.active_recording_proc.pid
        self.rec_status_lbl.config(text="Остановка записи...")
        self.rec_console.insert(tk.END, f"\n[Launcher] Остановка записи (PID {pid})...\n")
        self.rec_console.see(tk.END)

        if platform.system() == "Windows":
            # On Windows, kill the entire process tree (recorder.py + streamlink processes)
            try:
                subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)], capture_output=True)
            except Exception as e:
                print(f"taskkill error: {e}")
        else:
            try:
                import signal
                os.killpg(os.getpgid(pid), signal.SIGTERM)
            except Exception:
                try:
                    self.active_recording_proc.terminate()
                    self.active_recording_proc.wait(timeout=2)
                except Exception:
                    self.active_recording_proc.kill()

        try:
            self.active_recording_proc.wait(timeout=2)
        except Exception:
            pass

        self.active_recording_proc = None
        self.btn_toggle_record.config(text="🔴 Начать запись")
        self.rec_status_lbl.config(text="Запись остановлена")
        self.rec_console.insert(tk.END, "[Launcher] Запись остановлена. Файлы сохранены в records/.\n")
        self.rec_console.see(tk.END)
        self.root.after(800, self.refresh_records)

    def _toggle_recording(self):
        if self.active_recording_proc and self.active_recording_proc.poll() is None:
            self._stop_active_recording()
        else:
            chan = self.rec_chan_entry.get().strip()
            if not chan:
                messagebox.showwarning("Внимание", "Укажите имя канала для записи!")
                return
            qual = self.rec_qual_combo.get().strip() or "best"
            python_bin = self.config.get("python_path", "python")
            recorder_py = SCRIPT_DIR / "recorder.py"

            cmd = [python_bin, str(recorder_py), chan, "--quality", qual]
            self.rec_console.delete("1.0", tk.END)
            self.rec_console.insert(tk.END, f"Запуск: {' '.join(cmd)}\n")

            popen_kwargs = {
                "stdout": subprocess.PIPE,
                "stderr": subprocess.STDOUT,
                "universal_newlines": True,
                "bufsize": 1,
            }
            if platform.system() == "Windows":
                popen_kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP

            self.active_recording_proc = subprocess.Popen(cmd, **popen_kwargs)
            self.btn_toggle_record.config(text="⏹ Остановить запись")
            self.rec_status_lbl.config(text=f"Идет запись стрима #{chan}...")

            def reader():
                while self.active_recording_proc and self.active_recording_proc.poll() is None:
                    line = self.active_recording_proc.stdout.readline()
                    if line:
                        self.rec_console.insert(tk.END, line)
                        self.rec_console.see(tk.END)
                self.root.after(500, self.refresh_records)

            threading.Thread(target=reader, daemon=True).start()

    def _on_close(self):
        if self.active_recording_proc and self.active_recording_proc.poll() is None:
            if messagebox.askyesno("Выход", "Идет запись стрима! Остановить запись и выйти?"):
                self._stop_active_recording()
                self.root.destroy()
        else:
            self.root.destroy()


def main():
    root = tk.Tk()
    app = TwitchRecordsApp(root)
    root.mainloop()


if __name__ == "__main__":
    main()
