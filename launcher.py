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
import queue
import time
from twitch_auth import refresh_twitch_token, TokenRefreshError
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
LUA_INPUT_SCRIPT = SCRIPT_DIR / "twitch_chat_input.lua"
RUST_DLL = SCRIPT_DIR / "twitch_chat.dll"
RUST_DLL_TARGET = SCRIPT_DIR / "build" / "twitch_chat.dll"


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
    temp = CONFIG_FILE.with_name(CONFIG_FILE.name + ".tmp")
    try:
        with open(temp, "w", encoding="utf-8") as file:
            json.dump(cfg, file, ensure_ascii=False, indent=2)
        os.replace(temp, CONFIG_FILE)
        return True
    except OSError:
        print("[Launcher] Не удалось сохранить config.json")
        return False


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
        self.root.geometry("600x780")
        self.root.minsize(500, 640)

        self.config = load_config()
        self.records_base = SCRIPT_DIR / self.config.get("records_dir", "records")
        self.records_base.mkdir(parents=True, exist_ok=True)

        self.channels_data: Dict[str, List[dict]] = {}
        self.selected_channel: Optional[str] = None
        self.active_recording_proc: Optional[subprocess.Popen] = None
        self.token_refresh_busy = False
        self.token_results = queue.Queue()
        self.ui_results = queue.Queue()
        self.import_busy = False
        self.waiting_for_token = []
        self.root.after(100, self._poll_ui_results)

        self._apply_theme()
        self._build_ui()
        self.refresh_records()
        self.root.after(100, self._poll_token_results)
        self.root.after(200, self._refresh_token_on_startup)

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

        self.style.configure("TLabelframe", background=self.bg_color, bordercolor="#34343d")
        self.style.configure("TLabelframe.Label", background=self.bg_color, foreground="#bf94ff", font=("Segoe UI", 9, "bold"))
        self.style.configure("Compact.TButton", padding=[7, 3])
        self.style.configure("Dim.TLabel", foreground=self.text_dim)
        self.style.configure("TEntry", padding=3)
        self.style.configure("TCombobox", padding=3)

    def _scrollable_frame(self, parent):
        host = ttk.Frame(parent)
        host.pack(fill=tk.BOTH, expand=True)
        canvas = tk.Canvas(host, bg=self.bg_color, highlightthickness=0, borderwidth=0)
        scrollbar = ttk.Scrollbar(host, orient=tk.VERTICAL, command=canvas.yview)
        scrollbar.pack(side=tk.RIGHT, fill=tk.Y)
        canvas.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        canvas.configure(yscrollcommand=scrollbar.set)
        content = ttk.Frame(canvas, padding=8)
        window = canvas.create_window((0, 0), window=content, anchor="nw")
        content.bind("<Configure>", lambda event: canvas.configure(scrollregion=canvas.bbox("all")))
        canvas.bind("<Configure>", lambda event: canvas.itemconfigure(window, width=event.width))

        def wheel(event):
            if canvas.winfo_exists() and canvas.winfo_ismapped():
                if content.winfo_reqheight() > canvas.winfo_height():
                    canvas.yview_scroll(-1 if event.delta > 0 else 1, "units")

        def install_wheel(widget):
            widget.bind("<MouseWheel>", wheel, add="+")
            for child in widget.winfo_children():
                install_wheel(child)

        self.root.after_idle(lambda: install_wheel(content))
        return content

    def _build_ui(self):
        # Header bar
        header = ttk.Frame(self.root, padding=(12, 8))
        header.pack(fill=tk.X)

        title = tk.Label(
            header,
            text="Twitch · mpv",
            font=("Segoe UI", 14, "bold"),
            bg=self.bg_color,
            fg="#bf94ff",
        )
        title.pack(anchor=tk.W)

        subtitle = tk.Label(
            header,
            text="Записи, чат и синхронизация",
            font=("Segoe UI", 9),
            bg=self.bg_color,
            fg=self.text_dim,
        )
        subtitle.pack(anchor=tk.W, pady=(2, 0))

        # Tabbed Notebook
        self.notebook = ttk.Notebook(self.root)
        self.notebook.pack(fill=tk.BOTH, expand=True, padx=12, pady=(0, 12))

        # Tab 1: Recordings & Player
        self.tab_records = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(self.tab_records, text="Записи")

        # Tab 2: Importer (bestlogs)
        self.tab_import = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(self.tab_import, text="Импорт")

        # Tab 3: Live Recorder
        self.tab_record = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(self.tab_record, text="Запись")

        # Tab 4: Settings
        self.tab_settings = ttk.Frame(self.notebook, padding=10)
        self.notebook.add(self.tab_settings, text="Настройки")

        self._build_records_tab()
        self._build_import_tab()
        self._build_recorder_tab()
        self._build_settings_tab()

    def _build_records_tab(self):
        self.tab_records.rowconfigure(0, weight=1)
        self.tab_records.columnconfigure(0, weight=1)
        paned = tk.PanedWindow(self.tab_records, orient=tk.VERTICAL, bg=self.card_bg, sashwidth=5)
        paned.grid(row=0, column=0, sticky="nsew")

        left_frame = ttk.Frame(paned, padding=6)
        paned.add(left_frame, minsize=80, height=150)
        ttk.Label(left_frame, text="КАНАЛЫ", font=("Segoe UI", 9, "bold"), foreground=self.text_dim).pack(anchor=tk.W, pady=(0, 6))
        channel_scroll = ttk.Scrollbar(left_frame, orient=tk.VERTICAL)
        channel_scroll.pack(side=tk.RIGHT, fill=tk.Y)
        self.channel_listbox = tk.Listbox(left_frame, bg=self.card_bg, fg=self.text_color,
            selectbackground=self.accent_purple, selectforeground="#ffffff", highlightthickness=0,
            borderwidth=0, font=("Segoe UI", 10), width=16, exportselection=False,
            yscrollcommand=channel_scroll.set)
        self.channel_listbox.pack(fill=tk.BOTH, expand=True)
        channel_scroll.configure(command=self.channel_listbox.yview)
        self.channel_listbox.bind("<<ListboxSelect>>", self._on_channel_selected)

        right_frame = ttk.Frame(paned, padding=6)
        paned.add(right_frame, minsize=220)
        right_frame.columnconfigure(0, weight=1)
        right_frame.rowconfigure(1, weight=1)
        toolbar = ttk.Frame(right_frame)
        toolbar.grid(row=0, column=0, sticky="ew", pady=(0, 6))
        self.rec_title_lbl = ttk.Label(toolbar, text="Выберите канал", font=("Segoe UI", 10, "bold"))
        self.rec_title_lbl.pack(side=tk.LEFT)
        ttk.Button(toolbar, text="Обновить", style="Compact.TButton", command=self.refresh_records).pack(side=tk.RIGHT)
        ttk.Button(toolbar, text="Папка", style="Compact.TButton", command=self._open_records_folder).pack(side=tk.RIGHT, padx=4)

        table = ttk.Frame(right_frame)
        table.grid(row=1, column=0, sticky="nsew")
        table.rowconfigure(0, weight=1)
        table.columnconfigure(0, weight=1)
        columns = ("date", "parts", "duration", "size", "chat", "emotes")
        self.rec_tree = ttk.Treeview(table, columns=columns, show="headings", selectmode="browse", height=6)
        for name, label, width in [("date", "Дата и время", 155), ("parts", "Части", 48),
            ("duration", "Время", 85), ("size", "Размер", 78), ("chat", "Чат", 45), ("emotes", "Эмоуты", 60)]:
            self.rec_tree.heading(name, text=label)
            self.rec_tree.column(name, width=width, minwidth=45, stretch=name == "date", anchor=tk.W if name == "date" else tk.CENTER)
        tree_y = ttk.Scrollbar(table, orient=tk.VERTICAL, command=self.rec_tree.yview)
        tree_x = ttk.Scrollbar(table, orient=tk.HORIZONTAL, command=self.rec_tree.xview)
        self.rec_tree.configure(yscrollcommand=tree_y.set, xscrollcommand=tree_x.set)
        self.rec_tree.grid(row=0, column=0, sticky="nsew")
        tree_y.grid(row=0, column=1, sticky="ns")
        tree_x.grid(row=1, column=0, sticky="ew")
        self.rec_tree.bind("<Double-1>", lambda event: self.launch_player())

        footer = ttk.Frame(self.tab_records, padding=(0, 6, 0, 0))
        footer.grid(row=1, column=0, sticky="ew")
        footer.columnconfigure(0, weight=1)
        ttk.Label(footer, text="Двойной клик — запуск", style="Dim.TLabel", font=("Segoe UI", 8)).grid(row=0, column=0, sticky="w")
        self.btn_open_session = ttk.Button(footer, text="Папка сессии", command=self._open_session_folder)
        self.btn_open_session.grid(row=0, column=1, padx=(6, 8))
        self.btn_launch = ttk.Button(footer, text="▶ Смотреть в mpv", style="Accent.TButton", command=self.launch_player)
        self.btn_launch.grid(row=0, column=2)

    def _build_import_tab(self):
        frm = ttk.Frame(self.tab_import, padding=8)
        frm.pack(fill=tk.BOTH, expand=True)
        ttk.Label(frm, text="Импорт чата для существующих видео", foreground="#bf94ff", wraplength=420).pack(anchor=tk.W, pady=(0, 8))

        def field(label, browse=None):
            ttk.Label(frm, text=label).pack(anchor=tk.W, pady=(5, 2))
            row = ttk.Frame(frm)
            row.pack(fill=tk.X)
            if browse:
                ttk.Button(row, text="Обзор…", command=browse).pack(side=tk.RIGHT, padx=(6, 0))
            entry = ttk.Entry(row)
            entry.pack(side=tk.LEFT, fill=tk.X, expand=True)
            return entry

        self.imp_channel_entry = field("Канал Twitch")
        self.imp_video_entry = field("Видео", self._browse_import_video)
        self.imp_time_entry = field("Старт (UTC): YYYY-MM-DD HH:MM:SS")
        self.var_auto_detect_start = tk.BooleanVar(value=True)
        self.chk_auto_detect = ttk.Checkbutton(frm, text="Автоопределение старта по сообщениям", variable=self.var_auto_detect_start)
        self.chk_auto_detect.pack(anchor=tk.W, pady=4)
        self.imp_date_entry = field("Даты архива (UTC): YYYY-MM-DD, …")
        self.imp_date_entry.insert(0, datetime.datetime.now().strftime("%Y-%m-%d"))
        self.btn_import = ttk.Button(frm, text="Создать сессию из архива", style="Accent.TButton", command=self._run_import)
        self.btn_import.pack(anchor=tk.W, pady=10)
        self.imp_status = tk.Text(frm, height=5, bg=self.card_bg, fg=self.text_color, font=("Consolas", 9))
        self.imp_status.pack(fill=tk.BOTH, expand=True)

    def _build_recorder_tab(self):
        frm = ttk.Frame(self.tab_record, padding=8)
        frm.pack(fill=tk.BOTH, expand=True)
        ttk.Label(frm, text="Запись стрима и чата", foreground="#bf94ff").pack(anchor=tk.W, pady=(0, 8))
        ttk.Label(frm, text="Канал Twitch").pack(anchor=tk.W)
        self.rec_chan_entry = ttk.Entry(frm)
        self.rec_chan_entry.pack(fill=tk.X, pady=(3, 8))
        ttk.Label(frm, text="Качество").pack(anchor=tk.W)
        self.rec_qual_combo = ttk.Combobox(frm, values=["best", "1080p60", "720p60", "480p", "worst"], width=12)
        self.rec_qual_combo.set("best")
        self.rec_qual_combo.pack(anchor=tk.W, pady=(3, 8))
        self.btn_toggle_record = ttk.Button(frm, text="🔴 Начать запись", style="Accent.TButton", command=self._toggle_recording)
        self.btn_toggle_record.pack(anchor=tk.W)
        self.rec_status_lbl = ttk.Label(frm, text="Готов к записи", foreground=self.text_dim)
        self.rec_status_lbl.pack(anchor=tk.W, pady=8)
        self.rec_console = tk.Text(frm, height=10, bg=self.card_bg, fg=self.text_color, font=("Consolas", 9))
        self.rec_console.pack(fill=tk.BOTH, expand=True)

    def _build_settings_tab(self):
        footer = ttk.Frame(self.tab_settings, padding=(8, 6))
        footer.pack(side=tk.BOTTOM, fill=tk.X)
        self.btn_save_settings = ttk.Button(footer, text="Сохранить настройки", style="Accent.TButton", command=self._save_settings_from_tab)
        self.btn_save_settings.pack(side=tk.RIGHT)
        frm = self._scrollable_frame(self.tab_settings)

        lbl = tk.Label(frm, text="Настройки путей и Twitch API", font=("Segoe UI", 11, "bold"), bg=self.bg_color, fg="#bf94ff")
        lbl.pack(anchor=tk.W, pady=(0, 8))

        def add_field(label_text, cfg_key, width=40):
            row = ttk.Frame(frm)
            row.pack(fill=tk.X, pady=3)
            row.columnconfigure(1, weight=1)
            ttk.Label(row, text=label_text).grid(row=0, column=0, sticky="w", pady=(0, 2))
            entry = ttk.Entry(row, width=width, show="•" if cfg_key in {"twitch_app_token", "twitch_client_secret", "twitch_refresh_token"} else "")
            entry.insert(0, str(self.config.get(cfg_key, "")))
            entry.grid(row=1, column=0, columnspan=2, sticky="ew")
            row.columnconfigure(0, weight=1)
            return entry

        self.cfg_streamlink = add_field("Путь к Streamlink:", "streamlink_path")
        self.cfg_mpv = add_field("Путь к mpv:", "mpv_path")
        self.cfg_python = add_field("Путь к Python:", "python_path")
        self.cfg_client_id = add_field("Twitch Client ID:", "twitch_client_id")
        self.cfg_token = add_field("Twitch App/User Token:", "twitch_app_token")
        self.cfg_secret = add_field("Twitch Client Secret:", "twitch_client_secret")
        self.cfg_refresh_token = add_field("Twitch Refresh Token:", "twitch_refresh_token")
        self.btn_refresh_token = ttk.Button(frm, text="Обновить токен", command=self._refresh_token_manually)
        self.btn_refresh_token.pack(anchor=tk.W, pady=(6, 2))
        self.token_status = ttk.Label(frm, text="Токен обновляется при запуске", style="Dim.TLabel", wraplength=400)
        self.token_status.pack(anchor=tk.W, pady=(0, 8))
        self.cfg_records_dir = add_field("Папка records:", "records_dir")

        r_eng = ttk.Frame(frm)
        r_eng.pack(fill=tk.X, pady=4)
        ttk.Label(r_eng, text="Движок чата:").pack(anchor=tk.W)
        self.cfg_engine = ttk.Combobox(
            r_eng,
            values=["Auto", "Rust", "Lua"],
            width=12,
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


    def _save_settings_from_tab(self):
        updated = dict(self.config)
        for key, entry in [("streamlink_path", self.cfg_streamlink), ("mpv_path", self.cfg_mpv),
            ("python_path", self.cfg_python), ("records_dir", self.cfg_records_dir),
            ("twitch_client_id", self.cfg_client_id), ("twitch_client_secret", self.cfg_secret),
            ("twitch_app_token", self.cfg_token), ("twitch_refresh_token", self.cfg_refresh_token)]:
            updated[key] = entry.get().strip()
        updated["chat_engine"] = self.cfg_engine.get() or "Auto"
        if not save_config(updated):
            messagebox.showerror("Ошибка", "Не удалось сохранить настройки")
            return
        self.config = updated
        self.records_base = SCRIPT_DIR / self.config.get("records_dir", "records")
        self.records_base.mkdir(parents=True, exist_ok=True)
        self.refresh_records()
        messagebox.showinfo("Сохранено", "Настройки успешно сохранены в config.json!")

    def _refresh_token_on_startup(self):
        self._start_token_refresh(manual=False)

    def _refresh_token_manually(self):
        self._start_token_refresh(manual=True)

    def _start_token_refresh(self, manual=False):
        if self.token_refresh_busy:
            return
        if manual:
            credentials = (self.cfg_client_id.get().strip(), self.cfg_secret.get().strip(), self.cfg_refresh_token.get().strip())
        else:
            credentials = tuple(str(self.config.get(key, "")).strip() for key in ("twitch_client_id", "twitch_client_secret", "twitch_refresh_token"))
        if not all(credentials):
            message = "Не заполнены Twitch Client ID, Client Secret или Refresh Token"
            print("[Twitch OAuth] " + message)
            self.token_status.configure(text=message)
            return
        self.token_refresh_busy = True
        self.btn_refresh_token.configure(state="disabled")
        self.btn_save_settings.configure(state="disabled")
        for entry in (self.cfg_client_id, self.cfg_secret, self.cfg_refresh_token, self.cfg_token):
            entry.configure(state="disabled")
        self.token_status.configure(text="Обновление токена…")
        print("[Twitch OAuth] Обновление токена…")

        def worker():
            try:
                result = refresh_twitch_token(*credentials)
                self.token_results.put((credentials, result, None))
            except TokenRefreshError as exc:
                self.token_results.put((credentials, None, str(exc)))
            except Exception:
                self.token_results.put((credentials, None, "Неожиданная ошибка обновления Twitch-токена"))

        threading.Thread(target=worker, daemon=True).start()

    def _poll_token_results(self):
        try:
            while True:
                credentials, result, error = self.token_results.get_nowait()
                self.token_refresh_busy = False
                self.btn_refresh_token.configure(state="normal")
                self.btn_save_settings.configure(state="normal")
                for entry in (self.cfg_client_id, self.cfg_secret, self.cfg_refresh_token, self.cfg_token):
                    entry.configure(state="normal")
                current = (self.cfg_client_id.get().strip(), self.cfg_secret.get().strip(), self.cfg_refresh_token.get().strip())
                if current != credentials:
                    error = "Реквизиты изменены во время запроса; результат не сохранён"
                if error is None:
                    updated = dict(self.config)
                    updated.update({
                        "twitch_client_id": credentials[0], "twitch_client_secret": credentials[1],
                        "twitch_app_token": result["access_token"], "twitch_refresh_token": result["refresh_token"],
                        "twitch_token_expires_at": int(time.time()) + result["expires_in"],
                    })
                    if save_config(updated):
                        self.config = updated
                        for entry, key in [(self.cfg_token, "twitch_app_token"), (self.cfg_refresh_token, "twitch_refresh_token")]:
                            entry.delete(0, tk.END)
                            entry.insert(0, updated[key])
                        message = "Access token и refresh token обновлены и сохранены"
                    else:
                        message = "Токен получен, но не удалось сохранить config.json"
                        # Keep the rotated credentials in memory for a retry after disk repair.
                        self.config = updated
                        for entry, key in [(self.cfg_token, "twitch_app_token"), (self.cfg_refresh_token, "twitch_refresh_token")]:
                            entry.delete(0, tk.END)
                            entry.insert(0, updated[key])
                else:
                    message = error
                print("[Twitch OAuth] " + message)
                self.token_status.configure(text=message)
                callbacks, self.waiting_for_token = self.waiting_for_token, []
                for callback in callbacks:
                    callback()
        except queue.Empty:
            pass
        self.root.after(100, self._poll_token_results)

    def _cache_directory(self):
        value = Path(self.config.get("emote_cache_dir", "emote_cache"))
        return (value if value.is_absolute() else SCRIPT_DIR / value).resolve()

    def _poll_ui_results(self):
        try:
            while True:
                kind, value = self.ui_results.get_nowait()
                if kind == "import-log":
                    self.imp_status.insert(tk.END, value + "\n")
                    self.imp_status.see(tk.END)
                elif kind == "import-done":
                    self.import_busy = False
                    self.btn_import.configure(state="normal")
                    self.refresh_records()
        except queue.Empty:
            pass
        self.root.after(100, self._poll_ui_results)

    def _after_token_refresh(self, callback):
        if self.token_refresh_busy:
            self.waiting_for_token.append(callback)
            print("[Launcher] Ожидание обновления Twitch-токена…")
        else:
            callback()

    def refresh_records(self):
        self.channel_listbox.delete(0, tk.END)
        self.rec_tree.delete(*self.rec_tree.get_children())
        self.channels_data = {}

        if not self.records_base.exists():
            return

        for p in self.records_base.iterdir():
            if p.is_dir() and not p.is_symlink():
                ch_name, dt = parse_channel_from_folder(p.name)
                if dt is None or not re.fullmatch(r"[A-Za-z0-9_]{1,25}", ch_name):
                    continue
                session_file = p / f"{p.name}.session.json"
                if not session_file.is_file():
                    continue
                try:
                    with session_file.open(encoding="utf-8") as stream:
                        manifest = json.load(stream)
                    if not isinstance(manifest, dict) or str(manifest.get("channel", "")).lower() != ch_name.lower():
                        continue
                    if not isinstance(manifest.get("session_start_ms"), int) or isinstance(manifest.get("session_start_ms"), bool):
                        continue
                except (OSError, ValueError):
                    continue
                chat_file = None
                emotes_file = None
                video_parts = []
                total_bytes = 0

                for item in sorted(p.iterdir()):
                    if item.is_file():
                        total_bytes += item.stat().st_size
                        if item == session_file:
                            pass
                        elif item.name.endswith(".chat.jsonl") or item.name == "chat.jsonl":
                            chat_file = item
                        elif item.name.endswith(".emotes.json") or item.name == "emotes.json":
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
                            referenced_videos = []
                            for part in s_parts:
                                if not isinstance(part, dict): continue
                                file = part.get("file")
                                if isinstance(file, str) and file:
                                    path = Path(file)
                                    referenced_videos.append((path if path.is_absolute() else p / path).resolve())
                                start, end = part.get("start_ms"), part.get("end_ms")
                                if isinstance(start, (int, float)) and isinstance(end, (int, float)):
                                    duration_sec += max(0, (end - start) / 1000.0)
                            if referenced_videos: video_parts = referenced_videos
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
                    "video_parts": video_parts,
                    "session_file": session_file,
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
        if self.token_refresh_busy:
            self._after_token_refresh(lambda: self.launch_player(force_script))
            return
        rec = self._get_selected_recording()
        if not rec:
            messagebox.showwarning("Внимание", "Пожалуйста, выберите запись для воспроизведения!")
            return

        video_parts = rec["video_parts"]
        if not video_parts:
            # Check if any .ts files exist
            video_parts = sorted(list(rec["dir"].glob("*.ts")), key=lambda x: x.name)

        if not video_parts:
            messagebox.showerror("Нет видео", "Сессия содержит только чат. Выберите видео при импорте или добавьте части в файл сессии.")
            return
        missing = [path for path in video_parts if not path.is_file()]
        if missing:
            messagebox.showerror("Видео недоступно", "Не найдены файлы:\n" + "\n".join(str(path) for path in missing))
            return

        mpv_bin = self.config.get("mpv_path", "mpv")
        is_win = platform.system() == "Windows"
        ipc_socket = r"\\.\pipe\mpv_socket" if is_win else "/tmp/mpv_socket"
        session_path = rec.get("session_file") or rec["dir"]
        script_opts = f"twitch_session={session_path.as_posix()}"

        # Detect script based on selected mode or fallback
        engine_mode = self.config.get("chat_engine", "Auto")
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
        if is_rust:
            print(f"[Launcher] Using native high-performance Rust plugin: {chosen_script.resolve()}")
            if LUA_INPUT_SCRIPT.exists():
                cmd.append(f"--script={str(LUA_INPUT_SCRIPT.resolve())}")
                print(f"[Launcher] Attached companion input script: {LUA_INPUT_SCRIPT.resolve()}")
        else:
            print(f"[Launcher] Using Lua script: {chosen_script}")

        # Add all video parts as playlist
        for vp in video_parts:
            cmd.append(str(vp.resolve()))

        # Ensure emotes metadata and used chat images are prepared before starting mpv
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
            def associated(suffix, plain):
                preferred = session_dir / f"{session_dir.name}{suffix}"
                if preferred.is_file(): return preferred
                simple = session_dir / plain
                if simple.is_file(): return simple
                return next(iter(sorted(session_dir.glob(f"*{suffix}"))), preferred)
            session_file = associated(".session.json", "session.json")
            emotes_file = associated(".emotes.json", "emotes.json")
            chat_file = associated(".chat.jsonl", "chat.jsonl")
            data = {}
            if session_file.is_file():
                with session_file.open(encoding="utf-8") as file: data = json.load(file)
            root = data.get("emote_cache_dir")
            cache_dir = Path(root) if isinstance(root, str) and root else self._cache_directory()
            if not cache_dir.is_absolute(): cache_dir = session_dir / cache_dir
            from emotes import EmoteManager
            manager = EmoteManager(cache_dir.resolve(), self.config.get("twitch_client_id", ""), self.config.get("twitch_app_token", ""))
            if data.get("channel"):
                manager.snapshot_session(emotes_file, data["channel"], data.get("room_id"), capture={"imported_start_ms": data.get("session_start_ms")})
            if chat_file.is_file() and emotes_file.is_file():
                report = manager.cache_used_emotes_from_chat(chat_file, emotes_file)
                if report.get("failed") or report.get("missing_metadata"):
                    print("[Launcher] Кэш эмоутов подготовлен частично; см. сообщения провайдеров")
        except Exception as exc:
            print(f"[Launcher] Не удалось подготовить кэш эмоутов ({type(exc).__name__})")

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

    def _run_import(self):
        if self.import_busy: return
        if self.token_refresh_busy:
            self._after_token_refresh(self._run_import)
            return
        channel = self.imp_channel_entry.get().strip().lower().lstrip("#")
        if not re.fullmatch(r"[a-z0-9_]{1,25}", channel):
            messagebox.showwarning("Канал", "Укажите Twitch login: буквы, цифры и подчёркивание")
            return
        try:
            dates = [datetime.date.fromisoformat(value.strip()) for value in self.imp_date_entry.get().split(",") if value.strip()]
            if not dates: raise ValueError("Укажите хотя бы одну дату")
            auto_detect = self.var_auto_detect_start.get()
            start_text = self.imp_time_entry.get().strip()
            start = datetime.datetime.strptime(start_text, "%Y-%m-%d %H:%M:%S").replace(tzinfo=datetime.timezone.utc) if start_text and not auto_detect else None
            if not auto_detect and start is None: raise ValueError("Задайте время старта в UTC")
            videos = [Path(value.strip()).resolve() for value in self.imp_video_entry.get().split(";") if value.strip()]
            for video in videos:
                if not video.is_file(): raise ValueError(f"Видео не найдено: {video.name}")
        except ValueError as exc:
            messagebox.showerror("Данные импорта", str(exc))
            return
        self.import_busy = True
        self.btn_import.configure(state="disabled")
        config = dict(self.config)
        records_dir = self.records_base
        cache_dir = self._cache_directory()
        self.ui_results.put(("import-log", f"Импорт {channel}; даты UTC: {', '.join(map(str, dates))}"))

        def worker():
            try:
                from importer import fetch_day_log, import_logs_to_session
                days = []
                for date in dates:
                    self.ui_results.put(("import-log", f"Загрузка архива за {date}…"))
                    days.append(fetch_day_log(channel, str(date.year), str(date.month), str(date.day)))
                session_dir = import_logs_to_session(channel, days, videos, start_dt=start,
                    records_dir=records_dir, emote_cache_dir=cache_dir, config=config,
                    target_date=dates[0], auto_detect_start=auto_detect, timezone="UTC")
                with (session_dir / f"{session_dir.name}.session.json").open(encoding="utf-8") as file:
                    metadata = json.load(file)
                for warning in metadata.get("import_warnings", []):
                    self.ui_results.put(("import-log", "Предупреждение: " + str(warning)))
                self.ui_results.put(("import-log", "Создана сессия: " + session_dir.name))
                if not videos: self.ui_results.put(("import-log", "Сессия содержит только чат; видео не добавлено"))
            except FileExistsError:
                self.ui_results.put(("import-log", "Сессия уже существует; файлы не перезаписаны. Для явной замены используйте CLI importer --replace."))
            except Exception as exc:
                detail = str(exc)
                for key in ("twitch_client_secret", "twitch_refresh_token", "twitch_app_token"):
                    value = str(config.get(key, ""))
                    if value: detail = detail.replace(value, "[скрыто]")
                self.ui_results.put(("import-log", f"Ошибка импорта: {detail}"))
            finally:
                self.ui_results.put(("import-done", None))

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
