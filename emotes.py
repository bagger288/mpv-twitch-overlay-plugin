#!/usr/bin/env python3
"""Static first-frame emote snapshots and bounded, on-demand PNG caching.

A snapshot records *current* provider metadata, not historical emote membership.
Existing snapshots are reused unless refresh=True; failed scopes retain their
previous entries. Images remain at source resolution for the renderer to scale.
"""

import datetime
import hashlib
import io
import json
import logging
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import tempfile
import time
from typing import Dict, Optional
import urllib.error
import urllib.parse
import urllib.request
import warnings
from email.utils import parsedate_to_datetime

from PIL import Image

logger = logging.getLogger("emotes")
USER_AGENT = "TwitchMpvOverlay/1.0 (+https://github.com)"
MAX_JSON_BYTES = 16 * 1024 * 1024
MAX_IMAGE_BYTES = 16 * 1024 * 1024
MAX_IMAGE_PIXELS = 16 * 1024 * 1024
MAX_IMAGE_DIMENSION = 8192
MAX_RETRIES = 2
MAX_RETRY_DELAY = 5.0
MAX_ANIMATION_FRAMES = 600
MAX_ANIMATION_PIXELS = 64 * 1024 * 1024
ANIMATION_KEYS = ("animation_file", "animation_format", "animated")
CACHE_VERSION = "static-v2"
PROVIDERS = ("7tv", "bttv", "ffz")  # Highest priority first.
ASCII_WHITESPACE = " \t\n\r\f"  # Rust split_ascii_whitespace excludes vertical tab.


def _https_url(url: str) -> str:
    if not isinstance(url, str) or any(ord(c) < 32 or c.isspace() for c in url):
        raise ValueError("Remote image/API URL must be a valid HTTPS URL")
    parts = urllib.parse.urlsplit(url)
    if (parts.scheme.lower() != "https" or not parts.hostname or
            parts.username is not None or parts.password is not None or "\\" in url):
        raise ValueError("Only HTTPS URLs without embedded credentials are allowed")
    # Accessing port validates malformed ports, too.
    _ = parts.port
    return url


def _origin(url):
    parts = urllib.parse.urlsplit(_https_url(url))
    return parts.hostname.lower(), parts.port or 443


class _SafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Allow HTTPS redirects, but never forward credentials across origins."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        _https_url(newurl)
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is not None and _origin(req.full_url) != _origin(newurl):
            for key in list(redirected.headers) + list(redirected.unredirected_hdrs):
                if key.lower() != "user-agent":
                    redirected.remove_header(key)
        return redirected


def _open_request(request):
    return urllib.request.build_opener(_SafeRedirectHandler()).open(request, timeout=10)


def _read_bounded(response, limit):
    length = response.headers.get("Content-Length")
    if length is not None and int(length) > limit:
        raise ValueError("Response exceeds resource limit")
    data = bytearray()
    while True:
        chunk = response.read(min(65536, limit + 1 - len(data)))
        if not chunk:
            return bytes(data)
        data.extend(chunk)
        if len(data) > limit:
            raise ValueError("Response exceeds resource limit")


def _retry_delay(header, attempt):
    try:
        delay = float(header)
    except (TypeError, ValueError):
        try:
            dt = parsedate_to_datetime(header)
            if dt.tzinfo is None:
                dt = dt.replace(tzinfo=datetime.timezone.utc)
            delay = (dt - datetime.datetime.now(datetime.timezone.utc)).total_seconds()
        except (TypeError, ValueError, OverflowError):
            delay = 0.5 * (2 ** attempt)
    return min(MAX_RETRY_DELAY, max(0.0, delay))


def _request_bytes(url, headers=None, limit=MAX_JSON_BYTES):
    _https_url(url)
    req_headers = {"User-Agent": USER_AGENT}
    req_headers.update(headers or {})
    for attempt in range(MAX_RETRIES + 1):
        try:
            with _open_request(urllib.request.Request(url, headers=req_headers)) as response:
                return _read_bounded(response, limit)
        except urllib.error.HTTPError as exc:
            retry = exc.code == 429 or 500 <= exc.code < 600
            retry_after = exc.headers.get("Retry-After") if exc.headers else None
            try:
                exc.close()
            except (OSError, KeyError, AttributeError):
                pass
            if not retry or attempt == MAX_RETRIES:
                raise
            time.sleep(_retry_delay(retry_after, attempt))


def _failure_reason(exc):
    """Never include exception text, request URLs, headers, or credentials."""
    if isinstance(exc, urllib.error.HTTPError):
        code = exc.code
        if code in (401, 403):
            return f"HTTP {code}; check the current Twitch token/client ID and permissions"
        if code == 404:
            return "HTTP 404; provider endpoint or channel metadata is unavailable"
        if code == 429:
            return "HTTP 429 after bounded retries; retry later"
        return f"HTTP {code}; retry later or check provider availability"
    if isinstance(exc, (urllib.error.URLError, TimeoutError, OSError)):
        return "network or filesystem request failed; check connectivity and cache permissions"
    return "invalid or oversized metadata/image; check the source and retry"


def fetch_json(url: str, headers: Optional[dict] = None):
    """Return decoded JSON (including []/{}) or None on request/JSON failure."""
    try:
        _https_url(url)
    except Exception:
        logger.warning("Metadata URL rejected; only credential-free HTTPS sources are permitted")
        return None
    try:
        return json.loads(_request_bytes(url, headers, MAX_JSON_BYTES).decode("utf-8"))
    except Exception as exc:
        logger.warning("Metadata request failed: %s", _failure_reason(exc))
        return None


def _check_dimensions(image):
    width, height = image.size
    if (width <= 0 or height <= 0 or width > MAX_IMAGE_DIMENSION or
            height > MAX_IMAGE_DIMENSION or width * height > MAX_IMAGE_PIXELS):
        raise ValueError("Image dimensions exceed resource limit")


def _valid_cached_png(path: Path) -> bool:
    try:
        if not path.is_file() or not 0 < path.stat().st_size <= MAX_IMAGE_BYTES:
            return False
        with warnings.catch_warnings():
            warnings.simplefilter("error", Image.DecompressionBombWarning)
            with Image.open(path) as image:
                if image.format != "PNG":
                    return False
                _check_dimensions(image)
                image.load()  # Decode, not just a successful header sniff.
                return True
    except Exception:
        return False


def download_image_to_png(url: str, dest_path: Path, target_height: int = 56, force=False) -> bool:
    """Atomically cache the first frame; retain source pixels, never scale to 28.

    target_height remains accepted for call compatibility. It is intentionally
    not applied: Rust performs the only resize at the final draw dimensions.
    Callers handling untrusted metadata must first resolve it within their cache.
    """
    dest_path = Path(dest_path)
    if url is not None:
        try:
            _https_url(url)
        except Exception:
            logger.warning("Image URL rejected; only credential-free HTTPS sources are permitted")
            return False
    if _valid_cached_png(dest_path) and not force:
        return True
    temporary = None
    try:
        data = _request_bytes(url, limit=MAX_IMAGE_BYTES)
        with warnings.catch_warnings():
            warnings.simplefilter("error", Image.DecompressionBombWarning)
            with Image.open(io.BytesIO(data)) as image:
                _check_dimensions(image)
                image.seek(0)
                image.load()
                frame = image.convert("RGBA")
        dest_path.parent.mkdir(parents=True, exist_ok=True)
        fd, name = tempfile.mkstemp(prefix=dest_path.name + ".", suffix=".tmp", dir=dest_path.parent)
        temporary = Path(name)
        with os.fdopen(fd, "wb") as output:
            frame.save(output, "PNG")
            output.flush()
            os.fsync(output.fileno())
        frame.close()
        if not _valid_cached_png(temporary):
            raise ValueError("Converted PNG failed validation")
        os.replace(temporary, dest_path)
        animated_format = None
        try:
            animated_format = _animation_info(data)
            if animated_format:
                _atomic_bytes(dest_path.with_suffix("." + animated_format), data)
        except Exception as exc:
            animated_format = None
            logger.warning("Animation unavailable; PNG preview kept: %s", _failure_reason(exc))
        # Never leave an animation from an older source next to a new preview.
        for extension in ("webp", "gif"):
            if extension != animated_format:
                try:
                    dest_path.with_suffix("." + extension).unlink(missing_ok=True)
                except OSError:
                    logger.warning("Stale animation cleanup failed; check cache directory permissions")
        # Written last: a marker means the PNG and animation for this URL are published.
        try:
            _atomic_bytes(dest_path.with_suffix(".animation-check.json"), json.dumps({"url": url, "checked": True}).encode())
        except OSError:
            logger.warning("Animation check marker not written; the image will be re-checked later")
        return True
    except Exception as exc:
        logger.warning("Image caching failed: %s", _failure_reason(exc))
        return False
    finally:
        if temporary is not None:
            try:
                temporary.unlink(missing_ok=True)
            except OSError:
                logger.warning("Temporary image cleanup failed; check cache directory permissions")


def _animation_info(data):
    with warnings.catch_warnings():
        warnings.simplefilter("error", Image.DecompressionBombWarning)
        with Image.open(io.BytesIO(data)) as image:
            if image.format not in {"WEBP", "GIF"}: return None
            _check_dimensions(image)
            count = getattr(image, "n_frames", 1)
            if count <= 1: return None
            if count > MAX_ANIMATION_FRAMES or image.width * image.height * count > MAX_ANIMATION_PIXELS:
                raise ValueError("Animation exceeds frame or decoded pixel limits")
            for index in range(count):
                image.seek(index)
                image.load()
            return image.format.lower()


def _atomic_bytes(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix=path.name + ".", suffix=".tmp", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as file:
            file.write(data); file.flush(); os.fsync(file.fileno())
        os.replace(name, path)
    finally:
        Path(name).unlink(missing_ok=True)


def _relative_png(value, prefix=None):
    if not isinstance(value, str) or not value or "\\" in value or ":" in value or "\x00" in value:
        raise ValueError("Cache file must be a safe relative PNG path")
    path = PurePosixPath(value)
    if (path.is_absolute() or PureWindowsPath(value).is_absolute() or
            any(part in ("", ".", "..") for part in value.split("/")) or
            any(not re.fullmatch(r"[A-Za-z0-9_.-]+", part) or part.endswith(".") or
                part.split(".", 1)[0].upper() in {"CON", "PRN", "AUX", "NUL", *(f"COM{i}" for i in range(1, 10)), *(f"LPT{i}" for i in range(1, 10))}
                for part in path.parts) or
            path.suffix.lower() != ".png" or
            (prefix and path.parts[0] != prefix) or
            path.parts[0] not in (*PROVIDERS, "badges", "twitch")):
        raise ValueError("Cache file must stay inside its provider directory")
    return path


def _identifier(value):
    if value is None or isinstance(value, (dict, list, bool)) or not str(value):
        raise ValueError("Missing metadata identifier")
    return str(value)


def _remote_url(value):
    if isinstance(value, str) and value.startswith("//"):
        value = "https:" + value
    return _https_url(value)


def _cache_filename(provider, identifier, url):
    # Hash the full ID and URL together; sanitized slugs alone can collide.
    slug = re.sub(r"[^A-Za-z0-9_-]", "_", identifier)[:48] or "image"
    digest = hashlib.sha256((CACHE_VERSION + "\0" + identifier + "\0" + url).encode("utf-8")).hexdigest()[:24]
    return f"{provider}/{slug}-{digest}.png"


def _emote_meta(provider, identifier, url, scope):
    identifier = _identifier(identifier)
    url = _remote_url(url)
    return {"provider": provider, "source": provider, "id": identifier, "url": url,
            "file": _cache_filename(provider, identifier, url), "scope": scope,
            "cache_version": CACHE_VERSION}


class EmoteManager:
    def __init__(self, cache_dir: Path, twitch_client_id: str = "", twitch_token: str = ""):
        self.cache_dir = Path(cache_dir).expanduser().resolve()
        self.twitch_client_id = twitch_client_id
        self.twitch_token = twitch_token
        self.provider_status = {}
        self._fetched_scopes = {}
        self.twitch_cache = self.cache_dir / "twitch"
        self.seven_tv_cache = self.cache_dir / "7tv"
        self.bttv_cache = self.cache_dir / "bttv"
        self.ffz_cache = self.cache_dir / "ffz"
        self.badge_cache = self.cache_dir / "badges"
        for prefix in ("twitch", *PROVIDERS, "badges"):
            directory = self.cache_dir / prefix
            self._inside_cache(directory)
            directory.mkdir(parents=True, exist_ok=True)

    def _inside_cache(self, path):
        resolved = Path(path).resolve()
        if not resolved.is_relative_to(self.cache_dir):
            raise ValueError("Cache path escapes the configured root")
        return resolved

    def _destination(self, meta, prefix):
        relative = _relative_png(meta.get("file") or meta.get("local_path"), prefix)
        return self._inside_cache(self.cache_dir.joinpath(*relative.parts))

    def _fetch_scopes(self, provider, room_id, endpoints, parser, headers=None):
        self.provider_status[provider] = {}
        self._fetched_scopes[provider] = {}
        combined = {}
        for scope in ("global", "channel"):
            if scope == "channel" and not room_id:
                self.provider_status[provider][scope] = {
                    "status": "skipped", "count": 0, "reason": "room ID unavailable; channel request not made"}
                continue
            try:
                payload = fetch_json(endpoints[scope], headers=headers) if headers else fetch_json(endpoints[scope])
                if payload is None:
                    raise ValueError("Request failed")
                entries = parser(payload, scope)
                self._fetched_scopes[provider][scope] = entries
                self.provider_status[provider][scope] = {"status": "ok", "count": len(entries)}
                combined.update(entries)
            except Exception:
                self.provider_status[provider][scope] = {
                    "status": "failed", "count": 0,
                    "reason": "request failed or provider response invalid; retry later (Twitch: check current credentials)"}
                logger.warning("%s %s metadata failed; previous snapshot entries will be retained", provider, scope)
        return combined

    def fetch_7tv_emotes(self, room_id: Optional[str] = None) -> Dict[str, dict]:
        def parse(payload, scope):
            if not isinstance(payload, dict):
                raise ValueError("Invalid 7TV response")
            if scope == "channel":
                if "emote_set" not in payload:
                    raise ValueError("Missing 7TV channel set")
                payload = payload["emote_set"]
                if payload is None:
                    return {}
            if not isinstance(payload, dict) or not isinstance(payload.get("emotes"), list):
                raise ValueError("Invalid 7TV set")
            entries = {}
            for item in payload["emotes"]:
                name = item["name"]
                if not isinstance(name, str) or not name:
                    raise ValueError("Invalid emote name")
                data = item["data"]
                host = data["host"]
                files = host["files"]
                # Prefer 2x source resolution, deterministic regardless of API ordering.
                candidates = [f["name"] for f in files if isinstance(f, dict) and isinstance(f.get("name"), str)]
                selected = next((n for n in ("2x.webp", "1x.webp", "3x.webp", "4x.webp", "2x.gif", "1x.gif", "2x.png", "1x.png")
                                 if n in candidates), None)
                if selected is None:
                    raise ValueError("No supported 7TV image")
                url = _remote_url(host["url"]).rstrip("/") + "/" + selected
                entries[name] = _emote_meta("7tv", item.get("id") or data.get("id"), url, scope)
            return entries
        rid = urllib.parse.quote(str(room_id or ""), safe="")
        return self._fetch_scopes("7tv", room_id, {
            "global": "https://7tv.io/v3/emote-sets/global",
            "channel": f"https://7tv.io/v3/users/twitch/{rid}"}, parse)

    def fetch_bttv_emotes(self, room_id: Optional[str] = None) -> Dict[str, dict]:
        def parse(payload, scope):
            if scope == "global":
                if not isinstance(payload, list):
                    raise ValueError("Invalid BTTV global response")
                items = payload
            else:
                if (not isinstance(payload, dict) or not isinstance(payload.get("channelEmotes"), list) or
                        not isinstance(payload.get("sharedEmotes"), list)):
                    raise ValueError("Invalid BTTV channel response")
                items = payload["sharedEmotes"] + payload["channelEmotes"]
            entries = {}
            for item in items:
                name = item["code"]
                if not isinstance(name, str) or not name:
                    raise ValueError("Invalid emote name")
                eid = _identifier(item["id"])
                url = "https://cdn.betterttv.net/emote/" + urllib.parse.quote(eid, safe="") + "/2x.png"
                entries[name] = _emote_meta("bttv", eid, url, scope)
            return entries
        rid = urllib.parse.quote(str(room_id or ""), safe="")
        return self._fetch_scopes("bttv", room_id, {
            "global": "https://api.betterttv.net/3/cached/emotes/global",
            "channel": f"https://api.betterttv.net/3/cached/users/twitch/{rid}"}, parse)

    def fetch_ffz_emotes(self, room_id: Optional[str] = None) -> Dict[str, dict]:
        def parse(payload, scope):
            if not isinstance(payload, dict) or not isinstance(payload.get("sets"), dict):
                raise ValueError("Invalid FFZ sets")
            entries = {}
            for key in sorted(payload["sets"]):
                items = payload["sets"][key]["emoticons"]
                if not isinstance(items, list):
                    raise ValueError("Invalid FFZ emotes")
                for item in items:
                    name = item["name"]
                    if not isinstance(name, str) or not name:
                        raise ValueError("Invalid emote name")
                    urls = item.get("animated") or item["urls"]
                    url = urls.get("2") or urls.get("4") or urls.get("1")
                    entries[name] = _emote_meta("ffz", item["id"], url, scope)
            return entries
        rid = urllib.parse.quote(str(room_id or ""), safe="")
        return self._fetch_scopes("ffz", room_id, {
            "global": "https://api.frankerfacez.com/v1/set/global",
            "channel": f"https://api.frankerfacez.com/v1/room/id/{rid}"}, parse)

    def fetch_badges(self, room_id: Optional[str] = None) -> Dict[str, dict]:
        use_helix = bool(self.twitch_client_id and self.twitch_token)
        token = self.twitch_token.removeprefix("oauth:").removeprefix("Bearer ")
        headers = {"Client-Id": self.twitch_client_id, "Authorization": f"Bearer {token}"} if use_helix else None
        rid = urllib.parse.quote(str(room_id or ""), safe="")
        if use_helix:
            endpoints = {"global": "https://api.twitch.tv/helix/chat/badges/global",
                         "channel": f"https://api.twitch.tv/helix/chat/badges?broadcaster_id={rid}"}
        else:
            logger.warning("No current Twitch credentials; legacy badge endpoints are best-effort and may fail")
            endpoints = {"global": "https://badges.twitch.tv/v1/badges/global/display",
                         "channel": f"https://badges.twitch.tv/v1/badges/channels/{rid}/display"}

        def parse(payload, scope):
            entries = {}
            if not isinstance(payload, dict):
                raise ValueError("Invalid badge response")
            if use_helix:
                items = payload.get("data")
                if not isinstance(items, list):
                    raise ValueError("Invalid Helix badges")
            else:
                sets = payload.get("badge_sets")
                if not isinstance(sets, dict):
                    raise ValueError("Invalid legacy badges")
                items = []
                for set_id, value in sets.items():
                    versions = value["versions"]
                    if not isinstance(versions, dict):
                        raise ValueError("Invalid badge versions")
                    items.append({"set_id": set_id,
                                  "versions": [dict(version, id=key) for key, version in versions.items()]})
            for item in items:
                set_id = _identifier(item["set_id"])
                versions = item["versions"]
                if not isinstance(versions, list):
                    raise ValueError("Invalid badge versions")
                for version in versions:
                    vid = _identifier(version["id"])
                    url = _remote_url(version.get("image_url_2x") or version.get("image_url_4x") or version.get("image_url_1x"))
                    identity = f"{room_id if scope == 'channel' else 'global'}/{set_id}/{vid}"
                    entries[f"{set_id}/{vid}"] = {
                        "provider": "twitch", "source": "twitch", "set_id": set_id, "version": vid,
                        "url": url, "file": _cache_filename("badges", identity, url),
                        "channel": scope == "channel", "scope": scope, "cache_version": CACHE_VERSION}
            return entries
        return self._fetch_scopes("badges", room_id, endpoints, parse, headers)

    def ensure_twitch_emote(self, emote_id: str) -> Optional[Path]:
        try:
            eid = _identifier(emote_id)
            if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", eid):
                raise ValueError("Unsafe Twitch identifier")
            dest = self._inside_cache(self.twitch_cache / f"{eid}.png")
            url = f"https://static-cdn.jtvnw.net/emoticons/v2/{eid}/default/dark/2.0"
            checked = dest.with_suffix(".animation-check.json")
            valid_check = False
            try: valid_check = json.loads(checked.read_text()).get("url") == url
            except Exception: pass
            success = download_image_to_png(url, dest, target_height=56, force=not valid_check)
            return dest if success or _valid_cached_png(dest) else None
        except Exception:
            logger.warning("Twitch image rejected; check the emote ID and cache path")
            return None

    def ensure_word_emote(self, word: str, word_meta: dict) -> Optional[Path]:
        try:
            provider = word_meta.get("source") or word_meta.get("provider")
            if provider not in PROVIDERS:
                raise ValueError("Unknown emote provider")
            dest = self._destination(word_meta, provider)
            animation_file = None
            for extension in ("webp", "gif"):
                candidate = dest.with_suffix("." + extension)
                if candidate.is_file() and candidate.stat().st_size <= MAX_IMAGE_BYTES:
                    try:
                        if _animation_info(candidate.read_bytes()) == extension:
                            animation_file = candidate
                            break
                    except Exception:
                        pass
            checked = dest.with_suffix(".animation-check.json")
            needs_check = not checked.is_file()
            if checked.is_file():
                try: needs_check = json.loads(checked.read_text()).get("url") != word_meta.get("animation_url", word_meta.get("url"))
                except Exception: needs_check = True
            url = word_meta.get("animation_url") or word_meta.get("url")
            if animation_file is None and needs_check:
                success = download_image_to_png(url, dest, target_height=56, force=True)
            else:
                success = download_image_to_png(url, dest, target_height=56)
            if not success: return dest if _valid_cached_png(dest) else None
            for extension in ("webp", "gif"):
                candidate = dest.with_suffix("." + extension)
                if candidate.is_file():
                    word_meta.update(animation_file=candidate.relative_to(self.cache_dir).as_posix(), animation_format=extension, animated=True)
                    break
            else:
                for key in ANIMATION_KEYS:
                    word_meta.pop(key, None)
            return dest
        except Exception:
            logger.warning("Third-party image rejected; check provider metadata and relative cache path")
            return None

    def ensure_badge(self, badge_key: str, badge_meta: dict) -> Optional[Path]:
        try:
            dest = self._destination(badge_meta, "badges")
            return dest if download_image_to_png(badge_meta.get("url"), dest, target_height=36) else None
        except Exception:
            logger.warning("Badge image rejected; check metadata and relative cache path")
            return None

    def _validate_entries(self, entries, provider):
        if not isinstance(entries, dict):
            raise ValueError("Invalid snapshot mapping")
        normalized = {}
        for key, value in entries.items():
            if not isinstance(key, str) or not key or not isinstance(value, dict):
                raise ValueError("Invalid snapshot entry")
            meta = dict(value)
            if provider == "badges":
                meta["set_id"] = _identifier(meta.get("set_id", key.split("/", 1)[0]))
                meta["version"] = _identifier(meta.get("version", key.split("/", 1)[-1]))
                meta["source"] = meta["provider"] = "twitch"
                prefix = "badges"
            else:
                prefix = meta.get("source") or meta.get("provider") or provider
                if prefix not in PROVIDERS or (provider in PROVIDERS and prefix != provider):
                    raise ValueError("Invalid snapshot provider")
                meta["source"] = meta["provider"] = prefix
                meta["id"] = _identifier(meta.get("id"))
            path = meta.get("file") or meta.get("local_path")
            _relative_png(path, prefix)
            self._inside_cache(self.cache_dir.joinpath(*PurePosixPath(path).parts))
            meta["file"] = path
            meta.pop("local_path", None)
            if meta.get("url"):
                meta["url"] = _remote_url(meta["url"])
            if meta.get("animation_file"):
                animation = meta["animation_file"]
                extension = PurePosixPath(animation).suffix
                if extension not in {".webp", ".gif"}: raise ValueError("Unsupported animation file")
                _relative_png(str(PurePosixPath(animation).with_suffix(".png")), prefix)
                self._inside_cache(self.cache_dir / animation)
            normalized[key] = meta
        return normalized

    def _load_snapshot(self, output_file):
        try:
            with open(output_file, encoding="utf-8") as source:
                # Bound untrusted manifests just like API responses.
                text = source.read(MAX_JSON_BYTES + 1)
            if len(text.encode("utf-8")) > MAX_JSON_BYTES:
                raise ValueError("Manifest too large")
            snapshot = json.loads(text)
            if not isinstance(snapshot, dict):
                raise ValueError("Invalid snapshot")
            snapshot["words"] = self._validate_entries(snapshot.get("words", {}), "words")
            snapshot["badges"] = self._validate_entries(snapshot.get("badges", {}), "badges")
            if "provider_snapshots" in snapshot:
                scopes = snapshot["provider_snapshots"]
                if not isinstance(scopes, dict):
                    raise ValueError("Invalid provider snapshots")
                for provider, values in scopes.items():
                    if provider not in (*PROVIDERS, "badges") or not isinstance(values, dict):
                        raise ValueError("Invalid provider scope")
                    for scope, entries in values.items():
                        if scope not in ("global", "channel"):
                            raise ValueError("Invalid provider scope")
                        values[scope] = self._validate_entries(entries, provider)
            return snapshot
        except Exception:
            raise ValueError("Snapshot is unreadable or unsafe; restore a valid manifest (existing file was not overwritten)") from None

    def _old_scopes(self, snapshot):
        if snapshot.get("provider_snapshots") is not None:
            return snapshot["provider_snapshots"]
        result = {provider: {"global": {}, "channel": {}} for provider in (*PROVIDERS, "badges")}
        for name, meta in snapshot.get("words", {}).items():
            scope = "channel" if meta.get("scope") == "channel" else "global"
            result[meta["source"]][scope][name] = meta
        for key, meta in snapshot.get("badges", {}).items():
            scope = "channel" if meta.get("channel") or meta.get("scope") == "channel" else "global"
            result["badges"][scope][key] = meta
        return result

    def _write_snapshot(self, output_file, snapshot):
        output_file.parent.mkdir(parents=True, exist_ok=True)
        temporary = None
        try:
            # Serialize before opening the temporary file, so partial/bad data cannot clobber old metadata.
            encoded = json.dumps(snapshot, ensure_ascii=False, indent=2).encode("utf-8")
            if len(encoded) > MAX_JSON_BYTES:
                raise ValueError("Snapshot exceeds resource limit")
            fd, filename = tempfile.mkstemp(prefix=output_file.name + ".", suffix=".tmp", dir=output_file.parent)
            temporary = Path(filename)
            with os.fdopen(fd, "wb") as target:
                target.write(encoded)
                target.flush()
                os.fsync(target.fileno())
            os.replace(temporary, output_file)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)

    def snapshot_session(self, output_file: Path, channel: str, room_id: Optional[str] = None,
                         download_all_images: bool = False, refresh: bool = False, capture=None) -> dict:
        """Reuse an existing snapshot by default; explicitly refresh current metadata.

        capture is optional JSON-serializable provenance (e.g. source/date/import
        kind). It does not imply that current API responses represent that date.
        """
        output_file = Path(output_file)
        existing = self._load_snapshot(output_file) if output_file.exists() else None
        if existing is not None and not refresh:
            self.provider_status = existing.get("provider_status", {})
            # Root relocation changes provenance only, never historical membership.
            if existing.get("cache_root") != str(self.cache_dir) or existing.get("emote_cache_dir") != str(self.cache_dir):
                existing["cache_root"] = existing["emote_cache_dir"] = str(self.cache_dir)
                self._write_snapshot(output_file, existing)
            logger.info("Using existing session emote snapshot; no metadata refresh requested")
            if download_all_images:
                self._cache_items(set(), set(existing["words"]), set(existing["badges"]), existing)
            return existing
        logger.warning("Capturing current provider metadata, NOT historical emote membership%s",
                       " (explicit refresh)" if refresh else "")
        self.provider_status = {}
        self._fetched_scopes = {}
        self.fetch_7tv_emotes(room_id)
        self.fetch_bttv_emotes(room_id)
        self.fetch_ffz_emotes(room_id)
        self.fetch_badges(room_id)
        merged = self._old_scopes(existing or {})
        # Old manifests did not retain scope membership. If any scope failed,
        # preserve ambiguous legacy entries in that failed scope, rather than
        # erasing a channel entry on a successful global response (or vice versa).
        if existing and "provider_snapshots" not in existing:
            for provider in (*PROVIDERS, "badges"):
                failed_scope = next((scope for scope in ("global", "channel")
                                     if self.provider_status[provider][scope]["status"] != "ok"), None)
                if failed_scope is not None:
                    old_entries = existing["badges"] if provider == "badges" else {
                        key: meta for key, meta in existing["words"].items() if meta["source"] == provider}
                    merged[provider][failed_scope].update(old_entries)
        captured_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
        for provider in (*PROVIDERS, "badges"):
            merged.setdefault(provider, {"global": {}, "channel": {}})
            for scope in ("global", "channel"):
                status = self.provider_status[provider][scope]
                if status["status"] == "ok":
                    merged[provider][scope] = self._fetched_scopes[provider][scope]
                    status["captured_at"] = captured_at
                else:
                    status["preserved_count"] = len(merged[provider].get(scope, {}))
                    previous = (existing or {}).get("provider_status", {}).get(provider, {}).get(scope, {})
                    status["retained_captured_at"] = previous.get("captured_at") or previous.get("retained_captured_at") or (existing or {}).get("captured_at")
        words = {}
        for provider in reversed(PROVIDERS):
            for scope in ("global", "channel"):
                words.update(merged[provider].get(scope, {}))
        badges = dict(merged["badges"].get("global", {}))
        badges.update(merged["badges"].get("channel", {}))
        snapshot = dict(existing or {})
        snapshot.update({"channel": channel, "room_id": str(room_id) if room_id else None,
                         "words": words, "badges": badges, "provider_snapshots": merged,
                         "provider_status": self.provider_status, "schema_version": 2,
                         "cache_root": str(self.cache_dir), "emote_cache_dir": str(self.cache_dir),
                         "captured_at": captured_at,
                         "capture": capture if capture is not None else (existing or {}).get("capture"),
                         "metadata_source": "current-provider-apis",
                         "historical_membership": False})
        # Validate every merged entry before an atomic write.
        snapshot["words"] = self._validate_entries(words, "words")
        snapshot["badges"] = self._validate_entries(badges, "badges")
        self._write_snapshot(output_file, snapshot)
        failures = sum(s["status"] == "failed" for p in self.provider_status.values() for s in p.values())
        log = logger.warning if failures else logger.info
        log("Snapshot written: %d words, %d badges, %d failed provider scopes (previous entries retained)",
            len(words), len(badges), failures)
        if download_all_images:
            self._cache_items(set(), set(words), set(badges), snapshot)
        return snapshot

    def _cache_items(self, twitch_ids, words, badges, snapshot):
        summary = {"newly_cached": 0, "existing": 0, "failed": 0, "missing_metadata": 0,
                   "scan_errors": 0, "requested": 0, "failures": []}
        tasks = []
        for eid in sorted(twitch_ids):
            tasks.append(("twitch", eid, None))
        for name in sorted(words):
            tasks.append(("word", name, snapshot["words"].get(name)))
        for key in sorted(badges):
            tasks.append(("badge", key, snapshot["badges"].get(key)))
        seen = set()
        for kind, key, meta in tasks:
            try:
                if kind == "twitch":
                    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", key):
                        raise ValueError("Invalid native ID")
                    dest = self._inside_cache(self.twitch_cache / f"{key}.png")
                elif meta is None:
                    summary["missing_metadata"] += 1
                    summary["failed"] += 1
                    summary["requested"] += 1
                    summary["failures"].append({"kind": kind, "reason": "snapshot has no metadata for required image"})
                    continue
                else:
                    prefix = "badges" if kind == "badge" else (meta.get("source") or meta.get("provider"))
                    dest = self._destination(meta, prefix)
                if dest in seen:
                    continue
                seen.add(dest)
                summary["requested"] += 1
                existed = _valid_cached_png(dest)
                if kind == "twitch":
                    result = self.ensure_twitch_emote(key)
                elif kind == "word":
                    result = self.ensure_word_emote(key, meta)
                else:
                    result = self.ensure_badge(key, meta)
                if result:
                    summary["existing" if existed else "newly_cached"] += 1
                else:
                    summary["failed"] += 1
                    summary["failures"].append({"kind": kind, "reason": "image unavailable; check source, network, and cache permissions"})
            except Exception:
                summary["requested"] += 1
                summary["failed"] += 1
                summary["failures"].append({"kind": kind, "reason": "unsafe or invalid cache metadata rejected"})
        log = logger.warning if summary["failed"] else logger.info
        log("Image cache: %d newly cached, %d existing, %d failed (%d missing metadata), root %s",
            summary["newly_cached"], summary["existing"], summary["failed"], summary["missing_metadata"], self.cache_dir)
        return summary

    def cache_used_emotes_from_chat(self, chat_file: Path, snapshot_file: Path):
        """Scan tagged/untagged IRC and normalized JSON; return download counts.

        Native spans use inclusive Unicode-codepoint offsets from the IRC emotes
        tag. Exact ASCII-whitespace-delimited names outside those spans only are
        considered for third-party lookup, including symbolic names such as :).
        """
        from chat_records import parse_irc_record, tokenize_plain_text

        empty = {"newly_cached": 0, "existing": 0, "failed": 0, "missing_metadata": 0,
                 "scan_errors": 0, "requested": 0, "failures": []}
        try:
            snapshot = self._load_snapshot(Path(snapshot_file))
            twitch_ids, words, badges = set(), set(), set()
            errors = 0
            with open(chat_file, encoding="utf-8") as source:
                for line in source:
                    if not line.strip():
                        continue
                    try:
                        try:
                            record = json.loads(line)
                        except json.JSONDecodeError:
                            record = line.rstrip("\r\n")
                        original = record if isinstance(record, dict) else {}
                        if original.get("normalized") is True:
                            parsed = dict(original)
                        else:
                            parsed = parse_irc_record(record)
                        if not isinstance(parsed, dict):
                            raise ValueError("Invalid chat record")
                        tags = {} if original.get("normalized") is True else (parsed.get("tags") or {})
                        text = parsed.get("text", parsed.get("trailing", original.get("text", ""))) or ""
                        emotes = tags.get("emotes", parsed.get("emotes", original.get("emotes", ""))) or ""
                        badge_tag = tags.get("badges", parsed.get("badges", original.get("badges", ""))) or ""
                        if not all(isinstance(value, str) for value in (text, emotes, badge_tag)):
                            raise ValueError("Invalid chat tags")
                        for badge in badge_tag.split(","):
                            if badge:
                                badges.add(badge)
                        spans = []
                        malformed_spans = False
                        for group in emotes.split("/"):
                            if not group:
                                continue
                            eid, separator, ranges = group.partition(":")
                            if not separator or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", eid):
                                malformed_spans = True
                                continue
                            twitch_ids.add(eid)
                            for interval in ranges.split(","):
                                match = re.fullmatch(r"(\d+)-(\d+)", interval)
                                if match:
                                    start, end = map(int, match.groups())
                                    if 0 <= start <= end < len(text):
                                        spans.append((start, end + 1))
                                        continue
                                malformed_spans = True
                        if malformed_spans:
                            errors += 1
                            # Unknown native coverage must not trigger wrong third-party downloads.
                            continue
                        cursor = 0
                        for start, end in sorted(spans):
                            if start < cursor:
                                continue
                            for token in tokenize_plain_text(text[cursor:start]):
                                if token and token[0] not in ASCII_WHITESPACE and token in snapshot["words"]:
                                    words.add(token)
                            cursor = end
                        for token in tokenize_plain_text(text[cursor:]):
                            if token and token[0] not in ASCII_WHITESPACE and token in snapshot["words"]:
                                words.add(token)
                    except Exception:
                        errors += 1
            summary = self._cache_items(twitch_ids, words, badges, snapshot)
            summary["scan_errors"] = errors
            for word in words:
                meta = snapshot["words"].get(word)
                if meta:
                    for provider_scopes in snapshot.get("provider_snapshots", {}).values():
                        for entries in provider_scopes.values():
                            if word in entries and entries[word].get("file") == meta.get("file"):
                                for key in ANIMATION_KEYS:
                                    if key in meta:
                                        entries[word][key] = meta[key]
                                    else:
                                        entries[word].pop(key, None)
            self._write_snapshot(Path(snapshot_file), snapshot)
            if errors:
                logger.warning("Chat scan skipped %d malformed records/native span tags; cache preparation is incomplete", errors)
            return summary
        except Exception:
            logger.warning("Emote cache preparation failed; check the chat file, shared parser, and snapshot paths/metadata")
            empty["scan_errors"] = 1
            empty["failed"] = 1
            empty["failures"].append({"kind": "scan", "reason": "chat/snapshot could not be safely read"})
            return empty
