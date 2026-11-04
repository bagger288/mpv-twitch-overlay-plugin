#!/usr/bin/env python3
"""
Twitch Stream and Chat Recorder
- Runs Streamlink as an external process to record TS video stream.
- Manages automatic reconnection and splitting into timestamped parts upon drops.
- Concurrently runs asynchronous IRC Chat Logger throughout the session.
- Captures emote snapshot once room_id is detected.
- Manages session.json and offsets.
"""

import argparse
import asyncio
import datetime
import json
import logging
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from typing import Optional

from chat_logger import TwitchChatLogger

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] (%(name)s) %(message)s",
    datefmt="%H:%M:%S",
)
logger = logging.getLogger("recorder")


def format_timestamp(dt: Optional[datetime.datetime] = None) -> str:
    """Formats datetime as dd-mm-yyyy_hh-mm-ss for Windows & Unix filesystem compatibility."""
    if dt is None:
        dt = datetime.datetime.now()
    return dt.strftime("%d-%m-%Y_%H-%M-%S")


class TwitchRecorder:
    def __init__(self, channel: str, config: Optional[dict] = None, session_dir: Optional[Path] = None):
        self.channel = channel.lower().lstrip("#")
        self.config = config or {}
        self.streamlink_bin = self.config.get("streamlink_path", "streamlink")
        self.quality = self.config.get("default_quality", "best")
        self.reconnect_delay = self.config.get("reconnect_delay_sec", 5)
        self.offline_timeout_min = self.config.get("offline_timeout_min", 10)

        # Base directories
        base_records = Path(self.config.get("records_dir", "records"))
        base_emote_cache = Path(self.config.get("emote_cache_dir", "emote_cache"))

        now_dt = datetime.datetime.now()
        self.session_prefix = f"{self.channel}_{format_timestamp(now_dt)}"
        self.session_dir = session_dir or (base_records / self.session_prefix)
        self.session_dir.mkdir(parents=True, exist_ok=True)

        self.session_file = self.session_dir / f"{self.session_prefix}.session.json"
        self.chat_file = self.session_dir / f"{self.session_prefix}.chat.jsonl"
        self.emotes_file = self.session_dir / f"{self.session_prefix}.emotes.json"

        self.session_data = {
            "channel": self.channel,
            "room_id": None,
            "session_start_ms": int(now_dt.timestamp() * 1000),
            "parts": [],
            "offsets_ms": {},
        }
        self.save_session()

        self.chat_logger = TwitchChatLogger(
            channel=self.channel,
            output_file=self.chat_file,
            on_room_id=self._on_room_id_discovered,
        )

        self.current_proc: Optional[subprocess.Popen] = None
        self.current_part_idx: Optional[int] = None
        self.is_running = True

    def save_session(self):
        try:
            with open(self.session_file, "w", encoding="utf-8") as f:
                json.dump(self.session_data, f, ensure_ascii=False, indent=2)
        except Exception as e:
            logger.error(f"Error saving session.json: {e}")

    def _on_room_id_discovered(self, room_id: str):
        self.session_data["room_id"] = room_id
        self.save_session()

    def terminate_current_proc(self):
        if self.current_proc and self.current_proc.poll() is None:
            logger.info("Terminating current Streamlink process...")
            try:
                self.current_proc.terminate()
                try:
                    self.current_proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.current_proc.kill()
            except Exception as e:
                logger.debug(f"Error stopping streamlink: {e}")

    def handle_shutdown(self):
        logger.info("Initiating graceful recorder shutdown...")
        self.is_running = False
        self.terminate_current_proc()
        now_ms = int(time.time() * 1000)
        if self.current_part_idx is not None and self.current_part_idx < len(self.session_data["parts"]):
            part = self.session_data["parts"][self.current_part_idx]
            if "end_ms" not in part or part["end_ms"] is None:
                part["end_ms"] = now_ms
        self.save_session()
        self.chat_logger.stop()

    async def run_streamlink_loop(self):
        logger.info(f"Starting Streamlink video capture loop for #{self.channel}...")
        consecutive_offline_seconds = 0
        offline_threshold_seconds = self.offline_timeout_min * 60

        while self.is_running:
            part_ts = format_timestamp()
            part_filename = f"{self.channel}_{part_ts}.ts"
            part_path = self.session_dir / part_filename

            start_ms = int(time.time() * 1000)
            part_info = {
                "file": part_filename,
                "start_ms": start_ms,
                "end_ms": None,
            }
            self.session_data["parts"].append(part_info)
            if part_filename not in self.session_data["offsets_ms"]:
                self.session_data["offsets_ms"][part_filename] = self.config.get("default_offset_ms", 0)
            self.current_part_idx = len(self.session_data["parts"]) - 1
            self.save_session()

            # Build Streamlink command
            # Canonical Twitch URL with https scheme
            target_url = f"https://www.twitch.tv/{self.channel}"

            # Quality with fallbacks in case 'best' is not available or transcoding is disabled
            qual = self.quality
            if qual == "best":
                qual = "best,1080p60,1080p,720p60,720p,480p,worst"

            # Base streamlink invocation
            # Using sys.executable -m streamlink avoids broken streamlink.exe wrapper stubs
            if self.streamlink_bin == "streamlink":
                base_cmd = [sys.executable, "-m", "streamlink"]
            else:
                base_cmd = [self.streamlink_bin]

            cmd = base_cmd + [
                target_url,
                qual,
                "-o",
                str(part_path.resolve()),
                "--force",
                "--twitch-disable-ads",
                "--retry-streams", "3",
                "--retry-max", "3",
            ]
            proxy_playlist = self.config.get("twitch_proxy_playlist", "")
            if proxy_playlist:
                cmd.append(f"--twitch-proxy-playlist={proxy_playlist}")

            extra_args = self.config.get("streamlink_extra_args", [])
            if extra_args:
                cmd.extend(extra_args)

            logger.info(f"Launching: {' '.join(cmd)}")
            try:
                # Run streamlink in subprocess
                self.current_proc = subprocess.Popen(
                    cmd,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    universal_newlines=True,
                    bufsize=1,
                )
            except FileNotFoundError:
                logger.error(f"Streamlink executable '{self.streamlink_bin}' not found! Please check PATH or config.json.")
                self.handle_shutdown()
                return

            # Read streamlink output continuously in background thread so no message is lost
            import threading
            def stream_reader():
                if not self.current_proc or not self.current_proc.stdout:
                    return
                for line in iter(self.current_proc.stdout.readline, ""):
                    if not line:
                        break
                    stripped = line.strip()
                    if stripped:
                        logger.info(f"[Streamlink] {stripped}")
                        if "error: No playable streams found on this URL" in stripped or "offline" in stripped.lower():
                            logger.info(f"Streamer #{self.channel} is currently offline.")

            reader_thread = threading.Thread(target=stream_reader, daemon=True)
            reader_thread.start()

            # Wait while process is running
            while self.is_running and self.current_proc.poll() is None:
                await asyncio.sleep(0.5)

            # Wait for remaining output to be read
            reader_thread.join(timeout=1.5)

            return_code = self.current_proc.poll()
            if return_code is not None and return_code != 0:
                logger.warning(f"Streamlink exited with code: {return_code}")

            end_ms = int(time.time() * 1000)
            part_info["end_ms"] = end_ms
            self.save_session()

            # Check if file was created and has data
            if part_path.exists() and part_path.stat().st_size > 0:
                duration_sec = (end_ms - start_ms) / 1000
                logger.info(f"Video part {part_filename} recorded ({duration_sec:.1f}s, {part_path.stat().st_size // 1024} KB).")
                consecutive_offline_seconds = 0
            else:
                logger.info(f"No video data was written for part {part_filename} (offline or stream ended).")
                # Remove empty record from parts if zero bytes
                if part_path.exists() and part_path.stat().st_size == 0:
                    part_path.unlink()
                self.session_data["parts"].pop()
                self.session_data["offsets_ms"].pop(part_filename, None)
                self.save_session()

                consecutive_offline_seconds += self.reconnect_delay
                if consecutive_offline_seconds >= offline_threshold_seconds:
                    logger.info(f"Streamer offline for {self.offline_timeout_min} minutes. Auto-stopping recorder.")
                    self.handle_shutdown()
                    break

            if not self.is_running:
                break

            logger.info(f"Waiting {self.reconnect_delay}s before next recording check...")
            await asyncio.sleep(self.reconnect_delay)

    async def main_loop(self):
        # Register signal handlers
        loop = asyncio.get_running_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            try:
                loop.add_signal_handler(sig, self.handle_shutdown)
            except NotImplementedError:
                # Windows doesn't support add_signal_handler for all signals
                pass

        # Concurrently run chat logger and streamlink video recorder
        chat_task = asyncio.create_task(self.chat_logger.start())
        video_task = asyncio.create_task(self.run_streamlink_loop())

        try:
            await asyncio.gather(chat_task, video_task)
        except asyncio.CancelledError:
            pass
        finally:
            self.handle_shutdown()


def load_config() -> dict:
    cfg_path = Path("config.json")
    if cfg_path.exists():
        try:
            with open(cfg_path, "r", encoding="utf-8") as f:
                return json.load(f)
        except Exception as e:
            logger.warning(f"Could not load config.json: {e}")
    return {}


def main():
    parser = argparse.ArgumentParser(description="Twitch Stream & Chat Recorder")
    parser.add_argument("channel", help="Twitch channel name (e.g. shroud or evelone192)")
    parser.add_argument("--quality", default=None, help="Stream quality (e.g. best, 1080p60, 720p)")
    parser.add_argument("--dir", default=None, help="Custom output directory")
    args = parser.parse_args()

    config = load_config()
    if args.quality:
        config["default_quality"] = args.quality

    custom_dir = Path(args.dir) if args.dir else None
    recorder = TwitchRecorder(args.channel, config=config, session_dir=custom_dir)

    print(f"==================================================")
    print(f" Recording Twitch channel: {recorder.channel}")
    print(f" Session directory:        {recorder.session_dir}")
    print(f" Chat log:                 {recorder.chat_file}")
    print(f" Press Ctrl+C to stop recording cleanly.")
    print(f"==================================================")

    try:
        asyncio.run(recorder.main_loop())
    except KeyboardInterrupt:
        recorder.handle_shutdown()


if __name__ == "__main__":
    main()
