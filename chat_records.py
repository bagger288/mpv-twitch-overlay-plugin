"""Shared, dependency-free normalization for archive and live IRC chat records.

Tokens are exact runs separated only by ASCII whitespace, matching Rust's
split_ascii_whitespace semantics without losing the intervening whitespace.
"""

import datetime
import re
from typing import List, Union
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

ASCII_WHITESPACE = " \t\n\r\f"  # Rust char::is_ascii_whitespace excludes vertical tab.
_TAG_ESCAPES = {"s": " ", ":": ";", "r": "\r", "n": "\n", "\\": "\\"}
_MAX_TIMESTAMP_MS = 253402300799999


def tokenize_plain_text(text: str) -> List[str]:
    """Return exact whitespace/word runs, preserving case and punctuation."""
    if not isinstance(text, str):
        raise ValueError("Chat text must be a string")
    return re.findall(r"[ \t\n\r\f]+|[^ \t\n\r\f]+", text)


def resolve_timezone(timezone: Union[str, datetime.tzinfo] = "UTC") -> datetime.tzinfo:
    if isinstance(timezone, datetime.tzinfo):
        return timezone
    if not isinstance(timezone, str):
        raise ValueError("Timezone must be UTC, an IANA name, or a UTC offset")
    if timezone.upper() in {"UTC", "Z"}:
        return datetime.timezone.utc
    match = re.fullmatch(r"(?:UTC)?([+-])(\d{2}):(\d{2})", timezone, re.IGNORECASE)
    if match:
        hours, minutes = int(match[2]), int(match[3])
        if hours > 23 or minutes > 59:
            raise ValueError("Invalid timezone offset")
        offset = datetime.timedelta(hours=hours, minutes=minutes)
        return datetime.timezone(offset if match[1] == "+" else -offset)
    try:
        return ZoneInfo(timezone)
    except (ZoneInfoNotFoundError, ValueError) as exc:
        raise ValueError("Unknown timezone; use UTC or an explicit offset if tzdata is unavailable") from exc


def timestamp_ms(value, *, timezone: Union[str, datetime.tzinfo] = "UTC") -> int:
    """Parse an ISO datetime (naive means the specified zone) or epoch milliseconds."""
    if isinstance(value, bool):
        raise ValueError("Invalid timestamp")
    if isinstance(value, int) or (isinstance(value, str) and re.fullmatch(r"\d+", value)):
        result = int(value)
    else:
        if isinstance(value, datetime.datetime):
            dt = value
        elif isinstance(value, str) and re.match(r"^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}", value):
            try:
                dt = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
            except ValueError as exc:
                raise ValueError("Invalid ISO timestamp") from exc
        else:
            raise ValueError("Missing or invalid timestamp")
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=resolve_timezone(timezone))
        try:
            epoch = dt.astimezone(datetime.timezone.utc) - datetime.datetime(1970, 1, 1, tzinfo=datetime.timezone.utc)
            result = (epoch.days * 86400 + epoch.seconds) * 1000 + epoch.microseconds // 1000
        except (OverflowError, ValueError) as exc:
            raise ValueError("Timestamp out of range") from exc
    if not 0 < result <= _MAX_TIMESTAMP_MS:
        raise ValueError("Timestamp must be positive epoch milliseconds in the supported date range")
    return result


def _decode_tag(value: str) -> str:
    # IRCv3 unknown escapes drop the slash; an incomplete final slash is ignored.
    return re.sub(r"\\(.)|\\$", lambda m: _TAG_ESCAPES.get(m[1], m[1]) if m[1] else "", value)


def parse_irc_record(record: Union[str, dict]) -> dict:
    """Parse IRC tags/prefix/command/trailing text, retaining an archive envelope.

    Accept a raw line or a dictionary with ``raw`` (also ``raw_message``/``irc``).
    Timestamp validation is deliberately left to normalize_record so scanners
    can still discover emotes in undated raw IRC fixtures.
    """
    if isinstance(record, str):
        result = {"raw": record}
    elif isinstance(record, dict):
        result = dict(record)
    else:
        raise ValueError("Chat record must be an object or IRC string")
    if result.get("normalized") is True:
        result["tags"] = {}
        return result
    tags = result.get("tags") or {}
    if not isinstance(tags, dict):
        raise ValueError("IRC tags must be an object")
    tags = dict(tags)
    raw = result.get("raw") or result.get("raw_message") or result.get("irc")
    if raw is None:
        result["tags"] = tags
        return result
    if not isinstance(raw, str):
        raise ValueError("Raw IRC must be a string")
    rest = raw.rstrip("\r\n")
    if rest.startswith("@"):
        tag_block, sep, rest = rest.partition(" ")
        if not sep:
            raise ValueError("IRC tags have no command")
        for part in tag_block[1:].split(";"):
            key, _, value = part.partition("=")
            if key:
                tags[key] = _decode_tag(value)
        rest = rest.lstrip(" ")
    prefix = ""
    if rest.startswith(":"):
        prefix, sep, rest = rest[1:].partition(" ")
        if not sep:
            raise ValueError("IRC prefix has no command")
        rest = rest.lstrip(" ")
    # The trailing parameter begins AFTER the prefix and command, never at the
    # first ' :' in a tagged line (which is the server/user prefix).
    command_params, marker, body = rest.partition(" :")
    params = command_params.split()
    if not params:
        raise ValueError("IRC command is missing")
    command = params[0].upper()
    if command not in {"PRIVMSG", "USERNOTICE", "CLEARCHAT", "CLEARMSG", "NOTICE"}:
        raise ValueError("Unsupported IRC chat command")
    result.update(tags=tags, command=command, prefix=prefix, text=body if marker else "")
    if prefix and "!" in prefix:
        result["login"] = prefix.split("!", 1)[0]
    if len(params) > 1:
        result["channel"] = params[1].lstrip("#")
    return result


def normalize_record(record: Union[str, dict], *, timezone: Union[str, datetime.tzinfo] = "UTC") -> dict:
    """Return the overlay ``ts/user/display/text`` schema or raise ValueError.

    A valid tmi-sent-ts wins; malformed tags fall back to ISO/envelope timestamps.
    Unknown envelope fields and raw input are retained for later reprocessing.
    """
    result = parse_irc_record(record)
    tags = result["tags"]
    ts = None
    tag_ts = tags.get("tmi-sent-ts")
    if isinstance(tag_ts, (str, int)) and not isinstance(tag_ts, bool):
        try:
            if not re.fullmatch(r"\d+", str(tag_ts)):
                raise ValueError("Invalid tmi timestamp")
            ts = timestamp_ms(tag_ts)
        except ValueError:
            pass
    if ts is None:
        for key in ("timestamp", "ts", "timestamp_ms", "time"):
            if result.get(key) is not None:
                try:
                    ts = timestamp_ms(result[key], timezone=timezone)
                    break
                except ValueError:
                    continue
    if ts is None:
        raise ValueError("Chat record has no valid tmi-sent-ts or ISO timestamp")
    text = result.get("text") or ""
    if not isinstance(text, str):
        raise ValueError("Chat text must be a string")
    display = result.get("displayName") or tags.get("display-name") or result.get("display") or ""
    user = result.get("user")
    if isinstance(user, dict):
        display = display or user.get("display_name") or user.get("displayName") or ""
        user = user.get("login") or user.get("name")
    login = result.get("login") or tags.get("login") or user or str(display).lower() or "user"
    msg_id = tags.get("msg-id") or result.get("msg_id") or ""
    command = result.get("command")
    kind = result.get("kind") or "message"
    if tags.get("target-user-id") or command == "CLEARCHAT":
        kind = "ban"
    elif msg_id or command == "USERNOTICE":
        kind = "event"
    elif command == "CLEARMSG":
        kind = "delete"
    # Bestlogs concatenates system-msg with milestone body; raw IRC does not.
    if msg_id == "viewermilestone" and not (result.get("raw") or result.get("raw_message") or result.get("irc")):
        system_msg = tags.get("system-msg") or ""
        if system_msg and text.startswith(system_msg):
            text = text[len(system_msg):].lstrip(" ")
    def string_field(name, tag_name=None):
        value = tags.get(tag_name or name)
        if value is None:
            value = result.get(name)
        return "" if value is None else str(value)
    result.update(
        normalized=True, ts=ts, kind=kind, msg_id=str(msg_id), user=str(login),
        display=str(display or login), text=text, color=tags.get("color", result.get("color")),
        badges=string_field("badges"), badge_info=string_field("badge_info", "badge-info"),
        emotes=string_field("emotes"), id=string_field("id"), room_id=string_field("room_id", "room-id"),
    )
    for key in ("id", "room-id", "user-id", "target-user-id", "target-msg-id"):
        if key in tags and tags[key] is not None:
            tags[key] = str(tags[key])
    for key in ("user_id", "target_user_id", "target_msg_id"):
        if key in result and result[key] is not None:
            result[key] = str(result[key])
    return result
