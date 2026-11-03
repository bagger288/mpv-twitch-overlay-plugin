"""Twitch OAuth refresh requests without exposing credentials in errors."""

import json
from urllib import error, parse, request

TOKEN_URL = "https://id.twitch.tv/oauth2/token"


class TokenRefreshError(Exception):
    pass


class NoRedirect(request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def refresh_twitch_token(client_id, client_secret, refresh_token, open_request=None):
    fields = {
        "grant_type": "refresh_token",
        "refresh_token": refresh_token.strip(),
        "client_id": client_id.strip(),
        "client_secret": client_secret.strip(),
    }
    if not all(fields[key] for key in ("refresh_token", "client_id", "client_secret")):
        raise TokenRefreshError("Не заполнены Twitch Client ID, Client Secret или Refresh Token")
    req = request.Request(
        TOKEN_URL,
        data=parse.urlencode(fields).encode("utf-8"),
        headers={"Content-Type": "application/x-www-form-urlencoded", "Accept": "application/json"},
        method="POST",
    )
    if open_request is None:
        open_request = request.build_opener(NoRedirect()).open
    try:
        with open_request(req, timeout=15) as response:
            raw = response.read(65537)
        if len(raw) > 65536:
            raise TokenRefreshError("Ответ Twitch превышает допустимый размер")
        payload = json.loads(raw)
    except error.HTTPError as exc:
        explanations = {400: "некорректный или недействительный refresh token", 401: "неверные реквизиты приложения", 429: "слишком много запросов"}
        detail = explanations.get(exc.code, "запрос отклонён сервером")
        raise TokenRefreshError(f"Twitch OAuth: HTTP {exc.code} — {detail}") from None
    except (error.URLError, TimeoutError, OSError):
        raise TokenRefreshError("Не удалось подключиться к Twitch OAuth: ошибка сети или таймаут") from None
    except (ValueError, UnicodeError):
        raise TokenRefreshError("Twitch OAuth вернул некорректный JSON") from None
    if not isinstance(payload, dict):
        raise TokenRefreshError("Twitch OAuth вернул неожиданный формат ответа")
    access = payload.get("access_token")
    refresh = payload.get("refresh_token")
    if not isinstance(access, str) or not access or not isinstance(refresh, str) or not refresh:
        raise TokenRefreshError("Ответ Twitch не содержит новые access_token и refresh_token")
    expires = payload.get("expires_in")
    if isinstance(expires, bool) or not isinstance(expires, int) or expires <= 0:
        raise TokenRefreshError("Ответ Twitch не содержит корректный expires_in")
    return {"access_token": access, "refresh_token": refresh, "expires_in": expires}
