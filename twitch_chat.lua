--[[
  Twitch Chat Overlay & Control Panel for mpv
  Supports:
  - Mode 1: Pure Lua text chat with ASS styles, colored nicks, auto word-wrap.
  - Mode 2: Launches & communicates with Python renderer (renderer.py) for rich emotes and badges.
  - Graphical Mouse-Only Controls (Zero hotkeys):
    - Offset buttons (-10s, -1s, -0.1s, +0.1s, +1s, +10s) and click-to-reset.
    - Font size controls [A-] and [A+] (scales font, badges, and inline emotes!).
    - Chat dimensions controls: Width [W-] [W+], Height [H-] [H+].
    - Position presets: [Top-Left] [Bottom-Left] [Top-Right] [Bottom-Right] and nudge buttons [<-] [->] [^] [v].
    - Font family switcher: [Шрифт: Segoe UI / Arial / Tahoma / Verdana / DejaVu Sans / Consolas].
    - Chat and Emotes toggles.
  - Real-time synchronization with renderer.py via mpv IPC user-data/twitch_chat_config.
--]]

local mp = require 'mp'
local utils = require 'mp.utils'

local FONT_LIST = {
    "Segoe UI",
    "Arial",
    "Tahoma",
    "Verdana",
    "DejaVu Sans",
    "Consolas"
}

-- Default Configuration & State
local opts = {
    session_dir = "",
    chat_enabled = true,
    emotes_enabled = true,
    python_path = "python",
    chat_width = 420,
    chat_height = 600,
    chat_font_size = 17,
    chat_font = "Segoe UI",
    chat_bg_opacity = 0.75,
    chat_x = 24,
    chat_y = 60,
    ipc_socket = ""
}

-- Utility: Determine if file exists
local function file_exists(name)
    local f = io.open(name, "r")
    if f ~= nil then
        io.close(f)
        return true
    else
        return false
    end
end

-- Normalize path separators
local function normalize_path(p)
    return p:gsub("\\", "/")
end

-- Extract directory from full filepath
local function get_dir(filepath)
    local norm = normalize_path(filepath)
    return norm:match("^(.*)/[^/]*$") or "."
end

-- Extract filename from full filepath
local function get_filename(filepath)
    local norm = normalize_path(filepath)
    return norm:match("[^/]+$") or filepath
end

-- Load defaults from config.json if available
local function load_base_config()
    local candidates = {
        opts.session_dir .. "/../../config.json",
        opts.session_dir .. "/../config.json",
        "config.json"
    }
    for _, cp in ipairs(candidates) do
        local f = io.open(cp, "r")
        if f then
            local content = f:read("*a")
            f:close()
            local ok, cfg = pcall(utils.parse_json, content)
            if ok and cfg then
                if cfg.chat_width then opts.chat_width = tonumber(cfg.chat_width) or opts.chat_width end
                if cfg.chat_height then opts.chat_height = tonumber(cfg.chat_height) or opts.chat_height end
                if cfg.chat_font_size then opts.chat_font_size = tonumber(cfg.chat_font_size) or opts.chat_font_size end
                if cfg.chat_font_family then opts.chat_font = tostring(cfg.chat_font_family) end
                if cfg.chat_x then opts.chat_x = tonumber(cfg.chat_x) or opts.chat_x end
                if cfg.chat_y then opts.chat_y = tonumber(cfg.chat_y) or opts.chat_y end
                if cfg.chat_bg_opacity ~= nil then
                    local op = tonumber(cfg.chat_bg_opacity) or 0.75
                    if op > 1.0 then op = op / 100.0 end
                    opts.chat_bg_opacity = math.max(0.0, math.min(1.0, op))
                end
                if cfg.python_path then opts.python_path = tostring(cfg.python_path) end
                break
            end
        end
    end
end

-- Read script options if passed (--script-opts=twitch_session=...,twitch_emotes=...)
local script_opts = mp.get_opt("twitch_session")
if script_opts and script_opts ~= "" then
    opts.session_dir = script_opts
end

load_base_config()

local opt_emotes = mp.get_opt("twitch_emotes")
if opt_emotes ~= nil then
    opts.emotes_enabled = (opt_emotes == "yes" or opt_emotes == "true" or opt_emotes == "1")
end

local opt_chat = mp.get_opt("twitch_chat")
if opt_chat ~= nil then
    opts.chat_enabled = (opt_chat == "yes" or opt_chat == "true" or opt_chat == "1")
end

local opt_w = mp.get_opt("twitch_width")
if opt_w and tonumber(opt_w) then opts.chat_width = tonumber(opt_w) end

local opt_h = mp.get_opt("twitch_height")
if opt_h and tonumber(opt_h) then opts.chat_height = tonumber(opt_h) end

local opt_fs = mp.get_opt("twitch_font_size")
if opt_fs and tonumber(opt_fs) then opts.chat_font_size = tonumber(opt_fs) end

local opt_font = mp.get_opt("twitch_font")
if opt_font and opt_font ~= "" then opts.chat_font = opt_font end

local opt_op = mp.get_opt("twitch_bg_opacity") or mp.get_opt("twitch_opacity")
if opt_op and tonumber(opt_op) then
    local op = tonumber(opt_op)
    if op > 1.0 then op = op / 100.0 end
    opts.chat_bg_opacity = math.max(0.0, math.min(1.0, op))
end

local opt_x = mp.get_opt("twitch_x")
if opt_x and tonumber(opt_x) then opts.chat_x = tonumber(opt_x) end

local opt_y = mp.get_opt("twitch_y")
if opt_y and tonumber(opt_y) then opts.chat_y = tonumber(opt_y) end

local current_file = ""
local session_data = nil
local session_file_path = ""
local current_part = nil
local current_offset_ms = 0

-- Chat message entries: { ts = number, nick = string, color = string, text = string, kind = string, badges = string }
local chat_events = {}
local osd_overlay = mp.create_osd_overlay("ass-events")
local panel_overlay = mp.create_osd_overlay("ass-events")
local search_overlay = mp.create_osd_overlay("ass-events")
local toggle_overlay = mp.create_osd_overlay("ass-events")
local plugin_hidden = false

-- Interactive Chat Scroll & Sync State
local chat_scroll_offset = 0
local selected_sync_msg = nil
local visible_chat_messages = {}
local pause_banner_y_min = nil
local pause_banner_y_max = nil

-- Interactive Chat Search State
local search_visible = false
local search_query = ""
local search_results = {}
local search_page = 1
local search_page_size = 7
local search_keys_bound = false

-- Format seconds into [HH:]MM:SS
local function format_hms(sec)
    if not sec or sec < 0 then sec = 0 end
    local h = math.floor(sec / 3600)
    local m = math.floor((sec % 3600) / 60)
    local s = math.floor(sec % 60)
    if h > 0 then
        return string.format("%02d:%02d:%02d", h, m, s)
    else
        return string.format("%02d:%02d", m, s)
    end
end

-- Panel UI & Mouse state
local panel_visible = false
local panel_timer = nil
local mouse_x = 0
local mouse_y = 0

-- Mode 2 Python renderer process
local renderer_proc = nil

-- Get precise video-to-window scaling and letterbox margins
local function get_video_layout()
    local dims = mp.get_property_native("osd-dimensions")
    local d_w = mp.get_property_number("dwidth", 1920)
    local d_h = mp.get_property_number("dheight", 1080)
    if not d_w or d_w <= 0 then d_w = 1920 end
    if not d_h or d_h <= 0 then d_h = 1080 end

    if not dims or not dims.w or dims.w <= 0 or not dims.h or dims.h <= 0 then
        return {
            win_w = 1920,
            win_h = 1080,
            v_left = 0,
            v_top = 0,
            v_w = 1920,
            v_h = 1080,
            scale_x = 1.0,
            scale_y = 1.0,
            vid_w = d_w,
            vid_h = d_h,
        }
    end

    local ml = dims.ml or 0
    local mr = dims.mr or 0
    local mt = dims.mt or 0
    local mb = dims.mb or 0
    local v_w = math.max(1, dims.w - ml - mr)
    local v_h = math.max(1, dims.h - mt - mb)

    return {
        win_w = dims.w,
        win_h = dims.h,
        v_left = ml,
        v_top = mt,
        v_w = v_w,
        v_h = v_h,
        scale_x = v_w / d_w,
        scale_y = v_h / d_h,
        vid_w = d_w,
        vid_h = d_h,
    }
end

-- Ensure OSD overlays match window pixel coordinates 1:1
local function update_overlay_resolutions()
    local dims = mp.get_property_native("osd-dimensions")
    if dims and dims.w and dims.w > 0 and dims.h and dims.h > 0 then
        panel_overlay.res_x = dims.w
        panel_overlay.res_y = dims.h
        osd_overlay.res_x = dims.w
        osd_overlay.res_y = dims.h
        search_overlay.res_x = dims.w
        search_overlay.res_y = dims.h
        toggle_overlay.res_x = dims.w
        toggle_overlay.res_y = dims.h
    end
end

-- Forward declarations
local update_renderer_mode
local render_control_panel
local render_text_chat
local render_search_panel
local toggle_search
local sync_session_to_selected_message
local seek_to_message
local toggle_master_plugin
local render_toggle_button

-- Deterministic nick color generation
local function hash_color(nick)
    local hash = 0
    for i = 1, #nick do
        hash = (hash * 31 + nick:byte(i)) % 16777215
    end
    local r = bit.band(bit.rshift(hash, 16), 0xFF)
    local g = bit.band(bit.rshift(hash, 8), 0xFF)
    local b = bit.band(hash, 0xFF)
    if (r * 0.299 + g * 0.587 + b * 0.114) < 70 then
        r = math.min(255, r + 90)
        g = math.min(255, g + 90)
        b = math.min(255, b + 90)
    end
    return string.format("&H%02X%02X%02X&", b, g, r)
end

-- Convert HTML hex color (#RRGGBB) to ASS color (&HBBGGRR&)
local function hex_to_ass(hex)
    if not hex or #hex < 7 then return nil end
    local r = hex:sub(2, 3)
    local g = hex:sub(4, 5)
    local b = hex:sub(6, 7)
    return string.format("&H%s%s%s&", b, g, r)
end

-- Parse raw IRC line into structured chat event
local function parse_raw_irc(raw, recv_ts)
    if not raw or raw == "" then return nil end
    local tags_str, rest = raw:match("^@([^ ]+)%s+(.*)$")
    local tags = {}
    if tags_str then
        for pair in tags_str:gmatch("([^;]+)") do
            local k, v = pair:match("([^=]+)=(.*)")
            if k then
                tags[k] = v:gsub("\\:", ";"):gsub("\\s", " "):gsub("\\\\", "\\")
            else
                tags[pair] = "true"
            end
        end
    else
        rest = raw
    end

    local prefix, cmd, params = rest:match("^:([^ ]+)%s+([^ ]+)%s*(.*)$")
    if not cmd then return nil end

    local user = prefix and prefix:match("^([^!]+)") or "anonymous"
    local text = ""
    local trailing = params:match(":(.*)$")
    if trailing then
        text = trailing
    else
        text = params
    end

    local ts = tonumber(tags["tmi-sent-ts"]) or recv_ts or 0
    local display_nick = tags["display-name"]
    if not display_nick or display_nick == "" then
        display_nick = user
    end

    local color = hex_to_ass(tags["color"]) or hash_color(display_nick)

    return {
        ts = ts,
        nick = display_nick,
        color = color,
        text = text,
        kind = cmd,
        badges = tags["badges"] or ""
    }
end

-- Load chat events from chat.jsonl
local function load_chat_data(dir)
    chat_events = {}
    local files = utils.readdir(dir, "files")
    if not files then return end

    local chat_jsonl_name = nil
    for _, fn in ipairs(files) do
        if fn:match("%.chat%.jsonl$") then
            chat_jsonl_name = fn
            break
        end
    end

    if not chat_jsonl_name then return end
    local full_path = dir .. "/" .. chat_jsonl_name
    local f = io.open(full_path, "r")
    if not f then return end

    mp.msg.info("Loading Twitch chat from: " .. full_path)
    for line in f:lines() do
        local ok, data = pcall(utils.parse_json, line)
        if ok and data then
            if data.type == "GAP" then
                table.insert(chat_events, {
                    ts = data.recv or 0,
                    nick = "SYSTEM",
                    color = "&H00AAFF&",
                    text = "[GAP: Chat connection lost and restored]",
                    kind = "GAP",
                    badges = ""
                })
            elseif data.raw then
                local parsed = parse_raw_irc(data.raw, data.recv)
                if parsed and (parsed.kind == "PRIVMSG" or parsed.kind == "USERNOTICE") then
                    table.insert(chat_events, parsed)
                end
            elseif data.text then
                local color = hex_to_ass(data.color) or hash_color(data.display or data.user or "user")
                table.insert(chat_events, {
                    ts = data.ts or 0,
                    nick = data.display or data.user or "user",
                    color = color,
                    text = data.text or "",
                    kind = data.kind or "message",
                    badges = data.badges or ""
                })
            end
        end
    end
    f:close()

    table.sort(chat_events, function(a, b) return a.ts < b.ts end)
    mp.msg.info(string.format("Loaded %d chat messages.", #chat_events))
end

-- Find session.json in directory and load it
local function load_session_data(dir)
    local files = utils.readdir(dir, "files")
    if not files then return end

    local session_file = nil
    for _, fn in ipairs(files) do
        if fn:match("%.session%.json$") then
            session_file = fn
            break
        end
    end

    if not session_file then return end
    session_file_path = dir .. "/" .. session_file
    local f = io.open(session_file_path, "r")
    if not f then return end
    local content = f:read("*a")
    f:close()

    local ok, data = pcall(utils.parse_json, content)
    if ok and data then
        session_data = data
        mp.msg.info("Loaded session for channel: " .. tostring(session_data.channel))
    end
end

-- Synchronize chat config to mpv property (so Python renderer reacts immediately) and persist
local function sync_config_to_ipc(persist)
    if persist == nil then persist = true end
    local cfg_obj = {
        width = opts.chat_width,
        height = opts.chat_height,
        font_size = opts.chat_font_size,
        font_family = opts.chat_font,
        x = opts.chat_x,
        y = opts.chat_y,
        opacity = opts.chat_bg_opacity,
        bg_opacity = opts.chat_bg_opacity,
        chat_enabled = (not plugin_hidden) and opts.chat_enabled,
        emotes_enabled = opts.emotes_enabled,
        offset_ms = current_offset_ms,
        scroll_offset = chat_scroll_offset
    }
    local json_str = utils.format_json(cfg_obj)
    mp.set_property("user-data/twitch_chat_config", json_str)

    -- Save to session chat_settings.json only when requested (not on every pixel during dragging)
    if persist and opts.session_dir and opts.session_dir ~= "" then
        local sp = opts.session_dir .. "/chat_settings.json"
        local f = io.open(sp, "w")
        if f then
            f:write(json_str)
            f:close()
        end
    end
end

-- Save current offsets to session.json
local function save_session_offset()
    if not session_data or session_file_path == "" or current_file == "" then return end
    if not session_data.offsets_ms then
        session_data.offsets_ms = {}
    end
    session_data.offsets_ms[current_file] = current_offset_ms

    local f = io.open(session_file_path, "w")
    if f then
        f:write(utils.format_json(session_data))
        f:close()
    end
    sync_config_to_ipc()
end

-- Binary search for message index with ts <= target_ts
local function binary_search_chat(target_ts)
    local low = 1
    local high = #chat_events
    local best = 0

    while low <= high do
        local mid = math.floor((low + high) / 2)
        if chat_events[mid].ts <= target_ts then
            best = mid
            low = mid + 1
        else
            high = mid - 1
        end
    end
    return best
end

-- Escape string for ASS markup
local function escape_ass(text)
    if not text then return "" end
    text = text:gsub("\\", "\\\\")
    text = text:gsub("{", "\\{")
    text = text:gsub("}", "\\}")
    text = text:gsub("\n", " ")
    return text
end

-- Word-wrap helper for Mode 1 pure ASS text
local function wrap_text_lua(text, max_chars)
    if #text <= max_chars then return { text } end
    local lines = {}
    local cur = ""
    for word in text:gmatch("%S+") do
        if #cur == 0 then
            cur = word
        elseif #cur + 1 + #word <= max_chars then
            cur = cur .. " " .. word
        else
            table.insert(lines, cur)
            cur = word
        end
    end
    if #cur > 0 then
        table.insert(lines, cur)
    end
    return lines
end

-- Render Mode 1 Text Chat
function render_text_chat()
    update_overlay_resolutions()
    if plugin_hidden or not opts.chat_enabled or opts.emotes_enabled or #chat_events == 0 or not current_part then
        osd_overlay.data = ""
        osd_overlay:update()
        return
    end

    local time_pos = mp.get_property_number("time-pos")
    if not time_pos then return end

    local current_time_ms = current_part.start_ms + (time_pos * 1000) + current_offset_ms
    local last_idx = binary_search_chat(current_time_ms)
    if last_idx == 0 then
        osd_overlay.data = ""
        osd_overlay:update()
        return
    end

    local eff_idx = last_idx
    if chat_scroll_offset ~= 0 then
        eff_idx = math.max(1, math.min(#chat_events, last_idx - chat_scroll_offset))
    end

    local line_h = math.max(18, math.floor(opts.chat_font_size * 1.55))
    local max_lines = math.floor(opts.chat_height / line_h)
    local max_chars = math.max(16, math.floor(opts.chat_width / (opts.chat_font_size * 0.6)))

    local accumulated_lines = {}
    local msg_entries = {}

    for i = eff_idx, 1, -1 do
        local msg = chat_events[i]
        local clean_text = escape_ass(msg.text)
        local nick_str = escape_ass(msg.nick)
        local full_msg = nick_str .. ": " .. clean_text
        local wrapped = wrap_text_lua(full_msg, max_chars)
        local this_msg_lines = {}

        for w_idx = #wrapped, 1, -1 do
            local w_line = wrapped[w_idx]
            local line_ass
            if w_idx == 1 then
                local clean_nick_prefix = escape_ass(msg.nick .. ": ")
                local rest_text = w_line:sub(#(msg.nick .. ": ") + 1)
                line_ass = string.format("{\\b1\\1c%s}%s{\\b0\\1c&HFFFFFF&}%s", msg.color, clean_nick_prefix, rest_text)
            else
                line_ass = string.format("{\\b0\\1c&HFFFFFF&}  %s", w_line)
            end
            table.insert(this_msg_lines, 1, line_ass)
            if #accumulated_lines + #this_msg_lines >= max_lines then
                break
            end
        end

        for _, l in ipairs(this_msg_lines) do
            table.insert(accumulated_lines, 1, l)
        end
        table.insert(msg_entries, 1, { msg = msg, line_count = #this_msg_lines, idx = i })

        if #accumulated_lines >= max_lines then
            break
        end
    end

    local bg_h = math.min(opts.chat_height, #accumulated_lines * line_h + 16)
    local ass = {}

    -- Background Box (clean, borderless, configurable opacity)
    local alpha_byte = math.floor((1.0 - (opts.chat_bg_opacity or 0.75)) * 255)
    if alpha_byte < 0 then alpha_byte = 0 elseif alpha_byte > 255 then alpha_byte = 255 end
    local alpha_hex = string.format("%02X", alpha_byte)
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord0\\shad0\\1c&H141416&\\1a&H%s&\\p1}m 0 0 l %d 0 l %d %d l 0 %d{\\p0}",
        opts.chat_x - 6, opts.chat_y - 6, alpha_hex, opts.chat_width + 12, opts.chat_width + 12, bg_h + 12, bg_h + 12
    ))

    -- Messages lines & bounding boxes
    visible_chat_messages = {}
    local y_offset = opts.chat_y + 4
    for _, item in ipairs(msg_entries) do
        local item_h = item.line_count * line_h
        table.insert(visible_chat_messages, {
            msg = item.msg,
            idx = item.idx,
            y_min = y_offset,
            y_max = y_offset + item_h
        })
        y_offset = y_offset + item_h
    end

    local draw_y = opts.chat_y + 4
    for _, line_str in ipairs(accumulated_lines) do
        table.insert(ass, string.format(
            "{\\an7\\pos(%d,%d)\\fs%d\\fn%s}%s",
            opts.chat_x, draw_y, opts.chat_font_size, opts.chat_font, line_str
        ))
        draw_y = draw_y + line_h
    end

    -- If scrolled, draw pause/scroll banner at bottom of chat panel
    if chat_scroll_offset ~= 0 then
        local ban_h = 24
        local ban_y = opts.chat_y + bg_h - ban_h - 2
        pause_banner_y_min = ban_y - 6
        pause_banner_y_max = ban_y + ban_h + 8
        table.insert(ass, string.format(
            "{\\an7\\pos(%d,%d)\\bord1\\3c&H9147FF&\\1c&H251838&\\1a&H10&\\p1}m 0 0 l %d 0 l %d %d l 0 %d{\\p0}",
            opts.chat_x - 4, ban_y, opts.chat_width + 8, opts.chat_width + 8, ban_h, ban_h
        ))
        local banner_msg = (chat_scroll_offset > 0)
            and string.format("⬇ Чат: назад (-%d) | Клик: к live", chat_scroll_offset)
            or string.format("⬆ Чат: вперёд (+%d) | Клик: к live", -chat_scroll_offset)
        table.insert(ass, string.format(
            "{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&H00FFFF&}%s",
            opts.chat_x + math.floor(opts.chat_width / 2), ban_y + math.floor(ban_h / 2), banner_msg
        ))
    else
        pause_banner_y_min = nil
        pause_banner_y_max = nil
    end

    osd_overlay.data = table.concat(ass, "\n")
    osd_overlay:update()
end

-- Render Interactive Mouse Control Panel
function render_control_panel()
    update_overlay_resolutions()
    if not panel_visible then
        panel_overlay.data = ""
        panel_overlay:update()
        return
    end

    local panel_w = 600
    local panel_h = 210
    local px = 20
    local py = 20

    local ass = {}

    -- Panel Background with modern dark style & border
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord2\\3c&H9147FF&\\shad2\\1c&H121216&\\1a&H1A&\\p1}m 0 0 l %d 0 l %d %d l 0 %d{\\p0}",
        px, py, panel_w, panel_w, panel_h, panel_h
    ))

    -- Row 1: Header / Status & Info
    local offset_sec = current_offset_ms / 1000.0
    local offset_sign = (offset_sec >= 0) and "+" or ""
    local title_str = string.format("Twitch Chat Overlay | Смещение: %s%.1fs (клик: сброс)", offset_sign, offset_sec)
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs13\\b1\\1c&H9147FF&}%s", px + 12, py + 8, title_str))

    -- Button [ 👁 Скрыть ] (Полное скрытие оверлея)
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H774444&\\1c&H2D1A1A&\\p1}m 0 0 l 70 0 l 70 20 l 0 20{\\p0}",
        px + panel_w - 238, py + 6
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&HFFAAAA&}👁 Скрыть", px + panel_w - 203, py + 16))

    -- Button [ 🔍 Поиск ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H9147FF&\\1c&H2A1E3C&\\p1}m 0 0 l 66 0 l 66 20 l 0 20{\\p0}",
        px + panel_w - 162, py + 6
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&H00FFFF&}🔍 Поиск", px + panel_w - 129, py + 16))

    -- Button [ ⚓ Синхр ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H00D2FF&\\1c&H1A2B3C&\\p1}m 0 0 l 60 0 l 60 20 l 0 20{\\p0}",
        px + panel_w - 90, py + 6
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&H00FFCC&}⚓ Синхр", px + panel_w - 60, py + 16))

    -- Close button [ × ] in header
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H25252E&\\p1}m 0 0 l 20 0 l 20 20 l 0 20{\\p0}",
        px + panel_w - 24, py + 6
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HAAAAAA&}×", px + panel_w - 14, py + 16))

    -- Row 2: Offset Buttons [-10s] [-1s] [-0.1s] [+0.1s] [+1s] [+10s]
    local btn_labels = {"-10s", "-1s", "-0.1s", "+0.1s", "+1s", "+10s"}
    local bx = px + 12
    local by = py + 30
    local bw = 54
    local bh = 22

    for i, lbl in ipairs(btn_labels) do
        table.insert(ass, string.format(
            "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H25252E&\\p1}m 0 0 l %d 0 l %d %d l 0 %d{\\p0}",
            bx, by, bw, bw, bh, bh
        ))
        table.insert(ass, string.format(
            "{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}%s",
            bx + (bw / 2), by + (bh / 2) - 1, lbl
        ))
        bx = bx + bw + 6
    end

    -- Wheel hint text
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\fs11\\1c&H888899&}Колесо: скролл чата",
        bx + 6, by + 4
    ))

    -- Row 3: Chat Size & Font Scale: [A-] [A+] (Шрифт) | [W-] [W+] (Ширина) | [H-] [H+] (Высота)
    local r3_y = py + 58
    local rx = px + 12

    -- Label: Шрифт
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs11\\b1\\1c&HDDDDFF&}Шрифт:", rx, r3_y + 4))
    rx = rx + 48

    -- [ A- ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 32 0 l 32 22 l 0 22{\\p0}",
        rx, r3_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}A-", rx + 16, r3_y + 10))
    rx = rx + 36

    -- [ A+ ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 32 0 l 32 22 l 0 22{\\p0}",
        rx, r3_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}A+", rx + 16, r3_y + 10))
    rx = rx + 36

    -- Font size value
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs11\\1c&H00D2FF&}%dpx", rx, r3_y + 4, opts.chat_font_size))
    rx = rx + 44

    -- [ W- ] [ W+ ]
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs11\\b1\\1c&HDDDDFF&}Шир:", rx, r3_y + 4))
    rx = rx + 34
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 32 0 l 32 22 l 0 22{\\p0}",
        rx, r3_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}W-", rx + 16, r3_y + 10))
    rx = rx + 36
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 32 0 l 32 22 l 0 22{\\p0}",
        rx, r3_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}W+", rx + 16, r3_y + 10))
    rx = rx + 36
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs11\\1c&H00D2FF&}%d", rx, r3_y + 4, opts.chat_width))
    rx = rx + 44

    -- [ H- ] [ H+ ]
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs11\\b1\\1c&HDDDDFF&}Выс:", rx, r3_y + 4))
    rx = rx + 34
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 32 0 l 32 22 l 0 22{\\p0}",
        rx, r3_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}H-", rx + 16, r3_y + 10))
    rx = rx + 36
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 32 0 l 32 22 l 0 22{\\p0}",
        rx, r3_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}H+", rx + 16, r3_y + 10))
    rx = rx + 36
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs11\\1c&H00D2FF&}%d", rx, r3_y + 4, opts.chat_height))

    -- Row 4: Position Presets & Nudge Arrows
    local r4_y = py + 86
    local pos_x = px + 12

    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs11\\b1\\1c&HDDDDFF&}Угол:", pos_x, r4_y + 4))
    pos_x = pos_x + 38

    local pos_presets = {
        {lbl = "↖ ВЛ", name = "TL"},
        {lbl = "↙ НЛ", name = "BL"},
        {lbl = "↗ ВП", name = "TR"},
        {lbl = "↘ НП", name = "BR"}
    }
    for _, pre in ipairs(pos_presets) do
        table.insert(ass, string.format(
            "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 46 0 l 46 22 l 0 22{\\p0}",
            pos_x, r4_y
        ))
        table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&HFFFFFF&}%s", pos_x + 23, r4_y + 10, pre.lbl))
        pos_x = pos_x + 50
    end

    pos_x = pos_x + 10
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs11\\b1\\1c&HDDDDFF&}Сдвиг (20px):", pos_x, r4_y + 4))
    pos_x = pos_x + 86

    local nudge_btns = {"←", "→", "↑", "↓"}
    for _, nlbl in ipairs(nudge_btns) do
        table.insert(ass, string.format(
            "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 34 0 l 34 22 l 0 22{\\p0}",
            pos_x, r4_y
        ))
        table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}%s", pos_x + 17, r4_y + 10, nlbl))
        pos_x = pos_x + 40
    end

    -- Row 5: Font Switcher, Chat Toggle, Emotes Toggle, Background Opacity, Reset View
    local r5_y = py + 114
    local fx = px + 12

    -- [ Шрифт: Segoe UI ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H9147FF&\\1c&H282436&\\p1}m 0 0 l 124 0 l 124 24 l 0 24{\\p0}",
        fx, r5_y
    ))
    table.insert(ass, string.format(
        "{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&HBF94FF&}Шрифт: %s",
        fx + 62, r5_y + 11, opts.chat_font
    ))
    fx = fx + 130

    -- [ Чат: ВКЛ/ВЫКЛ ]
    local chat_text = opts.chat_enabled and "Чат: ВКЛ" or "Чат: ВЫКЛ"
    local chat_col = opts.chat_enabled and "&H55DD55&" or "&H666666&"
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H25252E&\\p1}m 0 0 l 84 0 l 84 24 l 0 24{\\p0}",
        fx, r5_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs10\\b1\\1c%s}%s", fx + 42, r5_y + 11, chat_col, chat_text))
    fx = fx + 90

    -- [ Эмоуты: ВКЛ/ВЫКЛ ]
    local emote_text = opts.emotes_enabled and "Эмоуты: ВКЛ" or "Эмоуты: ВЫКЛ"
    local emote_col = opts.emotes_enabled and "&H55DD55&" or "&H666666&"
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H25252E&\\p1}m 0 0 l 104 0 l 104 24 l 0 24{\\p0}",
        fx, r5_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs10\\b1\\1c%s}%s", fx + 52, r5_y + 11, emote_col, emote_text))
    fx = fx + 110

    -- [ Фон: 75% ]
    local op_pct = math.floor((opts.chat_bg_opacity or 0.75) * 100)
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H25252E&\\p1}m 0 0 l 84 0 l 84 24 l 0 24{\\p0}",
        fx, r5_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&H00D2FF&}Фон: %d%%", fx + 42, r5_y + 11, op_pct))
    fx = fx + 90

    -- [ Сброс вида ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H774444&\\1c&H302222&\\p1}m 0 0 l 84 0 l 84 24 l 0 24{\\p0}",
        fx, r5_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&HFFAAAA&}Сброс", fx + 42, r5_y + 11))

    -- Row 6: 1-Click Sync & Selected Message Indicator
    local r6_y = py + 144
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H3D3D50&\\1c&H1C1C26&\\p1}m 0 0 l %d 0 l %d 26 l 0 26{\\p0}",
        px + 12, r6_y, panel_w - 24, panel_w - 24
    ))
    
    local sync_preview = "Кликните сообщение в чате, чтобы выбрать его как якорь"
    if selected_sync_msg then
        local raw_t = selected_sync_msg.text or ""
        if #raw_t > 28 then raw_t = raw_t:sub(1, 28) .. "..." end
        sync_preview = string.format("Якорь: %s: %s", selected_sync_msg.nick, raw_t)
    end
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\fs10\\1c&HDDDDFF&}%s",
        px + 20, r6_y + 6, escape_ass(sync_preview)
    ))

    -- Button [ ⚓ Синхронизировать с кадром ]
    local btn_sync_w = 210
    local btn_sync_x = px + panel_w - 14 - btn_sync_w
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H00FFCC&\\1c&H1A2E28&\\p1}m 0 0 l %d 0 l %d 22 l 0 22{\\p0}",
        btn_sync_x, r6_y + 2, btn_sync_w, btn_sync_w
    ))
    table.insert(ass, string.format(
        "{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&H00FFCC&}⚓ Синхронизировать с кадром",
        btn_sync_x + math.floor(btn_sync_w / 2), r6_y + 13
    ))

    -- Row 7: Tips
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\fs10\\1c&H888888&}Колесо на чате: скролл. Клик по сообщению: переход к таймкоду. Кнопка [⚓] калибрует session_start_ms.",
        px + 12, py + 178
    ))

    panel_overlay.data = table.concat(ass, "\n")
    panel_overlay:update()
end

-- Adjust offset
local function adjust_offset(delta_ms)
    current_offset_ms = current_offset_ms + delta_ms
    save_session_offset()
    render_control_panel()
    render_text_chat()
    mp.osd_message(string.format("Twitch Offset: %+.1fs", current_offset_ms / 1000.0), 1.2)
end

-- Reset offset to 0
local function reset_offset()
    current_offset_ms = 0
    save_session_offset()
    render_control_panel()
    render_text_chat()
    mp.osd_message("Twitch Offset сброшен на 0.0с", 1.2)
end

-- Adjust chat font size
local function adjust_font_size(delta)
    opts.chat_font_size = math.max(11, math.min(40, opts.chat_font_size + delta))
    sync_config_to_ipc()
    render_control_panel()
    render_text_chat()
    mp.osd_message(string.format("Шрифт чата: %d px (эмоуты масштабированы)", opts.chat_font_size), 1.0)
end

-- Adjust chat width
local function adjust_chat_width(delta)
    opts.chat_width = math.max(260, math.min(1200, opts.chat_width + delta))
    sync_config_to_ipc()
    render_control_panel()
    render_text_chat()
    mp.osd_message(string.format("Ширина чата: %d px", opts.chat_width), 1.0)
end

-- Adjust chat height
local function adjust_chat_height(delta)
    opts.chat_height = math.max(220, math.min(1400, opts.chat_height + delta))
    sync_config_to_ipc()
    render_control_panel()
    render_text_chat()
    mp.osd_message(string.format("Высота чата: %d px", opts.chat_height), 1.0)
end

-- Nudge position
local function nudge_position(dx, dy)
    local layout = get_video_layout()
    opts.chat_x = math.max(0, math.min(layout.vid_w - 60, opts.chat_x + dx))
    opts.chat_y = math.max(0, math.min(layout.vid_h - 60, opts.chat_y + dy))
    sync_config_to_ipc()
    render_control_panel()
    render_text_chat()
    mp.osd_message(string.format("Позиция чата: (%d, %d)", opts.chat_x, opts.chat_y), 0.8)
end

-- Snap position to corner
local function snap_corner(corner)
    local layout = get_video_layout()
    local vid_w = layout.vid_w
    local vid_h = layout.vid_h

    if corner == "TL" then
        opts.chat_x = 24
        opts.chat_y = 60
    elseif corner == "BL" then
        opts.chat_x = 24
        opts.chat_y = math.max(40, vid_h - opts.chat_height - 40)
    elseif corner == "TR" then
        opts.chat_x = math.max(24, vid_w - opts.chat_width - 24)
        opts.chat_y = 60
    elseif corner == "BR" then
        opts.chat_x = math.max(24, vid_w - opts.chat_width - 24)
        opts.chat_y = math.max(40, vid_h - opts.chat_height - 40)
    end

    sync_config_to_ipc()
    render_control_panel()
    render_text_chat()
    mp.osd_message(string.format("Позиция чата: угол %s (%d, %d)", corner, opts.chat_x, opts.chat_y), 1.0)
end

-- Cycle font family
local function cycle_font()
    local cur_idx = 1
    for i, fn in ipairs(FONT_LIST) do
        if fn == opts.chat_font then
            cur_idx = i
            break
        end
    end
    local next_idx = (cur_idx % #FONT_LIST) + 1
    opts.chat_font = FONT_LIST[next_idx]
    sync_config_to_ipc()
    render_control_panel()
    render_text_chat()
    mp.osd_message("Шрифт чата: " .. opts.chat_font, 1.2)
end

-- Reset view to defaults
local function reset_view()
    opts.chat_width = 420
    opts.chat_height = 600
    opts.chat_font_size = 17
    opts.chat_font = "Segoe UI"
    opts.chat_bg_opacity = 0.75
    opts.chat_x = 24
    opts.chat_y = 60
    sync_config_to_ipc()
    render_control_panel()
    render_text_chat()
    mp.osd_message("Вид чата сброшен по умолчанию (420x600, 17px, фон 75%)", 1.2)
end

-- Cycle chat background opacity
local OPACITY_PRESETS = { 1.0, 0.85, 0.75, 0.60, 0.40, 0.20, 0.0 }
local function cycle_opacity()
    local cur = opts.chat_bg_opacity or 0.75
    local next_idx = 1
    for i, p in ipairs(OPACITY_PRESETS) do
        if math.abs(cur - p) < 0.06 then
            next_idx = (i % #OPACITY_PRESETS) + 1
            break
        end
    end
    opts.chat_bg_opacity = OPACITY_PRESETS[next_idx]
    sync_config_to_ipc()
    render_control_panel()
    render_text_chat()
    mp.osd_message(string.format("Прозрачность фона: %d%%", math.floor(opts.chat_bg_opacity * 100)), 1.0)
end

-- Helper to safely get mouse coordinates from mpv native property
local function get_mouse_coords()
    local mpos = mp.get_property_native("mouse-pos")
    if mpos and type(mpos) == "table" and mpos.x and mpos.y then
        mouse_x = mpos.x
        mouse_y = mpos.y
        return mpos.x, mpos.y, (mpos.hover ~= false)
    end
    return mouse_x or 0, mouse_y or 0, false
end

-- Forward declaration of show_panel
-- Forward declarations
local show_panel
local toggle_search
local perform_chat_search
local render_search_panel
local handle_search_panel_click
local enable_search_keys
local disable_search_keys

-- Seek video directly to a message's timestamp
seek_to_message = function(msg)
    if not msg or not current_part then return end
    local seek_sec = (msg.ts - current_part.start_ms - current_offset_ms) / 1000.0
    if seek_sec < 0 then seek_sec = 0 end
    mp.commandv("seek", seek_sec, "absolute")
    selected_sync_msg = msg
    local time_str = format_hms(seek_sec)
    mp.osd_message(string.format("▶ Переход к сообщению [%s]\n%s: %s\n(выбрано как якорь для синхронизации)", time_str, msg.nick, msg.text), 2.5)
    render_control_panel()
    if not opts.emotes_enabled then
        render_text_chat()
    end
end

-- 1-Click Sync: Anchors the video frame to the selected chat message and saves to session.json
sync_session_to_selected_message = function()
    local time_pos = mp.get_property_number("time-pos")
    if not time_pos then
        mp.osd_message("Ошибка: видео не воспроизводится", 1.5)
        return
    end

    local target_msg = selected_sync_msg
    if not target_msg then
        -- If none explicitly clicked, find the message nearest to current playback position
        local cur_ts = current_part.start_ms + math.floor(time_pos * 1000) + current_offset_ms
        local idx = binary_search_chat(cur_ts)
        if idx > 0 and chat_events[idx] then
            target_msg = chat_events[idx]
            selected_sync_msg = target_msg
        else
            mp.osd_message("Сначала кликните по сообщению в чате!", 2.5)
            return
        end
    end

    local vid_pos_ms = math.floor(time_pos * 1000)
    local new_start_ms = target_msg.ts - vid_pos_ms

    current_part.start_ms = new_start_ms
    current_offset_ms = 0

    if session_data then
        session_data.session_start_ms = new_start_ms
        if session_data.parts then
            for _, p in ipairs(session_data.parts) do
                if p.file == current_file or get_filename(p.file) == current_file then
                    p.start_ms = new_start_ms
                end
            end
        end
        if not session_data.offsets_ms then
            session_data.offsets_ms = {}
        end
        session_data.offsets_ms[current_file] = 0

        local f = io.open(session_file_path, "w")
        if f then
            f:write(utils.format_json(session_data))
            f:close()
        end
    end

    sync_config_to_ipc(true)
    render_control_panel()
    render_text_chat()

    local vid_str = format_hms(time_pos)
    local preview = escape_ass(target_msg.nick .. ": " .. (target_msg.text or ""):sub(1, 28))
    mp.osd_message(string.format("⚓ Видео и чат синхронизированы!\nКадр видео: %s\nЯкорь: %s\nsession_start_ms = %d", vid_str, preview, new_start_ms), 3.5)
end

-- Resolve which chat message is at a given Y screen coordinate
local function get_chat_message_at_coord(cy)
    -- Check visible_chat_messages list first (Mode 1 and updated sets)
    for _, item in ipairs(visible_chat_messages) do
        if cy >= item.y_min and cy <= item.y_max then
            return item.msg
        end
    end

    -- If in Mode 2 and visible_chat_messages wasn't populated yet, compute from time_pos
    local time_pos = mp.get_property_number("time-pos")
    if not time_pos or #chat_events == 0 or not current_part then return nil end

    local line_h = math.max(18, math.floor(opts.chat_font_size * 1.55))
    local current_time_ms = current_part.start_ms + (time_pos * 1000) + current_offset_ms
    local last_idx = binary_search_chat(current_time_ms)
    if chat_scroll_offset > 0 then
        last_idx = math.max(1, last_idx - chat_scroll_offset)
    end
    if last_idx == 0 then return nil end

    local max_lines = math.floor(opts.chat_height / line_h)
    local max_chars = math.max(16, math.floor(opts.chat_width / (opts.chat_font_size * 0.6)))

    local msgs_in_view = {}
    local total_lines = 0
    for i = last_idx, 1, -1 do
        local msg = chat_events[i]
        local full_msg = (msg.nick or "") .. ": " .. (msg.text or "")
        local wrapped = wrap_text_lua(full_msg, max_chars)
        local count = #wrapped
        table.insert(msgs_in_view, 1, { msg = msg, lines = count })
        total_lines = total_lines + count
        if total_lines >= max_lines then break end
    end

    local cur_y = opts.chat_y + 4
    for _, item in ipairs(msgs_in_view) do
        local h = item.lines * line_h
        if cy >= cur_y and cy <= cur_y + h then
            return item.msg
        end
        cur_y = cur_y + h
    end

    return nil
end

-- UTF-8 Cyrillic lowercase converter for Russian case-insensitive search
local RUS_UPPER_TO_LOWER = {
    ["А"] = "а", ["Б"] = "б", ["В"] = "в", ["Г"] = "г", ["Д"] = "д",
    ["Е"] = "е", ["Ё"] = "ё", ["Ж"] = "ж", ["З"] = "з", ["И"] = "и",
    ["Й"] = "й", ["К"] = "к", ["Л"] = "л", ["М"] = "м", ["Н"] = "н",
    ["О"] = "о", ["П"] = "п", ["Р"] = "р", ["С"] = "с", ["Т"] = "т",
    ["У"] = "у", ["Ф"] = "ф", ["Х"] = "х", ["Ц"] = "ц", ["Ч"] = "ч",
    ["Ш"] = "ш", ["Щ"] = "щ", ["Ъ"] = "ъ", ["Ы"] = "ы", ["Ь"] = "ь",
    ["Э"] = "э", ["Ю"] = "ю", ["Я"] = "я"
}

local function utf8_lower(str)
    if not str then return "" end
    local s = str:lower()
    for u, l in pairs(RUS_UPPER_TO_LOWER) do
        s = s:gsub(u, l)
    end
    return s
end

-- Safely pop the last character of a UTF-8 string (handles 2-byte Russian & multi-byte chars)
local function utf8_pop_last_char(str)
    if not str or #str == 0 then return "" end
    local len = #str
    local i = len
    while i > 1 and str:byte(i) >= 128 and str:byte(i) <= 191 do
        i = i - 1
    end
    return str:sub(1, i - 1)
end

-- Swap keyboard layout between Russian (ЙЦУКЕН) and English (QWERTY)
local EN_RU_PAIRS = {
    {"q","й"}, {"w","ц"}, {"e","у"}, {"r","к"}, {"t","е"}, {"y","н"}, {"u","г"}, {"i","ш"}, {"o","щ"}, {"p","з"}, {"[","х"}, {"]","ъ"},
    {"a","ф"}, {"s","ы"}, {"d","в"}, {"f","а"}, {"g","п"}, {"h","р"}, {"j","о"}, {"k","л"}, {"l","д"}, {";","ж"}, {"'","э"},
    {"z","я"}, {"x","ч"}, {"c","с"}, {"v","м"}, {"b","и"}, {"n","т"}, {"m","ь"}, {",","б"}, {".","ю"},
    {"Q","Й"}, {"W","Ц"}, {"E","У"}, {"R","К"}, {"T","Е"}, {"Y","Н"}, {"U","Г"}, {"I","Ш"}, {"O","Щ"}, {"P","З"}, {"{","Х"}, {"}","Ъ"},
    {"A","Ф"}, {"S","Ы"}, {"D","В"}, {"F","А"}, {"G","П"}, {"H","Р"}, {"J","О"}, {"K","Л"}, {"L","Д"}, {":","Ж"}, {'"',"Э"},
    {"Z","Я"}, {"X","Ч"}, {"C","С"}, {"V","М"}, {"B","И"}, {"N","Т"}, {"M","Ь"}, {"<","Б"}, {">","Ю"}
}
local LAYOUT_SWAP = {}
for _, pair in ipairs(EN_RU_PAIRS) do
    LAYOUT_SWAP[pair[1]] = pair[2]
    LAYOUT_SWAP[pair[2]] = pair[1]
end

local function swap_keyboard_layout(str)
    if not str or #str == 0 then return "" end
    local result = {}
    local i = 1
    local len = #str
    while i <= len do
        local b = str:byte(i)
        if b < 128 then
            local ch = str:sub(i, i)
            table.insert(result, LAYOUT_SWAP[ch] or ch)
            i = i + 1
        elseif b >= 192 and b <= 223 and i + 1 <= len then
            local ch = str:sub(i, i + 1)
            table.insert(result, LAYOUT_SWAP[ch] or ch)
            i = i + 2
        else
            table.insert(result, str:sub(i, i))
            i = i + 1
        end
    end
    return table.concat(result)
end

-- Perform search in chat messages (supports both Russian and English case-insensitively)
perform_chat_search = function(query)
    search_query = query or ""
    search_results = {}
    search_page = 1
    if #search_query == 0 then
        render_search_panel()
        return
    end

    local q = utf8_lower(search_query)
    for idx, ev in ipairs(chat_events) do
        local txt = utf8_lower(ev.text or "")
        local nck = utf8_lower(ev.nick or "")
        if txt:find(q, 1, true) or nck:find(q, 1, true) then
            local sec = math.max(0, (ev.ts - current_part.start_ms - current_offset_ms) / 1000.0)
            table.insert(search_results, {
                msg = ev,
                sec = sec,
                time_str = format_hms(sec),
            })
        end
    end
    render_search_panel()
end

-- Render Search Overlay Panel (Centered, no chips)
render_search_panel = function()
    update_overlay_resolutions()
    if not search_visible then
        search_overlay.data = ""
        search_overlay:update()
        return
    end

    local layout = get_video_layout()
    local sw = 640
    local sh = 350
    local sx = math.max(10, math.floor((layout.win_w - sw) / 2))
    local sy = math.max(10, math.floor((layout.win_h - sh) / 2))

    local ass = {}

    -- Background Box
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord2\\3c&H9147FF&\\shad3\\1c&H121218&\\1a&H15&\\p1}m 0 0 l %d 0 l %d %d l 0 %d{\\p0}",
        sx, sy, sw, sw, sh, sh
    ))

    -- Header
    local title_str = string.format("🔍 Поиск сообщений в чате | Найдено: %d", #search_results)
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs13\\b1\\1c&H9147FF&}%s", sx + 14, sy + 10, title_str))

    -- Close Button [ × ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H25252E&\\p1}m 0 0 l 22 0 l 22 18 l 0 18{\\p0}",
        sx + sw - 32, sy + 8
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs12\\b1\\1c&HAAAAAA&}×", sx + sw - 21, sy + 17))

    -- Row 2: Search Input Query Box & action buttons
    local q_y = sy + 34
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H444455&\\1c&H1C1C24&\\p1}m 0 0 l 350 0 l 350 24 l 0 24{\\p0}",
        sx + 14, q_y
    ))
    local display_q = search_query
    if #display_q == 0 then
        display_q = "{\\1c&H777788&}Введите слово для поиска (на клавиатуре)..."
    else
        display_q = escape_ass(display_q) .. "{\\1c&H9147FF&}_"
    end
    table.insert(ass, string.format("{\\an7\\pos(%d,%d)\\fs12\\1c&HFFFFFF&} %s", sx + 20, q_y + 4, display_q))

    -- [ RU ⇄ EN ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H00D2FF&\\1c&H1A2B3C&\\p1}m 0 0 l 74 0 l 74 24 l 0 24{\\p0}",
        sx + 372, q_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&H00FFFF&}RU ⇄ EN", sx + 409, q_y + 12))

    -- [ ⌫ Стереть ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 78 0 l 78 24 l 0 24{\\p0}",
        sx + 452, q_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HDDDDFF&}⌫ Стереть", sx + 491, q_y + 12))

    -- [ 🗑 Сброс ]
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H2B2B36&\\p1}m 0 0 l 72 0 l 72 24 l 0 24{\\p0}",
        sx + 536, q_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HDDDDFF&}🗑 Сброс", sx + 572, q_y + 12))

    -- Results List (directly below search box)
    local total_pages = math.max(1, math.ceil(#search_results / search_page_size))
    if search_page > total_pages then search_page = total_pages end
    if search_page < 1 then search_page = 1 end

    local list_y = sy + 68
    local item_h = 32

    if #search_results == 0 then
        local empty_tip = (#search_query == 0) and "Начните вводить текст с клавиатуры для поиска" or "Сообщений по данному запросу не найдено"
        table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs12\\1c&H888899&}%s", sx + math.floor(sw / 2), list_y + 80, empty_tip))
    else
        local start_idx = (search_page - 1) * search_page_size + 1
        local end_idx = math.min(#search_results, start_idx + search_page_size - 1)

        for ri = start_idx, end_idx do
            local res = search_results[ri]
            local iy = list_y + (ri - start_idx) * item_h

            -- Row background box
            table.insert(ass, string.format(
                "{\\an7\\pos(%d,%d)\\bord1\\3c&H333344&\\1c&H1A1A22&\\p1}m 0 0 l 612 0 l 612 28 l 0 28{\\p0}",
                sx + 14, iy
            ))

            -- Time badge [ 01:23:45 ]
            table.insert(ass, string.format(
                "{\\an7\\pos(%d,%d)\\fs11\\b1\\1c&H00FFCC&}[%s]",
                sx + 22, iy + 6, res.time_str
            ))

            -- Nick & Text
            local clean_nick = escape_ass(res.msg.nick or "user")
            local clean_txt = escape_ass(res.msg.text or "")
            if #clean_txt > 45 then clean_txt = clean_txt:sub(1, 45) .. "..." end
            table.insert(ass, string.format(
                "{\\an7\\pos(%d,%d)\\fs11\\b1\\1c%s}%s:{\\b0\\1c&HFFFFFF&} %s",
                sx + 92, iy + 6, res.msg.color or "&HFFFFFF&", clean_nick, clean_txt
            ))

            -- Jump Button [ ▶ Перейти ]
            table.insert(ass, string.format(
                "{\\an7\\pos(%d,%d)\\bord1\\3c&H9147FF&\\1c&H2D2040&\\p1}m 0 0 l 74 0 l 74 22 l 0 22{\\p0}",
                sx + sw - 98, iy + 3
            ))
            table.insert(ass, string.format(
                "{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&H00FFFF&}▶ Перейти",
                sx + sw - 61, iy + 13
            ))
        end
    end

    -- Footer: Pagination [ ◀ Пред. ]  Стр X из Y  [ След. ▶ ]
    local foot_y = sy + sh - 36
    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H25252E&\\p1}m 0 0 l 80 0 l 80 24 l 0 24{\\p0}",
        sx + 14, foot_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}◀ Пред.", sx + 54, foot_y + 12))

    table.insert(ass, string.format(
        "{\\an5\\pos(%d,%d)\\fs11\\1c&HAAAAAA&}Стр. %d из %d (%d сообщ.)",
        sx + math.floor(sw / 2), foot_y + 12, search_page, total_pages, #search_results
    ))

    table.insert(ass, string.format(
        "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H25252E&\\p1}m 0 0 l 80 0 l 80 24 l 0 24{\\p0}",
        sx + sw - 94, foot_y
    ))
    table.insert(ass, string.format("{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&HFFFFFF&}След. ▶", sx + sw - 54, foot_y + 12))

    search_overlay.data = table.concat(ass, "\n")
    search_overlay:update()
end

-- Handle clicks inside Search Panel
handle_search_panel_click = function(mx, my)
    local layout = get_video_layout()
    local sw = 640
    local sh = 350
    local sx = math.max(10, math.floor((layout.win_w - sw) / 2))
    local sy = math.max(10, math.floor((layout.win_h - sh) / 2))

    -- 1. Close button [ × ]
    if mx >= sx + sw - 36 and mx <= sx + sw - 6 and my >= sy + 4 and my <= sy + 28 then
        toggle_search(false)
        return true
    end

    -- 2.0 Click [ RU ⇄ EN ]
    local q_y = sy + 34
    if mx >= sx + 370 and mx <= sx + 448 and my >= q_y - 2 and my <= q_y + 26 then
        if #search_query > 0 then
            perform_chat_search(swap_keyboard_layout(search_query))
        end
        return true
    end

    -- 2.1 [ ⌫ Стереть ]
    if mx >= sx + 450 and mx <= sx + 532 and my >= q_y - 2 and my <= q_y + 26 then
        if #search_query > 0 then
            perform_chat_search(utf8_pop_last_char(search_query))
        end
        return true
    end

    -- 2.2 [ 🗑 Сброс ]
    if mx >= sx + 534 and mx <= sx + 612 and my >= q_y - 2 and my <= q_y + 26 then
        perform_chat_search("")
        return true
    end

    -- 3. Results Row Clicks
    local total_pages = math.max(1, math.ceil(#search_results / search_page_size))
    local list_y = sy + 68
    local item_h = 32
    local start_idx = (search_page - 1) * search_page_size + 1
    local end_idx = math.min(#search_results, start_idx + search_page_size - 1)

    for ri = start_idx, end_idx do
        local iy = list_y + (ri - start_idx) * item_h
        if mx >= sx + 14 and mx <= sx + sw - 14 and my >= iy and my <= iy + 28 then
            local res = search_results[ri]
            seek_to_message(res.msg)
            return true
        end
    end

    -- 4. Pagination [ ◀ Пред. ] / [ След. ▶ ]
    local foot_y = sy + sh - 36
    if mx >= sx + 14 and mx <= sx + 96 and my >= foot_y and my <= foot_y + 26 then
        if search_page > 1 then
            search_page = search_page - 1
            render_search_panel()
        end
        return true
    end
    if mx >= sx + sw - 96 and mx <= sx + sw - 12 and my >= foot_y and my <= foot_y + 26 then
        if search_page < total_pages then
            search_page = search_page + 1
            render_search_panel()
        end
        return true
    end

    return true
end

-- Toggle Search Overlay and Full Bilingual (Cyrillic + Latin) Keyboard Capture
local SEARCH_BINDINGS_LIST = {}

-- Basic printable symbols
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
    { key = "(", char = "(" },
    { key = ")", char = ")" }
}
for _, sym in ipairs(SYM_LIST) do
    table.insert(SEARCH_BINDINGS_LIST, sym)
end

-- Digits 0-9
for d = 0, 9 do
    local s = tostring(d)
    table.insert(SEARCH_BINDINGS_LIST, { key = s, char = s })
end

-- Latin letters a-z and A-Z
local LATIN_CHARS = "abcdefghijklmnopqrstuvwxyz"
for i = 1, #LATIN_CHARS do
    local c = LATIN_CHARS:sub(i, i)
    table.insert(SEARCH_BINDINGS_LIST, { key = c, char = c })
    table.insert(SEARCH_BINDINGS_LIST, { key = c:upper(), char = c:upper() })
    table.insert(SEARCH_BINDINGS_LIST, { key = "Shift+" .. c, char = c:upper() })
end

-- Cyrillic letters (Russian lowercase & uppercase)
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
for i, c in ipairs(CYR_LOWER) do
    local c_up = CYR_UPPER[i] or c
    table.insert(SEARCH_BINDINGS_LIST, { key = c, char = c })
    table.insert(SEARCH_BINDINGS_LIST, { key = c_up, char = c_up })
    table.insert(SEARCH_BINDINGS_LIST, { key = "Shift+" .. c, char = c_up })
end

enable_search_keys = function()
    if search_keys_bound then return end
    search_keys_bound = true

    for i, item in ipairs(SEARCH_BINDINGS_LIST) do
        mp.add_forced_key_binding(item.key, "sk_" .. i, function()
            perform_chat_search(search_query .. item.char)
        end)
    end
    mp.add_forced_key_binding("BS", "sk_bs", function()
        if #search_query > 0 then
            perform_chat_search(utf8_pop_last_char(search_query))
        end
    end)
    mp.add_forced_key_binding("ESC", "sk_esc", function()
        toggle_search(false)
    end)
    mp.add_forced_key_binding("ENTER", "sk_enter", function()
        if #search_results > 0 then
            seek_to_message(search_results[1].msg)
        end
    end)
end

disable_search_keys = function()
    if not search_keys_bound then return end
    search_keys_bound = false

    for i = 1, #SEARCH_BINDINGS_LIST do
        mp.remove_key_binding("sk_" .. i)
    end
    mp.remove_key_binding("sk_bs")
    mp.remove_key_binding("sk_esc")
    mp.remove_key_binding("sk_enter")
end

toggle_search = function(force_state)
    if force_state ~= nil then
        search_visible = force_state
    else
        search_visible = not search_visible
    end

    if search_visible then
        enable_search_keys()
        if #search_query > 0 then
            perform_chat_search(search_query)
        else
            render_search_panel()
        end
    else
        disable_search_keys()
        search_overlay.data = ""
        search_overlay:update()
    end
end

-- Handle Mouse Click on Control Panel
local function handle_control_panel_click(mx, my)
    local px = 20
    local py = 20
    local panel_w = 600

    -- Keep panel active on click
    show_panel(true)

    -- 0. Click Close button [ × ]
    if mx >= px + panel_w - 28 and mx <= px + panel_w - 4 and my >= py + 4 and my <= py + 26 then
        panel_visible = false
        render_control_panel()
        return
    end

    -- 0.05 Click [ 👁 Скрыть ] (Master hide)
    if mx >= px + panel_w - 242 and mx <= px + panel_w - 168 and my >= py + 4 and my <= py + 26 then
        toggle_master_plugin(false)
        return
    end

    -- 0.1 Click [ 🔍 Поиск ]
    if mx >= px + panel_w - 165 and mx <= px + panel_w - 95 and my >= py + 4 and my <= py + 26 then
        toggle_search()
        return
    end

    -- 0.2 Click [ ⚓ Синхр ]
    if mx >= px + panel_w - 92 and mx <= px + panel_w - 30 and my >= py + 4 and my <= py + 26 then
        sync_session_to_selected_message()
        return
    end

    -- 1. Click on Header to reset offset
    if mx >= px + 12 and mx <= px + 280 and my >= py + 6 and my <= py + 26 then
        reset_offset()
        return
    end

    -- 2. Click on offset buttons [-10s]..[+10s]
    local btn_values = {-10000, -1000, -100, 100, 1000, 10000}
    local bx = px + 12
    local by = py + 30
    local bw = 54
    local bh = 22

    for i, delta in ipairs(btn_values) do
        if mx >= bx - 3 and mx <= bx + bw + 3 and my >= by - 3 and my <= by + bh + 4 then
            adjust_offset(delta)
            return
        end
        bx = bx + bw + 6
    end

    -- 3. Row 3 clicks: Font Size [A-] [A+], Width [W-] [W+], Height [H-] [H+]
    local r3_y = py + 58
    local rx = px + 12 + 48

    -- A-
    if mx >= rx - 3 and mx <= rx + 34 and my >= r3_y - 3 and my <= r3_y + 24 then
        adjust_font_size(-1)
        return
    end
    rx = rx + 36

    -- A+
    if mx >= rx - 3 and mx <= rx + 34 and my >= r3_y - 3 and my <= r3_y + 24 then
        adjust_font_size(1)
        return
    end
    rx = rx + 36 + 44 + 34

    -- W-
    if mx >= rx - 3 and mx <= rx + 34 and my >= r3_y - 3 and my <= r3_y + 24 then
        adjust_chat_width(-40)
        return
    end
    rx = rx + 36

    -- W+
    if mx >= rx - 3 and mx <= rx + 34 and my >= r3_y - 3 and my <= r3_y + 24 then
        adjust_chat_width(40)
        return
    end
    rx = rx + 36 + 44 + 34

    -- H-
    if mx >= rx - 3 and mx <= rx + 34 and my >= r3_y - 3 and my <= r3_y + 24 then
        adjust_chat_height(-40)
        return
    end
    rx = rx + 36

    -- H+
    if mx >= rx - 3 and mx <= rx + 34 and my >= r3_y - 3 and my <= r3_y + 24 then
        adjust_chat_height(40)
        return
    end

    -- 4. Row 4 clicks: Corner presets and nudge buttons
    local r4_y = py + 86
    local pos_x = px + 12 + 38

    local pos_presets = {"TL", "BL", "TR", "BR"}
    for _, pre in ipairs(pos_presets) do
        if mx >= pos_x - 3 and mx <= pos_x + 48 and my >= r4_y - 3 and my <= r4_y + 24 then
            snap_corner(pre)
            return
        end
        pos_x = pos_x + 50
    end

    pos_x = pos_x + 10 + 86
    local nudge_actions = {
        {-20, 0},
        {20, 0},
        {0, -20},
        {0, 20}
    }
    for _, act in ipairs(nudge_actions) do
        if mx >= pos_x - 3 and mx <= pos_x + 36 and my >= r4_y - 3 and my <= r4_y + 24 then
            nudge_position(act[1], act[2])
            return
        end
        pos_x = pos_x + 40
    end

    -- 5. Row 5 clicks: Font Switcher, Chat Toggle, Emotes Toggle, Background Opacity, Reset View
    local r5_y = py + 114
    local fx = px + 12

    -- Font switcher
    if mx >= fx - 3 and mx <= fx + 126 and my >= r5_y - 3 and my <= r5_y + 26 then
        cycle_font()
        return
    end
    fx = fx + 130

    -- Chat toggle
    if mx >= fx - 3 and mx <= fx + 86 and my >= r5_y - 3 and my <= r5_y + 26 then
        opts.chat_enabled = not opts.chat_enabled
        sync_config_to_ipc(true)
        update_renderer_mode()
        render_control_panel()
        render_text_chat()
        mp.osd_message("Чат: " .. (opts.chat_enabled and "ВКЛ" or "ВЫКЛ"), 1.0)
        return
    end
    fx = fx + 90

    -- Emotes toggle
    if mx >= fx - 3 and mx <= fx + 106 and my >= r5_y - 3 and my <= r5_y + 26 then
        opts.emotes_enabled = not opts.emotes_enabled
        sync_config_to_ipc(true)
        update_renderer_mode()
        render_control_panel()
        render_text_chat()
        mp.osd_message("Эмоуты (7TV/BTTV/FFZ): " .. (opts.emotes_enabled and "ВКЛ" or "ВЫКЛ"), 1.0)
        return
    end
    fx = fx + 110

    -- Background opacity cycle
    if mx >= fx - 3 and mx <= fx + 86 and my >= r5_y - 3 and my <= r5_y + 26 then
        cycle_opacity()
        return
    end
    fx = fx + 90

    -- Reset view
    if mx >= fx - 3 and mx <= fx + 86 and my >= r5_y - 3 and my <= r5_y + 26 then
        reset_view()
        return
    end

    -- 6. Row 6 clicks: Button [ ⚓ Синхронизировать с кадром ]
    local r6_y = py + 144
    local btn_sync_w = 210
    local btn_sync_x = px + panel_w - 14 - btn_sync_w
    if mx >= btn_sync_x and mx <= btn_sync_x + btn_sync_w and my >= r6_y and my <= r6_y + 26 then
        sync_session_to_selected_message()
        return
    end
end

-- Render floating master toggle button
render_toggle_button = function()
    update_overlay_resolutions()
    local layout = get_video_layout()
    local tb_w = 124
    local tb_h = 24
    local tb_x = layout.win_w - tb_w - 16
    local tb_y = 14

    local ass = {}
    if plugin_hidden then
        table.insert(ass, string.format(
            "{\\an7\\pos(%d,%d)\\bord2\\3c&H9147FF&\\shad2\\1c&H1A1526&\\1a&H10&\\p1}m 0 0 l %d 0 l %d %d l 0 %d{\\p0}",
            tb_x, tb_y, tb_w, tb_w, tb_h, tb_h
        ))
        table.insert(ass, string.format(
            "{\\an5\\pos(%d,%d)\\fs11\\b1\\1c&H00FFFF&}💬 Показать чат",
            tb_x + math.floor(tb_w / 2), tb_y + 12
        ))
    else
        table.insert(ass, string.format(
            "{\\an7\\pos(%d,%d)\\bord1\\3c&H555566&\\1c&H16161E&\\1a&H35&\\p1}m 0 0 l %d 0 l %d %d l 0 %d{\\p0}",
            tb_x, tb_y, tb_w, tb_w, tb_h, tb_h
        ))
        table.insert(ass, string.format(
            "{\\an5\\pos(%d,%d)\\fs10\\b1\\1c&HCCCCCC&}👁 Скрыть чат",
            tb_x + math.floor(tb_w / 2), tb_y + 12
        ))
    end

    toggle_overlay.data = table.concat(ass, "\n")
    toggle_overlay:update()
end

-- Completely show / hide the entire chat plugin
toggle_master_plugin = function(forced_state)
    if forced_state ~= nil then
        plugin_hidden = not forced_state
    else
        plugin_hidden = not plugin_hidden
    end

    if plugin_hidden then
        osd_overlay.data = ""
        osd_overlay:update()
        panel_overlay.data = ""
        panel_overlay:update()
        if search_visible then
            toggle_search(false)
        end
        mp.commandv("overlay-remove", 1)
        sync_config_to_ipc(false)
        mp.osd_message("Twitch Chat скрыт (кликните «💬 Показать чат»)", 1.5)
    else
        sync_config_to_ipc(true)
        update_renderer_mode()
        render_text_chat()
        show_panel(true)
        mp.osd_message("Twitch Chat активен", 1.5)
    end
    render_toggle_button()
end

-- Show / hide control panel on mouse activity
show_panel = function(force_render)
    if plugin_hidden then return end
    local was_hidden = not panel_visible
    panel_visible = true
    if was_hidden or force_render then
        render_control_panel()
    end

    if panel_timer then
        panel_timer:kill()
    end
    panel_timer = mp.add_timeout(6.0, function()
        panel_visible = false
        render_control_panel()
    end)
end

-- Handle Mouse Wheel: Dedicated exclusively to scrolling chat history (bidirectional, smooth and responsive)
local function handle_wheel(is_up, is_fast)
    if plugin_hidden or not opts.chat_enabled or #chat_events == 0 then return end
    local delta = is_fast and 30 or 8

    -- UP = scroll back into past (older messages)
    -- DOWN = scroll forward into future (newer messages)
    if is_up then
        chat_scroll_offset = chat_scroll_offset + delta
    else
        chat_scroll_offset = chat_scroll_offset - delta
    end

    -- Clamp to actual bounds of loaded stream messages
    local time_pos = mp.get_property_number("time-pos") or 0
    local cur_ms = (current_part and current_part.start_ms or 0) + (time_pos * 1000) + current_offset_ms
    local last_idx = binary_search_chat(cur_ms)
    if last_idx > 0 then
        local max_back = last_idx - 1
        local max_forward = -(#chat_events - last_idx)
        if chat_scroll_offset > max_back then chat_scroll_offset = max_back end
        if chat_scroll_offset < max_forward then chat_scroll_offset = max_forward end
    end

    -- Immediate in-memory IPC sync (false = avoid blocking synchronous disk write during scrolling)
    sync_config_to_ipc(false)
    render_text_chat()

    if chat_scroll_offset == 0 then
        mp.osd_message("Чат: live (автоскролл)", 0.5)
    elseif chat_scroll_offset > 0 then
        mp.osd_message(string.format("Чат: назад (-%d)", chat_scroll_offset), 0.5)
    else
        mp.osd_message(string.format("Чат: вперёд (+%d)", -chat_scroll_offset), 0.5)
    end
end

-- Handle Mouse Button click
local function handle_mouse_btn(event)
    local cx, cy, hover = get_mouse_coords()
    if not cx or not cy then return end

    local ev_name = "press"
    if type(event) == "table" and event.event then
        ev_name = event.event
    elseif type(event) == "string" then
        ev_name = event
    end

    if ev_name == "repeat" or ev_name == "up" then
        return
    end

    local layout = get_video_layout()

    -- 0. Check Master Toggle Button click (Always active, whether hidden or shown)
    local tb_w = 124
    local tb_h = 24
    local tb_x = layout.win_w - tb_w - 16
    local tb_y = 14
    if cx >= tb_x and cx <= tb_x + tb_w and cy >= tb_y and cy <= tb_y + tb_h then
        toggle_master_plugin()
        return
    end

    -- If plugin is completely hidden, do not process other UI clicks
    if plugin_hidden then
        return
    end

    -- 1. Check if clicking on Search Overlay panel (Centered)
    if search_visible then
        local sw = 640
        local sh = 350
        local sx = math.max(10, math.floor((layout.win_w - sw) / 2))
        local sy = math.max(10, math.floor((layout.win_h - sh) / 2))
        if cx >= sx and cx <= sx + sw and cy >= sy and cy <= sy + sh then
            handle_search_panel_click(cx, cy)
            return
        end
    end

    -- 2. Check if clicking on Control Panel area
    local px = 20
    local py = 20
    local panel_w = 600
    local panel_h = 210
    if panel_visible and cx >= px and cx <= px + panel_w and cy >= py and cy <= py + panel_h then
        handle_control_panel_click(cx, cy)
        return
    end

    -- 3. Check if clicking on Chat Overlay area
    if opts.chat_enabled and cx >= opts.chat_x - 10 and cx <= opts.chat_x + opts.chat_width + 10
       and cy >= opts.chat_y - 10 and cy <= opts.chat_y + opts.chat_height + 20 then

        -- Check if clicked on the pause banner or bottom of chat area when scrolled
        if chat_scroll_offset ~= 0 then
            local is_banner_click = false
            -- Check recorded banner bounds in Mode 1
            if pause_banner_y_min and cy >= pause_banner_y_min and cy <= pause_banner_y_max then
                is_banner_click = true
            -- Check bottom 50px of chat area (Mode 2 or Mode 1 full height)
            elseif cy >= (opts.chat_y + opts.chat_height - 50) and cy <= (opts.chat_y + opts.chat_height + 20) then
                is_banner_click = true
            -- Check if clicked directly below the last rendered message in Mode 1
            elseif not opts.emotes_enabled and visible_chat_messages and #visible_chat_messages > 0 then
                local last_msg = visible_chat_messages[#visible_chat_messages]
                if last_msg and last_msg.y_max and cy >= (last_msg.y_max - 6) then
                    is_banner_click = true
                end
            end

            if is_banner_click then
                chat_scroll_offset = 0
                sync_config_to_ipc(false)
                render_text_chat()
                mp.osd_message("Автоскролл возобновлен (live)", 1.0)
                return
            end
        end

        -- Find clicked message in chat
        local clicked_msg = get_chat_message_at_coord(cy)
        if clicked_msg then
            seek_to_message(clicked_msg)
            show_panel(true)
            return
        end
    end

    -- 4. Clicking elsewhere shows control panel
    show_panel(true)
end

-- Track mouse movement to smoothly display control panel
local function on_mouse_move(name_or_ev, val)
    local cx, cy, hover = get_mouse_coords()
    if not cx or not cy then return end
    if plugin_hidden then return end

    -- Show or keep panel alive smoothly
    show_panel(false)
end

-- Mode 2: Start / Stop Python renderer
function update_renderer_mode()
    if plugin_hidden or not opts.chat_enabled then
        stop_python_renderer()
        osd_overlay.data = ""
        osd_overlay:update()
        return
    end

    if opts.emotes_enabled then
        start_python_renderer()
        osd_overlay.data = ""
        osd_overlay:update()
    else
        stop_python_renderer()
        render_text_chat()
    end
end

function start_python_renderer()
    if renderer_proc then return end
    if not opts.session_dir or opts.session_dir == "" then return end

    local renderer_script = opts.session_dir .. "/../renderer.py"
    if not file_exists(renderer_script) then
        renderer_script = "renderer.py"
    end

    local ipc_sock = mp.get_property("input-ipc-server")
    if not ipc_sock or ipc_sock == "" then
        ipc_sock = (package.config:sub(1,1) == "\\") and "\\\\.\\pipe\\mpv_socket" or "/tmp/mpv_socket"
        mp.set_property("input-ipc-server", ipc_sock)
    end

    sync_config_to_ipc()

    mp.msg.info("Starting Python Emote Renderer: " .. renderer_script .. " on IPC: " .. ipc_sock)
    renderer_proc = mp.command_native_async({
        name = "subprocess",
        playback_only = false,
        args = {opts.python_path, renderer_script, "--session", opts.session_dir, "--socket", ipc_sock}
    }, function(success, res, err)
        renderer_proc = nil
    end)
end

function stop_python_renderer()
    if renderer_proc then
        mp.commandv("overlay-remove", 1)
        renderer_proc = nil
    end
end

-- File loaded handler
local function on_file_loaded()
    current_file = get_filename(mp.get_property("path", ""))
    local file_dir = get_dir(mp.get_property("path", ""))
    if not opts.session_dir or opts.session_dir == "" then
        opts.session_dir = file_dir
    end

    load_session_data(opts.session_dir)
    load_chat_data(opts.session_dir)

    -- Identify current part in session
    current_part = nil
    if session_data and session_data.parts then
        for _, part in ipairs(session_data.parts) do
            if part.file == current_file or get_filename(part.file) == current_file then
                current_part = part
                break
            end
        end
    end

    if not current_part then
        current_part = {
            file = current_file,
            start_ms = (session_data and session_data.session_start_ms) or 0
        }
    end

    -- Restore saved offset for this file
    if session_data and session_data.offsets_ms and session_data.offsets_ms[current_file] then
        current_offset_ms = session_data.offsets_ms[current_file]
    else
        current_offset_ms = 0
    end

    chat_scroll_offset = 0
    selected_sync_msg = nil

    mp.msg.info(string.format("Active part: %s, start_ms: %d, offset: %d", current_part.file, current_part.start_ms, current_offset_ms))
    sync_config_to_ipc()
    update_renderer_mode()
    render_text_chat()
    render_toggle_button()
end

-- Tick / seek observation
mp.observe_property("time-pos", "number", function(name, val)
    if not plugin_hidden and not opts.emotes_enabled then
        render_text_chat()
    end
end)

-- Purely mouse-driven bindings and continuous mouse tracking (no hotkeys required)
mp.observe_property("mouse-pos", "native", on_mouse_move)
mp.observe_property("osd-dimensions", "native", function()
    update_overlay_resolutions()
    render_toggle_button()
    if not plugin_hidden and panel_visible then
        render_control_panel()
    end
    if not plugin_hidden and search_visible then
        render_search_panel()
    end
end)

mp.add_forced_key_binding("MBTN_LEFT", "twitch_mouse_click", handle_mouse_btn, {complex = true})
mp.add_forced_key_binding("WHEEL_UP", "twitch_wheel_up", function() handle_wheel(true, false) end)
mp.add_forced_key_binding("WHEEL_DOWN", "twitch_wheel_down", function() handle_wheel(false, false) end)
mp.add_forced_key_binding("Shift+WHEEL_UP", "twitch_wheel_up_fast", function() handle_wheel(true, true) end)
mp.add_forced_key_binding("Shift+WHEEL_DOWN", "twitch_wheel_down_fast", function() handle_wheel(false, true) end)

mp.add_key_binding("Ctrl+f", "twitch_search_toggle", function() toggle_search() end)
mp.register_script_message("twitch-chat-search", function(query)
    perform_chat_search(query)
    toggle_search(true)
end)

mp.register_event("file-loaded", on_file_loaded)
mp.register_event("shutdown", function()
    disable_search_keys()
    stop_python_renderer()
end)

-- Render toggle button on initial load
render_toggle_button()
