--[[
  Twitch Chat Companion Input Interceptor for mpv
  Provides 100% exclusive keyboard input capture with full Cyrillic (Russian)
  and Latin support, blocking native mpv hotkeys (space, seek, quit, etc.)
  while the Twitch Chat search view is active.
--]]

local mp = require 'mp'

local search_active = false
local bound_keys = {}

local SYM_LIST = {
    { key = "SPACE", char = " " },
    { key = "-", char = "-" },
    { key = "_", char = "_" },
    { key = ".", char = "." },
    { key = ",", char = "," },
    { key = "!", char = "!" },
    { key = "?", char = "?" },
    { key = "@", char = "@" },
    { key = "#", char = "#" },
    { key = "/", char = "/" },
    { key = ":", char = ":" },
    { key = ";", char = ";" },
    { key = "'", char = "'" },
    { key = '"', char = '"' },
    { key = "(", char = "(" },
    { key = ")", char = ")" },
    { key = "[", char = "[" },
    { key = "]", char = "]" },
    { key = "{", char = "{" },
    { key = "}", char = "}" },
    { key = "+", char = "+" },
    { key = "=", char = "=" },
    { key = "*", char = "*" },
    { key = "\\", char = "\\" },
    { key = "|", char = "|" },
    { key = "<", char = "<" },
    { key = ">", char = ">" },
}

local LATIN_CHARS = "abcdefghijklmnopqrstuvwxyz"

local CYR_LOWER = {
    "а", "б", "в", "г", "д", "е", "ё", "ж", "з", "и", "й", "к", "л", "м", "н",
    "о", "п", "р", "с", "т", "у", "ф", "х", "ц", "ч", "ш", "щ", "ъ", "ы", "ь",
    "э", "ю", "я"
}
local CYR_UPPER = {
    "А", "Б", "В", "Г", "Д", "Е", "Ё", "Ж", "З", "И", "Й", "К", "Л", "М", "Н",
    "О", "П", "Р", "С", "Т", "У", "Ф", "Х", "Ц", "Ч", "Ш", "Щ", "Ъ", "Ы", "Ь",
    "Э", "Ю", "Я"
}

-- Mpv hotkeys to suppress while typing so they never leak to player actions
local BLOCKED_PLAYER_KEYS = {
    "LEFT", "RIGHT", "UP", "DOWN",
    "Shift+LEFT", "Shift+RIGHT", "Shift+UP", "Shift+DOWN",
    "Ctrl+LEFT", "Ctrl+RIGHT",
    "PGUP", "PGDWN", "HOME", "END",
    "Tab", "Shift+Tab"
}

local function bind_key(key, name, callback)
    table.insert(bound_keys, name)
    mp.add_forced_key_binding(key, name, callback, { repeatable = true })
end

local function enable_search_capture()
    if search_active then return end
    search_active = true
    bound_keys = {}

    -- 1. Printable symbols
    for i, item in ipairs(SYM_LIST) do
        bind_key(item.key, "tci_sym_" .. i, function()
            mp.commandv("script-message", "twitch_chat_key", item.char)
        end)
    end

    -- 2. Digits 0-9
    for d = 0, 9 do
        local s = tostring(d)
        bind_key(s, "tci_dig_" .. s, function()
            mp.commandv("script-message", "twitch_chat_key", s)
        end)
    end

    -- 3. Latin letters a-z, A-Z, and Shift combinations
    for i = 1, #LATIN_CHARS do
        local c = LATIN_CHARS:sub(i, i)
        local u = c:upper()
        bind_key(c, "tci_lat_low_" .. c, function()
            mp.commandv("script-message", "twitch_chat_key", c)
        end)
        bind_key(u, "tci_lat_up_" .. u, function()
            mp.commandv("script-message", "twitch_chat_key", u)
        end)
        bind_key("Shift+" .. c, "tci_lat_sh_" .. c, function()
            mp.commandv("script-message", "twitch_chat_key", u)
        end)
    end

    -- 4. Cyrillic letters (Russian lowercase & uppercase)
    for i, c in ipairs(CYR_LOWER) do
        local c_up = CYR_UPPER[i] or c
        bind_key(c, "tci_cyr_low_" .. i, function()
            mp.commandv("script-message", "twitch_chat_key", c)
        end)
        bind_key(c_up, "tci_cyr_up_" .. i, function()
            mp.commandv("script-message", "twitch_chat_key", c_up)
        end)
        bind_key("Shift+" .. c, "tci_cyr_sh_" .. i, function()
            mp.commandv("script-message", "twitch_chat_key", c_up)
        end)
    end

    -- 5. Backspace
    bind_key("BS", "tci_bs", function()
        mp.commandv("script-message", "twitch_chat_bs")
    end)

    -- 6. Escape (closes search)
    bind_key("ESC", "tci_esc", function()
        mp.commandv("script-message", "twitch_chat_esc")
    end)

    -- 7. Enter (jumps to first result)
    bind_key("ENTER", "tci_enter", function()
        mp.commandv("script-message", "twitch_chat_enter")
    end)

    -- 8. Suppress navigation and player keys while typing
    for i, k in ipairs(BLOCKED_PLAYER_KEYS) do
        bind_key(k, "tci_blk_" .. i, function()
            -- Suppressed keypress, prevents mpv from seeking or altering playback
        end)
    end

    mp.msg.info("[twitch_chat_input] Search keyboard intercept enabled (exclusive)")
end

local function disable_search_capture()
    if not search_active then return end
    search_active = false

    for _, name in ipairs(bound_keys) do
        mp.remove_key_binding(name)
    end
    bound_keys = {}

    mp.msg.info("[twitch_chat_input] Search keyboard intercept disabled")
end

-- Listen for activation messages from Rust plugin or Lua UI
mp.register_script_message("twitch_search_active", function(state_str)
    if state_str == "1" or state_str == "true" or state_str == "yes" then
        enable_search_capture()
    else
        disable_search_capture()
    end
end)

mp.register_script_message("twitch_chat_search_active", function(state_str)
    if state_str == "1" or state_str == "true" or state_str == "yes" then
        enable_search_capture()
    else
        disable_search_capture()
    end
end)

mp.msg.info("[twitch_chat_input] Companion input script loaded.")
