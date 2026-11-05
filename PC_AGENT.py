import socketio
import requests
import subprocess
import time
import os
import shutil
import ctypes
import winsound
import threading
import sys
import json as _json

def _load_agent_config():
    """Load config from config.json in parent directory (same as launcher)."""
    config_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "config.json")
    if os.path.isfile(config_path):
        try:
            with open(config_path, "r", encoding="utf-8") as f:
                return _json.load(f)
        except Exception:
            pass
    return {}

_cfg = _load_agent_config()

# --- CONFIG ---
SERVER_URL = os.environ.get("SERVER_URL") or _cfg.get("server_url") or ""
TWITCH_PROXY_PLAYLIST = os.environ.get("TWITCH_PROXY_PLAYLIST") or _cfg.get("twitch_proxy_playlist") or ""
RECORDS_DIR = os.environ.get("RECORDS_DIR") or _cfg.get("records_dir") or "records"
STREAMLINK_PATH = os.environ.get("STREAMLINK_PATH") or _cfg.get("streamlink_path") or "streamlink"
POLL_INTERVAL = 10

# --- НАСТРОЙКИ УВЕДОМЛЕНИЙ О ДИСКЕ ---
WARN_FREE_GB = 3.0          # Предупредить заранее, когда осталось меньше 3 ГБ (запись НЕ останавливается)
FULL_DISK_MB = 150          # Если streamlink остановился и места меньше 150 МБ — значит, кончилось место на диске
ALERT_COOLDOWN = 54        # Пауза между повторными уведомлениями (5 минут)

# --- STATE ---
recording_threads = {}
stop_flags = {}
state_lock = threading.Lock()
last_warn_time = 0
last_full_time = 0

def get_free_space_mb(folder=None):
    if folder is None:
        folder = RECORDS_DIR
    """Возвращает количество свободных мегабайт на диске с папкой recordings."""
    os.makedirs(folder, exist_ok=True)
    total, used, free = shutil.disk_usage(os.path.abspath(folder))
    return free / (1024 * 1024)

def notify_windows(title, message, critical=False):
    """Показывает системное уведомление Windows и играет звук (в отдельном потоке)."""
    def _send():
        try:
            winsound.MessageBeep(winsound.MB_ICONHAND if critical else winsound.MB_ICONEXCLAMATION)
        except Exception:
            pass

        # Всплывающее Toast-уведомление в правом нижнем углу Windows
        ps_script = f"""
        [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null;
        $template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02);
        $textNodes = $template.GetElementsByTagName("text");
        $textNodes.Item(0).AppendChild($template.CreateTextNode("{title}")) > $null;
        $textNodes.Item(1).AppendChild($template.CreateTextNode("{message}")) > $null;
        $toast = [Windows.UI.Notifications.ToastNotification]::new($template);
        [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("PC Stream Agent").Show($toast);
        """
        try:
            subprocess.run(
                ["powershell", "-NoProfile", "-WindowStyle", "Hidden", "-Command", ps_script],
                creationflags=subprocess.CREATE_NO_WINDOW
            )
        except Exception:
            pass

        # Окно поверх всех программ, если запись уже оборвалась из-за 0 места
        if critical:
            try:
                ctypes.windll.user32.MessageBoxW(0, message, title, 0x00040030)
            except Exception:
                pass

    threading.Thread(target=_send, daemon=True).start()

def resolve_quality(raw_quality):
    """Приводит качество к формату fallback-цепочки для Streamlink."""
    if not raw_quality:
        return "best,audio_only"
    if raw_quality == "audio_only" or "," in raw_quality:
        return raw_quality
    return f"{raw_quality},audio_only"

def record_worker(login, stop_event, quality="best,audio_only"):
    global last_warn_time, last_full_time

    while not stop_event.is_set():
        os.makedirs(RECORDS_DIR, exist_ok=True)

        # Если перед стартом новой попытки место уже на нуле — уведомляем и ждем очистки
        free_mb = get_free_space_mb(RECORDS_DIR)
        if free_mb < FULL_DISK_MB:
            now = time.time()
            if now - last_full_time > ALERT_COOLDOWN:
                last_full_time = now
                msg = f"Запись {login} остановилась, так как на диске осталось всего {free_mb:.0f} МБ! Освободите место — запись сразу продолжится."
                print(f"\n[ALERT] {msg}")
                notify_windows("[ALERT] Запись остановлена: нет места!", msg, critical=True)
            if stop_event.wait(timeout=10):
                break
            continue
            
        timestamp = time.strftime('%d-%m-%Y_%H-%M-%S')
        filename = f"{RECORDS_DIR}/{login}_{timestamp}.ts"
        
        print(f"\n[REC] Connecting to twitch.tv/{login} (quality: {quality})...")
        
        cmd = [
            STREAMLINK_PATH,
            f"https://twitch.tv/{login}",
            quality,
            "-o", filename,
            "--twitch-disable-ads",
            "--retry-streams", "5",
            "--retry-max", "3",
        ]
        if TWITCH_PROXY_PLAYLIST:
            cmd.append(f"--twitch-proxy-playlist={TWITCH_PROXY_PLAYLIST}")
        
        popen_kwargs = {
            "stdout": sys.stdout,
            "stderr": sys.stderr,
        }
        if sys.platform == "win32":
            popen_kwargs["creationflags"] = subprocess.CREATE_NO_WINDOW

        try:
            process = subprocess.Popen(cmd, **popen_kwargs)
            ticks = 0
            while process.poll() is None:
                if stop_event.is_set():
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                    return
                
                ticks += 1

                # Каждые 3 минуты (180 секунд) выводим текущий размер файла и длительность записи
                if ticks % 180 == 0:
                    if os.path.exists(filename):
                        size_bytes = os.path.getsize(filename)
                        if size_bytes < 1024 * 1024:
                            size_str = f"{size_bytes / 1024:.1f} КБ"
                        elif size_bytes < 1024 * 1024 * 1024:
                            size_str = f"{size_bytes / (1024 * 1024):.1f} МБ"
                        else:
                            size_str = f"{size_bytes / (1024 * 1024 * 1024):.2f} ГБ"
                        m, s = divmod(ticks, 60)
                        h, m = divmod(m, 60)
                        dur_str = f"{h:02d}:{m:02d}:{s:02d}" if h else f"{m:02d}:{s:02d}"
                        print(f"[REC] {login}: записано {size_str} (длительность {dur_str})", flush=True)

                # Каждые 10 секунд проверяем, не подходит ли место к концу (просто предупреждаем, НЕ останавливая запись!)
                if ticks % 10 == 0:
                    free_gb = get_free_space_mb(RECORDS_DIR) / 1024.0
                    now = time.time()
                    if free_gb < WARN_FREE_GB and (now - last_warn_time > ALERT_COOLDOWN):
                        last_warn_time = now
                        msg = f"Идет запись ({login}), на диске осталось {free_gb:.2f} ГБ! Освободите место, чтобы стрим не прервался."
                        print(f"\n[WARN] {msg}", flush=True)
                        notify_windows("[WARN] Скоро закончится место на диске!", msg, critical=False)

                time.sleep(1)
            
            if stop_event.is_set():
                break
                
            if os.path.exists(filename) and os.path.getsize(filename) > 0:
                print(f"[OK] [FINISHED] Recording for {login} saved: {filename}")
            else:
                if os.path.exists(filename):
                    try:
                        os.remove(filename)
                    except OSError:
                        pass

            # Проверяем, не оборвался ли только что streamlink из-за того, что место кончилось в ноль
            free_mb_after = get_free_space_mb(RECORDS_DIR)
            if free_mb_after < FULL_DISK_MB:
                last_full_time = time.time()
                msg = f"Запись {login} прервалась — закончилось место на диске ({free_mb_after:.0f} МБ свободно)! Файл .ts сохранен. Освободите место для продолжения!"
                print(f"\n[ALERT] {msg}")
                notify_windows("[ALERT] Запись прервалась: диск полон!", msg, critical=True)
            else:
                print(f"[RETRY] [RECONNECT] Stream for {login} stopped or not found. Retrying in 10s...")

            if stop_event.wait(timeout=10):
                break
            
        except Exception as e:
            print(f"[ERROR] Error in worker for {login}: {e}")
            if stop_event.wait(timeout=10):
                break

def start_recording(login, raw_quality="best"):
    quality = resolve_quality(raw_quality)
    with state_lock:
        if login in recording_threads and recording_threads[login].is_alive():
            return
        print(f"[*] Initializing persistent monitor for: {login} (quality: {quality})")
        stop_event = threading.Event()
        stop_flags[login] = stop_event
        thread = threading.Thread(target=record_worker, args=(login, stop_event, quality), daemon=True)
        recording_threads[login] = thread
        thread.start()

def stop_recording(login):
    with state_lock:
        if login in stop_flags:
            print(f"[STOP] Ending recording attempts for: {login}")
            stop_flags[login].set()
            del stop_flags[login]
            del recording_threads[login]

# --- WebSocket & Polling Logic ---
sio = socketio.Client(
    reconnection=True,
    reconnection_attempts=0,
    reconnection_delay=2,
    reconnection_delay_max=15
)

@sio.event
def connect():
    print("[OK] [WS] Connected to Render server! Listening for real-time triggers...")

@sio.event
def disconnect():
    print("[WARN] [WS] Disconnected from server. Waiting for automatic reconnect...")

@sio.on("start_record")
def on_start_record(data):
    login = data.get("login")
    quality = data.get("quality", "best,audio_only")
    if login:
        start_recording(login, quality)

def print_streamer_list():
    """Fetches and prints all tracked streamers with their status, record flag, and quality."""
    try:
        response = requests.get(f"{SERVER_URL}/api/streamers", timeout=30)
        if response.status_code != 200:
            print(f"[WARN] Could not fetch streamer list (HTTP {response.status_code}).")
            return

        streamers = response.json()
        if not streamers:
            print("[LIST] No streamers are currently being tracked.")
            return

        print(f"[LIST] Tracked Streamers ({len(streamers)}):")
        for s in streamers:
            login = s.get('login', '?')
            status = s.get('status', 'unknown')
            record_enabled = s.get('record', False)
            quality = s.get('quality', 'best')

            status_icon = "[ON]" if status == "online" else "[--]"
            record_label = "ON" if record_enabled else "OFF"

            print(f"   {status_icon} {login:<20} status={status:<8} record={record_label:<3} quality={quality}")
    except Exception as e:
        print(f"[WARN] Could not fetch streamer list: {e}")

def check_status():
    """Queries the server to see who is currently online and acts immediately."""
    try:
        response = requests.get(f"{SERVER_URL}/api/streamers", timeout=15)
        if response.status_code == 200:
            streamers = response.json()
            active_logins = set()

            for s in streamers:
                login = s['login']
                active_logins.add(login)
                is_online = s.get('status') == "online"
                record_enabled = s.get('record', False)

                if is_online and record_enabled:
                    start_recording(login, s.get("quality", "best"))
                else:
                    stop_recording(login)

            with state_lock:
                currently_recording = list(stop_flags.keys())
            for rec_login in currently_recording:
                if rec_login not in active_logins:
                    stop_recording(rec_login)

    except Exception:
        pass

def poll_server():
    while True:
        check_status()
        time.sleep(POLL_INTERVAL)

if __name__ == "__main__":
    sys.stdout.reconfigure(encoding='utf-8', errors='replace', line_buffering=True)
    if not SERVER_URL:
        print("[ERROR] SERVER_URL не настроен! Укажите адрес сервера во вкладке 'Настройки' лаунчера.")
        sys.exit(1)
    free_gb = get_free_space_mb() / 1024.0
    print(f"[*] PC Agent Started | Free Disk Space: {free_gb:.2f} GB")

    print_streamer_list()

    print("[CHECK] Checking current streamer statuses...")
    check_status()

    threading.Thread(target=poll_server, daemon=True).start()
    
    print("[NET] Connecting to bot via Socket.IO...")
    while True:
        try:
            if not sio.connected:
                sio.connect(SERVER_URL, transports=['websocket', 'polling'], wait_timeout=20)
            sio.wait()
        except KeyboardInterrupt:
            print("\n[BYE] Exiting PC Agent...")
            sys.exit(0)
        except Exception as e:
            print(f"[WAIT] Connection failed ({e}), retrying in 10s...")
            time.sleep(10)