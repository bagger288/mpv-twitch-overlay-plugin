#!/usr/bin/env python3
"""Transactional Twitch archive importer; chat normalization needs no Pillow.

All datetimes default to UTC. Video files stay in place and session manifests
refer to their canonical absolute paths. Activity detection estimates an anchor,
not an authoritative live/offline boundary.
"""

import argparse
from collections import deque
import copy
import datetime
from decimal import Decimal, InvalidOperation
from email.utils import parsedate_to_datetime
import ipaddress
import json
import logging
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
from typing import List, Optional, Tuple, Union
import urllib.error
import urllib.parse
import urllib.request
import uuid

from chat_records import normalize_record, resolve_timezone, timestamp_ms

logger = logging.getLogger("importer")
USER_AGENT = "TwitchLogImporter/1.0 (+https://github.com)"
MAX_RESPONSE_BYTES = 32 * 1024 * 1024
BOT_NAMES = {
    "nightbot", "streamelements", "moobot", "fossabot", "stayhydratedbot", "wizebot",
    "soundalerts", "streamlabs", "kofi_bot", "botisimo", "buttsbot", "creakbot", "streamraiders",
}


class FetchError(RuntimeError):
    """Bounded archive request failed (messages never contain credentials)."""


def _validate_url(url: str, allowed_host: Optional[str] = None) -> str:
    try:
        parsed = urllib.parse.urlsplit(url)
        host = parsed.hostname
        port = parsed.port
    except (ValueError, TypeError) as exc:
        raise FetchError("Invalid archive URL") from exc
    if (parsed.scheme != "https" or not host or parsed.username or parsed.password
            or port not in (None, 443) or parsed.fragment):
        raise FetchError("Archive URL must be credential-free HTTPS on port 443")
    host = host.lower().rstrip(".")
    if allowed_host and host != allowed_host:
        raise FetchError("Cross-host archive redirect is not allowed")
    if host == "localhost" or host.endswith((".localhost", ".local")) or "." not in host:
        raise FetchError("Local archive URLs are not allowed")
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        address = None
    if address is not None and not address.is_global:
        raise FetchError("Non-public archive URLs are not allowed")
    return host


class _SafeRedirect(urllib.request.HTTPRedirectHandler):
    def __init__(self, host: str):
        self.host = host
        super().__init__()

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        _validate_url(newurl, allowed_host=self.host)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _retry_delay(headers, attempt: int) -> float:
    value = headers.get("Retry-After") if headers else None
    try:
        delay = float(value)
    except (TypeError, ValueError):
        try:
            dt = parsedate_to_datetime(value)
            if dt.tzinfo is None:
                dt = dt.replace(tzinfo=datetime.timezone.utc)
            delay = (dt - datetime.datetime.now(datetime.timezone.utc)).total_seconds()
        except (TypeError, ValueError, OverflowError):
            delay = 2 ** attempt
    if not 0 <= delay <= 30:
        delay = 30 if delay > 30 else 2 ** attempt
    return min(30.0, max(0.0, delay))


def fetch_json(url: str, *, timeout: float = 15.0, max_bytes: int = MAX_RESPONSE_BYTES,
               retries: int = 2, opener=None, sleep=None) -> Union[dict, list]:
    """Fetch bounded JSON over HTTPS, retrying only 429/5xx at most twice.

    opener/sleep are injectable for fully offline tests. Redirects are restricted
    to the original HTTPS host, and response/error bodies are never logged.
    """
    host = _validate_url(url)
    if not 0 < timeout <= 120 or not 0 < max_bytes <= 128 * 1024 * 1024 or not 0 <= retries <= 2:
        raise ValueError("Invalid request bounds")
    opener = opener or urllib.request.build_opener(_SafeRedirect(host)).open
    sleep = sleep or time.sleep
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "application/json"})
    for attempt in range(retries + 1):
        try:
            with opener(req, timeout=timeout) as resp:
                if hasattr(resp, "geturl"):
                    _validate_url(resp.geturl(), allowed_host=host)
                body = resp.read(max_bytes + 1)
                if len(body) > max_bytes:
                    raise FetchError("Archive response exceeds the size limit")
                try:
                    data = json.loads(body.decode("utf-8-sig"))
                except (UnicodeError, ValueError) as exc:
                    raise FetchError("Archive response is not valid JSON") from exc
                if not isinstance(data, (dict, list)):
                    raise FetchError("Archive JSON must be an object or array")
                return data
        except urllib.error.HTTPError as exc:
            status = exc.code
            delay = _retry_delay(exc.headers, attempt)
            try:
                exc.close()
            except (OSError, KeyError, AttributeError):
                pass
            if (status == 429 or 500 <= status <= 599) and attempt < retries:
                sleep(delay)
                continue
            raise FetchError(f"Archive request failed: HTTP {status}") from None
        except (urllib.error.URLError, OSError, TimeoutError):
            raise FetchError("Archive request failed: connection or timeout error") from None
    raise FetchError("Archive request failed")


def _channel_name(channel: str) -> str:
    if not isinstance(channel, str):
        raise ValueError("Channel must be a Twitch login")
    channel = channel.lower().lstrip("#")
    if not re.fullmatch(r"[a-z0-9_]{1,25}", channel):
        raise ValueError("Channel must contain only letters, numbers and underscore")
    return channel


def fetch_available_days(channel: str) -> List[dict]:
    channel = _channel_name(channel)
    data = fetch_json(f"https://bestlogs.lurkology.com/list?channel={urllib.parse.quote(channel)}")
    if not isinstance(data, dict) or not isinstance(data.get("availableLogs"), list):
        raise FetchError("Archive listing has no availableLogs array")
    if not all(isinstance(day, dict) for day in data["availableLogs"]):
        raise FetchError("Archive listing contains invalid day objects")
    return data["availableLogs"]


def fetch_day_log(channel: str, year: str, month: str, day: str) -> Union[dict, list]:
    channel = _channel_name(channel)
    date = datetime.date(int(year), int(month), int(day))
    data = fetch_json(f"https://bestlogs.lurkology.com/channel/{channel}/{date:%Y/%m/%d}?jsonBasic=1")
    try:
        _source_messages(data)
    except ValueError as exc:
        raise FetchError("Archive day has an invalid messages payload") from exc
    return data


def process_message_item(item: Union[dict, str], *, timezone="UTC") -> dict:
    """Normalize one bestlogs/IRC record; invalid timestamps raise ValueError."""
    return normalize_record(item, timezone=timezone)


def _source_messages(source: Union[dict, list]) -> list:
    if isinstance(source, list):
        messages = source
    elif isinstance(source, dict) and isinstance(source.get("messages"), list):
        messages = source["messages"]
    else:
        raise ValueError("Source must be a messages object or record array")
    if not all(isinstance(record, (dict, str)) for record in messages):
        raise ValueError("Source records must be objects or raw IRC strings")
    return messages


def detect_stream_start_ms(
    messages: List[dict], target_date: Optional[datetime.date] = None,
    min_streak: int = 10, max_gap_sec: float = 60.0, min_unique_users: int = 3,
    *, timezone="UTC", window_sec: float = 120.0,
) -> Tuple[int, str]:
    """Estimate an anchor from >=10 messages/3 users in a bounded <=120s window.

    max_gap_sec is retained for API compatibility, not a consecutive-gap streak.
    target_date never falls back to records from an earlier day in the selected
    timezone. A later day is allowed only as an explicitly labelled fallback.
    """
    zone = resolve_timezone(timezone)
    if not 0 < window_sec <= 120 or min_streak < 10 or min_unique_users < 3:
        raise ValueError("Density detection requires >=10 messages, >=3 users, and a <=120s window")
    candidates = sorted(messages, key=lambda m: m["ts"])
    if target_date:
        candidates = [m for m in candidates if datetime.datetime.fromtimestamp(m["ts"] / 1000, zone).date() >= target_date]
    if not candidates:
        raise ValueError("No valid messages on or after the requested date")
    human = [m for m in candidates if str(m.get("user", "")).lower() not in BOT_NAMES and m.get("kind", "message") == "message"]
    if target_date:
        on_date = [m for m in human if datetime.datetime.fromtimestamp(m["ts"] / 1000, zone).date() == target_date]
        human = on_date or human
    window = deque()
    for message in human:
        window.append(message)
        while window and message["ts"] - window[0]["ts"] > int(window_sec * 1000):
            window.popleft()
        users = {m.get("user") for m in window if m.get("user") and m.get("user") != "user"}
        if len(window) >= min_streak and len(users) >= min_unique_users:
            ts = window[0]["ts"]
            rendered = datetime.datetime.fromtimestamp(ts / 1000, zone).isoformat()
            return ts, f"Estimated stream start from chat density: {rendered} ({len(window)} messages within {window_sec:g}s from {len(users)} users; not an authoritative live boundary)"
    fallback = human[0] if human else candidates[0]
    rendered = datetime.datetime.fromtimestamp(fallback["ts"] / 1000, zone).isoformat()
    return fallback["ts"], f"Estimated anchor: first available {'viewer message' if human else 'event'} at {rendered}; no qualifying density burst, live boundaries unknown"


def probe_video_duration_ms(video_file: Path, *, ffprobe_path: Union[str, Path] = "ffprobe",
                            timeout: float = 15.0) -> Optional[int]:
    """Return a positive finite duration, or warn and return None (never run a shell)."""
    if not 0 < timeout <= 120:
        raise ValueError("ffprobe timeout must be between 0 and 120 seconds")
    try:
        result = subprocess.run(
            [str(ffprobe_path), "-v", "error", "-show_entries", "format=duration", "-of", "json", str(video_file)],
            capture_output=True, text=True, timeout=timeout, check=True,
        )
        value = json.loads(result.stdout).get("format", {}).get("duration")
        duration = Decimal(str(value))
        if not duration.is_finite() or duration <= 0:
            raise ValueError("Invalid duration")
        duration_ms = int(duration * 1000)
        if duration_ms <= 0:
            raise ValueError("Invalid duration")
        return duration_ms
    except (OSError, subprocess.SubprocessError, ValueError, TypeError, AttributeError, InvalidOperation, OverflowError):
        logger.warning("Video duration unavailable for %s; verify ffprobe_path and the video", video_file.name)
        return None


def _write_json(path: Path, data) -> None:
    with path.open("w", encoding="utf-8") as stream:
        json.dump(data, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def _deduplicate(messages: list) -> Tuple[list, int]:
    seen = set()
    unique = []
    for msg in sorted(messages, key=lambda m: m["ts"]):
        key = ("id", msg["id"]) if msg.get("id") else (
            "content", msg["ts"], msg.get("kind"), msg.get("user"), msg.get("text"), msg.get("msg_id"),
        )
        if key not in seen:
            seen.add(key)
            unique.append(msg)
    return unique, len(messages) - len(unique)


def _snapshot_emotes(stage: Path, session_name: str, channel: str, room_id: Optional[str],
                     cache_dir: Path, config: dict, session_json: dict, warnings: list) -> None:
    snapshot_path = stage / f"{session_name}.emotes.json"
    old_snapshot = {}
    old_snapshot_bytes = None
    snapshot_completed = False
    if snapshot_path.exists():
        old_snapshot_bytes = snapshot_path.read_bytes()
        with snapshot_path.open(encoding="utf-8") as stream:
            old_snapshot = json.load(stream)
        if not isinstance(old_snapshot, dict):
            raise ValueError("Existing emote snapshot must be an object")
    warning = "Current emote metadata is not a historical reconstruction of the imported chat; removed or renamed emotes may be missing."
    if not room_id:
        warnings.append("No room_id in source; emote/badge snapshot unavailable")
        session_json["emote_snapshot_status"] = "unavailable"
        session_json.setdefault("snapshot_captured_at", old_snapshot.get("snapshot_captured_at") or old_snapshot.get("captured_at"))
        if not old_snapshot:
            _write_json(snapshot_path, {"channel": channel, "room_id": None, "words": {}, "badges": {}, "snapshot_captured_at": None, "snapshot_status": "unavailable"})
        return
    # Do not import optional image dependencies during pure record processing.
    try:
        if config.get("snapshot_emotes", True) is False:
            raise RuntimeError("disabled")
        from emotes import EmoteManager
        manager = EmoteManager(
            cache_dir=cache_dir, twitch_client_id=config.get("twitch_client_id", ""),
            twitch_token=config.get("twitch_app_token", ""),
        )
        snapshot = manager.snapshot_session(
            snapshot_path, channel, str(room_id), download_all_images=False,
            capture={"imported_start_ms": session_json["session_start_ms"]},
        )
        if not isinstance(snapshot, dict) or not isinstance(snapshot.get("words"), dict) or not isinstance(snapshot.get("badges"), dict):
            raise ValueError("Invalid snapshot contract")
        captured = snapshot.get("captured_at") or snapshot.get("snapshot_captured_at")
        if not captured and not old_snapshot:
            captured = datetime.datetime.now(datetime.timezone.utc).isoformat()
        snapshot = {**old_snapshot, **snapshot, "snapshot_captured_at": captured, "historical_mismatch_warning": warning}
        _write_json(snapshot_path, snapshot)
        snapshot_completed = True
        session_json["snapshot_captured_at"] = captured
        session_json["historical_mismatch_warning"] = warning
        warnings.append(warning)
        # Emotes agent exposes snapshot_status/errors and an optional caching
        # report; older managers return None. Never claim historical coverage.
        status = snapshot.get("snapshot_status", "current_metadata_only")
        session_json["emote_snapshot_status"] = status
        provider_status = snapshot.get("provider_status", {})
        def scope_failed(entry):
            if isinstance(entry, dict):
                if entry.get("status") in ("partial", "failed", "unavailable", "error"):
                    return True
                return any(scope_failed(value) for value in entry.values() if isinstance(value, dict))
            return entry in ("partial", "failed", "unavailable", "error")
        provider_failed = scope_failed(provider_status)
        session_json["emote_provider_status"] = provider_status
        if status in ("partial", "failed", "unavailable") or snapshot.get("errors") or provider_failed:
            warnings.append("Emote metadata snapshot is incomplete; see snapshot diagnostics")
            session_json["emote_snapshot_status"] = "partial"
        cache_report = manager.cache_used_emotes_from_chat(stage / f"{session_name}.chat.jsonl", snapshot_path)
        if isinstance(cache_report, dict):
            session_json["emote_cache_report"] = cache_report
            if any(cache_report.get(key) for key in ("failed", "missing", "missing_metadata", "scan_errors", "errors")):
                warnings.append("Some used emotes/badges could not be cached")
                session_json["emote_snapshot_status"] = "partial"
    except Exception as exc:
        # No exception text: optional provider/auth failures may contain tokens.
        warnings.append(f"Emote snapshot/cache incomplete ({type(exc).__name__}); chat imported without full emote coverage")
        session_json["emote_snapshot_status"] = "partial" if session_json.get("snapshot_captured_at") else "unavailable"
        session_json.setdefault("snapshot_captured_at", old_snapshot.get("snapshot_captured_at") or old_snapshot.get("captured_at"))
        if old_snapshot_bytes is not None and not snapshot_completed:
            # A failed provider write must not corrupt the prior valid snapshot.
            snapshot_path.write_bytes(old_snapshot_bytes)
        elif not snapshot_path.exists() or not session_json.get("snapshot_captured_at"):
            _write_json(snapshot_path, {**old_snapshot, "channel": channel, "room_id": room_id,
                                       "words": old_snapshot.get("words", {}), "badges": old_snapshot.get("badges", {}),
                                       "snapshot_captured_at": old_snapshot.get("snapshot_captured_at") or old_snapshot.get("captured_at"), "snapshot_status": "unavailable"})


def _publish_stage(stage: Path, destination: Path, replace_existing: bool) -> None:
    """Rename a complete staged directory, restoring the original on failure."""
    backup = None
    if destination.exists():
        if not replace_existing:
            raise FileExistsError(f"Session already exists: {destination}; use replace_existing=True or --replace")
        backup = destination.with_name(f".{destination.name}.backup-{uuid.uuid4().hex}")
        destination.rename(backup)
    try:
        stage.rename(destination)
    except BaseException:
        if backup is not None:
            backup.rename(destination)
        raise
    if backup is not None:
        try:
            shutil.rmtree(backup)
        except OSError:
            logger.warning("Session replaced successfully; retained backup at %s", backup)


def import_logs_to_session(
    channel: str, days_data: List[Union[dict, list]], video_files: List[Path],
    start_dt: Optional[datetime.datetime] = None, records_dir: Path = Path("records"),
    emote_cache_dir: Path = Path("emote_cache"), config: Optional[dict] = None,
    target_date: Optional[datetime.date] = None, auto_detect_start: bool = True,
    *, replace_existing: bool = False, timezone=None, range_start=None, range_end=None,
) -> Path:
    """Import a session transactionally, preserving calibration on explicit replace.

    range_start/end accept ISO strings, datetime objects, or epoch milliseconds;
    the start is inclusive and the end exclusive. Raw day sources are never
    clipped. A density estimate alone does not remove offline chatter. Existing
    parts, offsets, source dumps and unknown manifest/files survive replacement.
    """
    channel = _channel_name(channel)
    config = dict(config or {})
    timezone = timezone if timezone is not None else config.get("timezone", "UTC")
    zone = resolve_timezone(timezone)
    warnings = []
    if not isinstance(days_data, (list, tuple)) or not days_data:
        raise ValueError("At least one archive source is required")
    all_messages = []
    skipped = 0
    for day_index, day in enumerate(days_data, 1):
        for record_index, item in enumerate(_source_messages(day), 1):
            try:
                all_messages.append(process_message_item(item, timezone=zone))
            except (ValueError, TypeError, OverflowError):
                skipped += 1
                logger.warning("Skipping invalid chat record %d in source %d (invalid timestamp or record)", record_index, day_index)
    if not all_messages:
        raise ValueError("No valid timestamped chat records; nothing imported")
    all_messages, duplicate_count = _deduplicate(all_messages)
    room_id = next((msg["room_id"] for msg in all_messages if msg.get("room_id")), None)
    if start_dt is not None:
        # A supplied manual anchor always wins, including older callers that
        # leave auto_detect_start at its default True.
        start_ms = timestamp_ms(start_dt, timezone=zone)
        detection_info = "Manual start time in specified timezone"
    elif auto_detect_start:
        start_ms, detection_info = detect_stream_start_ms(all_messages, target_date, timezone=zone)
    else:
        raise ValueError("start_dt is required when automatic detection is disabled")
    start_local = datetime.datetime.fromtimestamp(start_ms / 1000, zone)
    session_name = f"{channel}_{start_local:%d-%m-%Y_%H-%M-%S}"
    records_dir = Path(records_dir).expanduser().resolve()
    destination = records_dir / session_name
    existing = {}
    if destination.exists():
        if not replace_existing:
            raise FileExistsError(f"Session already exists: {destination}; use replace_existing=True or --replace")
        if not destination.is_dir() or destination.is_symlink():
            raise ValueError("Existing session must be a real directory")
        old_manifest = destination / f"{session_name}.session.json"
        if not old_manifest.is_file():
            raise ValueError("Refusing to replace a directory without its session manifest")
        with old_manifest.open(encoding="utf-8") as stream:
            existing = json.load(stream)
        if not isinstance(existing, dict):
            raise ValueError("Existing session manifest must be an object")
        if existing.get("session_start_ms") is not None:
            start_ms = timestamp_ms(existing["session_start_ms"])
            start_local = datetime.datetime.fromtimestamp(start_ms / 1000, zone)
            detection_info = existing.get("start_detection_info", "Preserved existing calibrated session start")
    cache_dir = Path(existing.get("emote_cache_dir") or emote_cache_dir).expanduser().resolve()
    # Resolve/validate every supplied video even when preserving calibrated parts.
    videos = [Path(video).expanduser().resolve(strict=True) for video in video_files]
    if any(not video.is_file() for video in videos):
        raise ValueError("Every video must be an existing regular file")
    parts = []
    offsets = {}
    cursor = start_ms
    if existing.get("parts") is not None:
        if not isinstance(existing["parts"], list):
            raise ValueError("Existing session parts must be an array")
        parts = copy.deepcopy(existing["parts"])
        for part in parts:
            if not isinstance(part, dict) or not isinstance(part.get("file"), str):
                raise ValueError("Existing session part is invalid")
            path = Path(part["file"]).expanduser()
            if not path.is_absolute():
                path = destination / path
            part["file"] = str(path.resolve(strict=True))
            if not Path(part["file"]).is_file():
                raise ValueError("Existing video part is not a regular file")
        offsets = copy.deepcopy(existing.get("offsets_ms", {}))
        warnings.append("Existing session start, video parts and offsets preserved; supplied videos do not replace calibration")
    else:
        for index, video in enumerate(videos):
            duration = probe_video_duration_ms(video, ffprobe_path=config.get("ffprobe_path", "ffprobe"),
                                               timeout=float(config.get("ffprobe_timeout", 15)))
            if duration is None and index < len(videos) - 1:
                raise ValueError("Cannot position subsequent video parts: preceding video duration is unknown")
            if duration is None:
                warnings.append("Video duration unavailable; live boundaries remain unknown and chat is not automatically trimmed")
            end = cursor + duration if duration is not None else None
            parts.append({"file": str(video), "start_ms": cursor, "end_ms": end})
            offsets[str(video)] = 0
            if end is not None:
                cursor = end
    clip_start = timestamp_ms(range_start, timezone=zone) if range_start is not None else None
    clip_end = timestamp_ms(range_end, timezone=zone) if range_end is not None else None
    if clip_start is not None and clip_end is not None and clip_end <= clip_start:
        raise ValueError("range_end must be later than range_start")
    if parts and any(isinstance(part.get("end_ms"), int) for part in parts):
        known_starts = [part.get("start_ms") for part in parts if isinstance(part.get("start_ms"), int)]
        if known_starts:
            clip_start = max(clip_start, min(known_starts)) if clip_start is not None else min(known_starts)
        if all(isinstance(part.get("end_ms"), int) for part in parts):
            video_end = max(part["end_ms"] for part in parts)
            clip_end = min(clip_end, video_end) if clip_end is not None else video_end
    output_messages = [msg for msg in all_messages if (clip_start is None or msg["ts"] >= clip_start)
                       and (clip_end is None or msg["ts"] < clip_end)]
    if skipped:
        warnings.append(f"Skipped {skipped} invalid chat records; original sources preserved")
    if not output_messages:
        warnings.append("No chat records inside the selected/video range; original sources preserved")
    session_json = {**existing, "channel": channel, "room_id": room_id or existing.get("room_id"),
                    "session_start_ms": start_ms, "start_time_iso": start_local.isoformat(),
                    "start_detection_info": detection_info,
                    "target_date": target_date.isoformat() if target_date else existing.get("target_date"),
                    "timezone": str(timezone), "parts": parts, "offsets_ms": offsets,
                    "emote_cache_dir": str(cache_dir), "range_start_ms": clip_start, "range_end_ms": clip_end,
                    "imported_message_count": len(output_messages), "invalid_record_count": skipped,
                    "duplicate_record_count": duplicate_count, "source_record_count": len(all_messages) + duplicate_count + skipped}
    records_dir.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{session_name}.import-", dir=records_dir))
    try:
        if existing:
            shutil.copytree(destination, stage, dirs_exist_ok=True, symlinks=True)
            # Generated paths must not follow links out of the staged session.
            for suffix in ("chat.jsonl", "session.json", "emotes.json"):
                generated = stage / f"{session_name}.{suffix}"
                if generated.is_symlink():
                    raise ValueError("Refusing to overwrite symlinked session output")
        source_files = list(existing.get("source_files", []))
        source_metadata = list(existing.get("source_metadata", []))
        index = 1
        for source in days_data:
            while ((stage / f"{session_name}.day_{index}.source.json").exists()
                   or (stage / f"{session_name}.day_{index}.source.json").is_symlink()):
                index += 1
            filename = f"{session_name}.day_{index}.source.json"
            _write_json(stage / filename, source)
            source_files.append(filename)
            metadata = {key: copy.deepcopy(value) for key, value in source.items() if key != "messages"} if isinstance(source, dict) else {}
            source_metadata.append({"file": filename, "metadata": metadata})
            index += 1
        session_json["source_files"] = source_files
        session_json["source_metadata"] = source_metadata
        chat_file = stage / f"{session_name}.chat.jsonl"
        with chat_file.open("w", encoding="utf-8") as stream:
            for message in output_messages:
                stream.write(json.dumps(message, ensure_ascii=False) + "\n")
        _snapshot_emotes(stage, session_name, channel, session_json["room_id"], cache_dir, config, session_json, warnings)
        session_json["import_warnings"] = list(dict.fromkeys(list(existing.get("import_warnings", [])) + warnings))
        _write_json(stage / f"{session_name}.session.json", session_json)
        _publish_stage(stage, destination, replace_existing)
    finally:
        if stage.exists():
            shutil.rmtree(stage)
    for warning in warnings:
        logger.warning("%s", warning)
    logger.info("Imported %d messages into %s", len(output_messages), destination)
    return destination


def _load_source(path: Path) -> Union[dict, list]:
    with path.open(encoding="utf-8-sig") as stream:
        if path.suffix.lower() == ".jsonl":
            data = [json.loads(line) for line in stream if line.strip()]
        else:
            data = json.load(stream)
    _source_messages(data)
    return data


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Import Twitch archives transactionally (default timezone UTC)")
    parser.add_argument("channel", help="Twitch channel login")
    parser.add_argument("--source", nargs="+", help="Local JSON/JSONL archives (kept for reprocessing)")
    parser.add_argument("--date", help="Comma-separated YYYY-MM-DD archive dates")
    parser.add_argument("--start-time", help="Manual ISO video start; naive values use --timezone/config timezone")
    parser.add_argument("--no-auto-detect", action="store_true", help="Require manual --start-time")
    parser.add_argument("--video", nargs="*", default=[], help="Existing external videos (not copied)")
    parser.add_argument("--replace", action="store_true", help="Explicitly replace a session, preserving calibration and unknown files")
    parser.add_argument("--timezone", help="UTC (default), IANA timezone, or +HH:MM")
    parser.add_argument("--range-start", help="Inclusive ISO chat clip start")
    parser.add_argument("--range-end", help="Exclusive ISO chat clip end")
    parser.add_argument("--config", help="Optional launcher config JSON (default: adjacent config.json if present)")
    parser.add_argument("--records-dir", help="Session root (default config records_dir or records)")
    parser.add_argument("--emote-cache-dir", help="Shared image cache (default config emote_cache_dir or emote_cache)")
    args = parser.parse_args(argv)
    config_path = Path(args.config).expanduser().resolve() if args.config else Path(__file__).resolve().with_name("config.json")
    config = {}
    try:
        if config_path.exists() or args.config:
            with config_path.open(encoding="utf-8-sig") as stream:
                config = json.load(stream)
            if not isinstance(config, dict):
                raise ValueError("Config must be an object")
        dates = [datetime.date.fromisoformat(value.strip()) for value in args.date.split(",")] if args.date else []
        if args.source and dates:
            raise ValueError("Choose --source or --date, not both")
        if not args.source and not dates:
            raise ValueError("Provide --source or --date")
        start_dt = datetime.datetime.fromisoformat(args.start_time.replace("Z", "+00:00")) if args.start_time else None
        days_data = [_load_source(Path(source)) for source in args.source] if args.source else [
            fetch_day_log(args.channel, str(date.year), str(date.month), str(date.day)) for date in dates]
        base = config_path.parent if config_path.exists() else Path.cwd()
        def configured_path(argument, key, default):
            path = Path(argument or config.get(key, default)).expanduser()
            return path if path.is_absolute() or argument else base / path
        destination = import_logs_to_session(
            channel=args.channel, days_data=days_data, video_files=[Path(video) for video in args.video],
            start_dt=start_dt, target_date=dates[0] if dates else None, auto_detect_start=not args.no_auto_detect,
            records_dir=configured_path(args.records_dir, "records_dir", "records"),
            emote_cache_dir=configured_path(args.emote_cache_dir, "emote_cache_dir", "emote_cache"),
            config=config, replace_existing=args.replace, timezone=args.timezone,
            range_start=args.range_start, range_end=args.range_end,
        )
    except (OSError, ValueError, FetchError, TypeError) as exc:
        # Do not print arbitrary config/JSON/provider exception bodies.
        if isinstance(exc, FileExistsError):
            logger.error("Session already exists; use --replace to preserve calibration during re-import")
        elif isinstance(exc, FetchError):
            logger.error("%s", exc)
        else:
            logger.error("Import failed (%s); check input files, dates, timezone and configuration", type(exc).__name__)
        return 1
    print(f"Imported session into: {destination} (check import_warnings for incomplete emote coverage)")
    return 0


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(asctime)s [Importer] %(levelname)s: %(message)s")
    raise SystemExit(main())
