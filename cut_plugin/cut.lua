mp.msg.info("MPV-CUT LOADED")

utils = require "mp.utils"

local function print(s)
	mp.msg.info(s)
	mp.osd_message(s)
end

local function table_to_str(o)
	if type(o) == 'table' then
		local s = ''
		for k,v in pairs(o) do
			if type(k) ~= 'number' then k = '"'..k..'"' end
			s = s .. '['..k..'] = ' .. table_to_str(v) .. '\n'
		end
		return s
	else
		return tostring(o)
	end
end

local function to_hms(seconds)
	local ms = math.floor((seconds - math.floor(seconds)) * 1000)
	local secs = math.floor(seconds)
	local mins = math.floor(secs / 60)
	secs = secs % 60
	local hours = math.floor(mins / 60)
	mins = mins % 60
	return string.format("%02d-%02d-%02d-%03d", hours, mins, secs, ms)
end

local function next_table_key(t, current)
	local preferred_order = { "COPY", "ENCODE", "Render chat", "LIST" }
	local active_order = {}
	for _, k in ipairs(preferred_order) do
		if t[k] then
			active_order[#active_order + 1] = k
		end
	end
	-- Add any other custom keys from t not in preferred_order
	local remaining = {}
	for k in pairs(t) do
		local found = false
		for _, pk in ipairs(preferred_order) do
			if k == pk then found = true break end
		end
		if not found then remaining[#remaining + 1] = k end
	end
	table.sort(remaining)
	for _, k in ipairs(remaining) do
		active_order[#active_order + 1] = k
	end

	for i = 1, #active_order do
		if active_order[i] == current then
			return active_order[(i % #active_order) + 1]
		end
	end
	return active_order[1] or "COPY"
end

ACTIONS = {}

ACTIONS.COPY = function(d)
	local args = {
		"ffmpeg",
		"-nostdin", "-y",
		"-loglevel", "error",
		"-ss", d.start_time,
		"-t", d.duration,
		"-i", d.inpath,
		"-c", "copy",
		"-map", "0",
		"-dn",
		"-avoid_negative_ts", "make_zero",
		utils.join_path(d.indir, "COPY_" .. d.channel .. "_" .. d.infile_noext .. "_FROM_" .. d.start_time_hms .. "_TO_" .. d.end_time_hms .. d.ext)
	}
	mp.command_native_async({
		name = "subprocess",
		args = args,
		playback_only = false,
	}, function() print("Done") end)
end

ACTIONS.ENCODE = function(d)
	local args = {
		"ffmpeg",
		"-nostdin", "-y",
		"-loglevel", "error",
		"-ss", d.start_time,
		"-t", d.duration,
		"-i", d.inpath,
	}
	local suffix = ""
	if d.crop then
		args[#args + 1] = "-vf"
		args[#args + 1] = string.format("crop=%d:%d:%d:%d", d.crop.w, d.crop.h, d.crop.x, d.crop.y)
		suffix = string.format("_CROP_%dx%d", d.crop.w, d.crop.h)
	end
	local rest = {
		"-pix_fmt", "yuv420p",
		"-crf", "18",
		"-preset", "superfast",
		utils.join_path(d.indir, "ENCODE_" .. d.channel .. "_" .. d.infile_noext .. suffix .. "_FROM_" .. d.start_time_hms .. "_TO_" .. d.end_time_hms .. d.ext)
	}
	for _, v in ipairs(rest) do args[#args + 1] = v end
	mp.command_native_async({
		name = "subprocess",
		args = args,
		playback_only = false,
	}, function() print("Done") end)
end

-- Helpers for Render chat
local function hex_to_ass(hex)
	if not hex or #hex < 7 then return nil end
	local r = hex:sub(2, 3)
	local g = hex:sub(4, 5)
	local b = hex:sub(6, 7)
	return string.format("&H00%s%s%s&", b, g, r)
end

local function hash_color(nick)
	local sum = 0
	for i = 1, #nick do sum = sum + nick:byte(i) end
	local colors = {
		"&H00FF69B4&", "&H001E90FF&", "&H0000FF7F&", "&H0000A5FF&",
		"&H00D2691E&", "&H008A2BE2&", "&H0000D7FF&", "&H0020B2AA&",
		"&H009370DB&", "&H0032CD32&"
	}
	return colors[(sum % #colors) + 1]
end

local function to_ass_time(seconds)
	if not seconds or seconds < 0 then seconds = 0 end
	local cs = math.floor((seconds - math.floor(seconds)) * 100)
	local secs = math.floor(seconds)
	local mins = math.floor(secs / 60)
	secs = secs % 60
	local hours = math.floor(mins / 60)
	mins = mins % 60
	return string.format("%d:%02d:%02d.%02d", hours, mins, secs, cs)
end

local function find_session_context(inpath, indir)
	local files = utils.readdir(indir, "files") or {}
	local sess_file = nil
	local chat_file = nil
	local settings_file = nil

	for _, fn in ipairs(files) do
		if fn:match("%.session%.json$") then sess_file = utils.join_path(indir, fn) end
		if fn:match("%.chat%.jsonl$") then chat_file = utils.join_path(indir, fn) end
		if fn == "chat_settings.json" then settings_file = utils.join_path(indir, fn) end
	end

	-- Check parent directory if not found in current directory
	if not sess_file or not chat_file then
		local parent_dir = utils.split_path(indir:gsub("[/\\]$", ""))
		if parent_dir and parent_dir ~= "" and parent_dir ~= indir then
			local p_files = utils.readdir(parent_dir, "files") or {}
			for _, fn in ipairs(p_files) do
				if not sess_file and fn:match("%.session%.json$") then sess_file = utils.join_path(parent_dir, fn) end
				if not chat_file and fn:match("%.chat%.jsonl$") then chat_file = utils.join_path(parent_dir, fn) end
				if not settings_file and fn == "chat_settings.json" then settings_file = utils.join_path(parent_dir, fn) end
			end
		end
	end

	return sess_file, chat_file, settings_file
end

local function generate_chat_ass(d, sess_path, chat_path, settings_path, ass_path)
	local sf = io.open(sess_path, "r")
	if not sf then return false, "Cannot open session file" end
	local s_content = sf:read("*a")
	sf:close()

	local ok_s, session = pcall(utils.parse_json, s_content)
	if not ok_s or not session then return false, "Cannot parse session JSON" end

	local video_fn = d.infile
	local part_start_ms = nil
	if session.parts then
		for _, p in ipairs(session.parts) do
			if p.file == video_fn or utils.split_path(p.file) == video_fn then
				part_start_ms = p.start_ms
				break
			end
		end
	end
	if not part_start_ms then
		part_start_ms = session.session_start_ms or 0
	end

	local offset_ms = 0
	if session.offsets_ms and session.offsets_ms[video_fn] then
		offset_ms = session.offsets_ms[video_fn]
	end

	local start_sec = tonumber(d.start_time) or 0
	local duration_sec = tonumber(d.duration) or 0
	local end_sec = tonumber(d.end_time) or (start_sec + duration_sec)

	local clip_start_ms = part_start_ms + offset_ms + math.floor(start_sec * 1000)
	local clip_end_ms = part_start_ms + offset_ms + math.floor(end_sec * 1000)

	-- Read chat settings
	local chat_font = "Segoe UI"
	local chat_font_size = 18
	local chat_x = 24
	local chat_y = 60
	local outline_w = 1.5

	if settings_path then
		local st_f = io.open(settings_path, "r")
		if st_f then
			local st_c = st_f:read("*a")
			st_f:close()
			local ok_st, st = pcall(utils.parse_json, st_c)
			if ok_st and st then
				if st.chat_font then chat_font = st.chat_font end
				if st.chat_font_size then chat_font_size = st.chat_font_size end
				if st.chat_x then chat_x = st.chat_x end
				if st.chat_y then chat_y = st.chat_y end
				if st.chat_outline_width then outline_w = st.chat_outline_width end
			end
		end
	end

	local cf = io.open(chat_path, "r")
	if not cf then return false, "Cannot open chat file" end

	local msgs = {}
	for line in cf:lines() do
		local ok_l, msg = pcall(utils.parse_json, line)
		if ok_l and msg then
			local ts = msg.ts or msg.recv
			local user = nil
			local text = nil
			local color = nil

			if msg.text then
				user = msg.display or msg.user or "user"
				text = msg.text
				color = hex_to_ass(msg.color) or hash_color(user)
			elseif msg.raw then
				local tags_str, rest = msg.raw:match("^@([^ ]+)%s+(.*)$")
				if tags_str then
					local tags = {}
					for pair in tags_str:gmatch("([^;]+)") do
						local k, v = pair:match("([^=]+)=(.*)")
						if k then tags[k] = v end
					end
					ts = tonumber(tags["tmi-sent-ts"]) or ts
					user = tags["display-name"] or (rest and rest:match("^:([^!]+)")) or "user"
					color = hex_to_ass(tags["color"]) or hash_color(user)
					text = rest and rest:match(":(.*)$") or ""
				end
			end

			if ts and user and text and text ~= "" then
				-- Accept messages up to 10s before clip start so chat is not empty at 00:00
				if ts >= (clip_start_ms - 10000) and ts <= clip_end_ms then
					table.insert(msgs, {
						ts = ts,
						user = user,
						text = text,
						color = color
					})
				end
			end
		end
	end
	cf:close()

	local vid_w, vid_h = get_video_size()
	if not vid_w or not vid_h then
		vid_w = 1920
		vid_h = 1080
	end

	local af = io.open(ass_path, "w")
	if not af then return false, "Cannot create temp ASS file" end

	af:write("[Script Info]\n")
	af:write("Title: Twitch Chat Render Overlay\n")
	af:write("ScriptType: v4.00+\n")
	af:write(string.format("PlayResX: %d\n", vid_w))
	af:write(string.format("PlayResY: %d\n", vid_h))
	af:write("ScaledBorderAndShadow: yes\n\n")

	af:write("[V4+ Styles]\n")
	af:write("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n")
	af:write(string.format(
		"Style: TwitchChat,%s,%d,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,1,0,0,0,100,100,0,0,1,%.1f,0,1,%d,20,%d,1\n\n",
		chat_font, chat_font_size, outline_w, chat_x, chat_y
	))

	af:write("[Events]\n")
	af:write("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n")

	for _, m in ipairs(msgs) do
		local rel_start = (m.ts - clip_start_ms) / 1000.0
		if rel_start < 0 then rel_start = 0 end
		local rel_end = rel_start + 8.5
		if rel_end > duration_sec then rel_end = duration_sec end

		if rel_start < duration_sec then
			local start_str = to_ass_time(rel_start)
			local end_str = to_ass_time(rel_end)
			local clean_u = m.user:gsub("\\", "\\\\"):gsub("{", "\\{"):gsub("}", "\\}")
			local clean_t = m.text:gsub("\\", "\\\\"):gsub("{", "\\{"):gsub("}", "\\}")
			local u_color = m.color or "&H001E90FF&"

			af:write(string.format(
				"Dialogue: 0,%s,%s,TwitchChat,,0,0,0,,{\\c%s}<b>%s</b>: {\\c&H00FFFFFF&}%s\n",
				start_str, end_str, u_color, clean_u, clean_t
			))
		end
	end

	af:close()
	return true, nil
end

ACTIONS["Render chat"] = function(d)
	local sess_path, chat_path, settings_path = find_session_context(d.inpath, d.indir)
	if not sess_path or not chat_path then
		print("Внимание: Чат сессии не найден! Выполняется стандартный ENCODE...")
		ACTIONS.ENCODE(d)
		return
	end

	local temp_ass = utils.join_path(d.indir, "temp_" .. d.infile_noext .. "_chat.ass")
	local ok, err = generate_chat_ass(d, sess_path, chat_path, settings_path, temp_ass)
	if not ok then
		print("Ошибка генерации чата: " .. tostring(err) .. " (ENCODE без чата)")
		ACTIONS.ENCODE(d)
		return
	end

	local args = {
		"ffmpeg",
		"-nostdin", "-y",
		"-loglevel", "error",
		"-ss", d.start_time,
		"-t", d.duration,
		"-i", d.inpath,
	}

	local escaped_ass = temp_ass:gsub("\\", "/"):gsub(":", "\\:")
	local suffix = ""
	if d.crop then
		args[#args + 1] = "-vf"
		args[#args + 1] = string.format("crop=%d:%d:%d:%d,subtitles='%s'", d.crop.w, d.crop.h, d.crop.x, d.crop.y, escaped_ass)
		suffix = string.format("_CROP_%dx%d", d.crop.w, d.crop.h)
	else
		args[#args + 1] = "-vf"
		args[#args + 1] = string.format("subtitles='%s'", escaped_ass)
	end

	local out_filename = "RENDER_CHAT_" .. d.channel .. "_" .. d.infile_noext .. suffix .. "_FROM_" .. d.start_time_hms .. "_TO_" .. d.end_time_hms .. d.ext
	local out_path = utils.join_path(d.indir, out_filename)

	local rest = {
		"-pix_fmt", "yuv420p",
		"-crf", "18",
		"-preset", "superfast",
		out_path
	}
	for _, v in ipairs(rest) do args[#args + 1] = v end

	print("Рендеринг видео с оверлеем чата...")
	mp.command_native_async({
		name = "subprocess",
		args = args,
		playback_only = false,
	}, function()
		os.remove(temp_ass)
		print("Done: " .. out_filename)
	end)
end

ACTIONS.LIST = function(d)
	local inpath = mp.get_property("path")
	local outpath = inpath .. ".list"
	local file = io.open(outpath, "a")
	if not file then print("Error writing to cut list") return end
	local filesize = file:seek("end")
	local s = "\n" .. d.channel
		.. ":" .. d.start_time
		.. ":" .. d.end_time
	file:write(s)
	local delta = file:seek("end") - filesize
	io.close(file)
	print("Δ " .. delta)
end

ACTION = "COPY"

CHANNEL = 1

CHANNEL_NAMES = {}

KEY_CUT = "c"
KEY_CANCEL_CUT = "C"
KEY_CYCLE_ACTION = "a"
KEY_BOOKMARK_ADD = "i"
KEY_CHANNEL_INC = "="
KEY_CHANNEL_DEC = "-"
KEY_CROP = "v"        -- enter crop mode / confirm selection
KEY_CROP_RESET = "V"  -- reset crop (in crop mode: clear the selection)

-- Pressing KEY_BOOKMARK_ADD within this many seconds of an existing
-- bookmark removes that bookmark instead of adding a new one.
BOOKMARK_TOLERANCE = 0.25

home_config = mp.command_native({"expand-path", "~/.config/mpv-cut/config.lua"})
if pcall(require, "config") then
    mp.msg.info("Loaded config file from script dir")
elseif pcall(dofile, home_config) then
    mp.msg.info("Loaded config file from " .. home_config)
else
    mp.msg.info("No config loaded")
end

for i, v in ipairs(CHANNEL_NAMES) do
    CHANNEL_NAMES[i] = string.gsub(v, ":", "-")
end

if not ACTIONS[ACTION] then ACTION = next_table_key(ACTIONS, nil) end

START_TIME = nil

-- Active crop: {x, y, w, h, vw, vh} in video pixels, or nil
CROP = nil

local function get_current_channel_name()
	return CHANNEL_NAMES[CHANNEL] or tostring(CHANNEL)
end

local function get_data()
	local d = {}
	d.inpath = mp.get_property("path")
	d.indir = utils.split_path(d.inpath)
	d.infile = mp.get_property("filename")
	d.infile_noext = mp.get_property("filename/no-ext")
	d.ext = mp.get_property("filename"):match("^.+(%..+)$") or ".mp4"
	d.channel = get_current_channel_name()
	return d
end

local function get_times(start_time, end_time)
	local d = {}
	d.start_time = tostring(start_time)
	d.end_time = tostring(end_time)
	d.duration = tostring(end_time - start_time)
	d.start_time_hms = tostring(to_hms(start_time))
	d.end_time_hms = tostring(to_hms(end_time))
	d.duration_hms = tostring(to_hms(end_time - start_time))
	return d
end

-- Video size as ffmpeg sees it after auto-rotation (swapped for 90/270 deg)
local function get_video_size()
	local w = mp.get_property_number("video-params/w")
	local h = mp.get_property_number("video-params/h")
	if not w or not h or w <= 0 or h <= 0 then return nil end
	local rot = mp.get_property_number("video-params/rotate") or 0
	if rot % 180 == 90 then w, h = h, w end
	return w, h
end

-- COPY can't crop, so with an active crop it is replaced by ENCODE
local function effective_action()
	if CROP and ACTION == "COPY" then return "ENCODE" end
	return ACTION
end

text_overlay = mp.create_osd_overlay("ass-events")
text_overlay.hidden = true
text_overlay:update()

local function text_overlay_off()
	-- https://github.com/mpv-player/mpv/issues/10227
	text_overlay:update()
	text_overlay.hidden = true
	text_overlay:update()
end

local function text_overlay_on()
	local channel = get_current_channel_name()
	local s = string.format("%s in %s from %s", effective_action(), channel, START_TIME)
	if CROP then s = s .. string.format(" [CROP %dx%d]", CROP.w, CROP.h) end
	text_overlay.data = s
	text_overlay.hidden = false
	text_overlay:update()
end

local function print_or_update_text_overlay(content)
	if START_TIME then text_overlay_on() else print(content) end
end

local function cycle_action()
	ACTION = next_table_key(ACTIONS, ACTION)
	print_or_update_text_overlay("ACTION: " .. ACTION)
end

------------------------------------------------------------------------
-- Visual crop
------------------------------------------------------------------------

local crop_active = false
local crop_sel = nil      -- raw selection in video px: {x1, y1, x2, y2}
local crop_drag = nil     -- current drag state
local crop_was_paused = false
local crop_exit  -- forward declaration

-- Wraps a handler so an error is shown on screen and never leaves crop mode stuck
local function crop_safe(fn)
	return function(...)
		local ok, err = pcall(fn, ...)
		if not ok then
			mp.msg.error("mpv-cut crop error: " .. tostring(err))
			mp.osd_message("mpv-cut crop error: " .. tostring(err), 10)
			if crop_active and crop_exit then pcall(crop_exit) end
		end
	end
end

local crop_poll_timer = nil
local crop_poll_start, crop_poll_stop, crop_on_move  -- defined later
local HANDLE_TOL = 10     -- px, grab distance for edges/corners

crop_overlay = mp.create_osd_overlay("ass-events")
crop_overlay.hidden = true
crop_overlay:update()

local function crop_overlay_off()
	crop_overlay:update()
	crop_overlay.hidden = true
	crop_overlay:update()
end

-- Rounds the raw selection to an even-sized, even-positioned rect inside the frame
local function crop_normalize(s, vw, vh)
	if not s then return nil end
	local x1 = math.max(0, math.min(s.x1, s.x2))
	local x2 = math.min(vw, math.max(s.x1, s.x2))
	local y1 = math.max(0, math.min(s.y1, s.y2))
	local y2 = math.min(vh, math.max(s.y1, s.y2))
	local w = math.floor((x2 - x1) / 2) * 2
	local h = math.floor((y2 - y1) / 2) * 2
	if w < 2 or h < 2 then return nil end
	local x = math.floor(x1 / 2) * 2
	local y = math.floor(y1 / 2) * 2
	if x + w > vw then x = math.floor((vw - w) / 2) * 2 end
	if y + h > vh then y = math.floor((vh - h) / 2) * 2 end
	return {x = x, y = y, w = w, h = h}
end

-- Geometry of the displayed video inside the window (OSD coordinates)
local function crop_dims()
	local d = mp.get_property_native("osd-dimensions")
	if not d then return nil end
	local sw = d.w - d.ml - d.mr
	local sh = d.h - d.mt - d.mb
	if sw <= 0 or sh <= 0 then return nil end
	return d, sw, sh
end

local function screen_to_video(mx, my, vw, vh)
	local d, sw, sh = crop_dims()
	if not d then return nil end
	local vx = (mx - d.ml) / sw * vw
	local vy = (my - d.mt) / sh * vh
	vx = math.max(0, math.min(vw, vx))
	vy = math.max(0, math.min(vh, vy))
	return vx, vy
end

local function video_to_screen(vx, vy, vw, vh)
	local d, sw, sh = crop_dims()
	if not d then return nil end
	return d.ml + vx / vw * sw, d.mt + vy / vh * sh
end

local function crop_draw()
	if not crop_active then return end
	local d = crop_dims()
	local vw, vh = get_video_size()
	if not d or not vw then
		return
	end
	local W, H = d.w, d.h
	crop_overlay.res_x = W
	crop_overlay.res_y = H

	local lines = {}
	local fs = math.max(14, math.floor(H / 22))

	lines[#lines + 1] = string.format(
		"{\\an8\\pos(%.1f,8)\\fs%d\\bord2\\shad0\\1c&HFFFFFF&\\3c&H000000&}Drag: select / move / resize   Enter or %s: confirm   Esc: cancel   %s: clear",
		W / 2, math.max(12, math.floor(fs * 0.6)), KEY_CROP, KEY_CROP_RESET)

	local r = crop_normalize(crop_sel, vw, vh)
	if r then
		local sx1, sy1 = video_to_screen(r.x, r.y, vw, vh)
		local sx2, sy2 = video_to_screen(r.x + r.w, r.y + r.h, vw, vh)

		-- dimmed area outside the selection (inner rect has reversed winding = hole)
		lines[#lines + 1] = string.format(
			"{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H000000&\\1a&H90&\\p1}m 0 0 l %d 0 %d %d 0 %d m %.1f %.1f l %.1f %.1f %.1f %.1f %.1f %.1f{\\p0}",
			W, W, H, H, sx1, sy1, sx1, sy2, sx2, sy2, sx2, sy1)

		-- border
		lines[#lines + 1] = string.format(
			"{\\an7\\pos(0,0)\\bord2\\shad0\\1a&HFF&\\3c&H00D7FF&\\p1}m %.1f %.1f l %.1f %.1f %.1f %.1f %.1f %.1f{\\p0}",
			sx1, sy1, sx2, sy1, sx2, sy2, sx1, sy2)

		-- 8 handles
		local mx, my = (sx1 + sx2) / 2, (sy1 + sy2) / 2
		local pts = {
			{sx1, sy1}, {mx, sy1}, {sx2, sy1},
			{sx1, my},             {sx2, my},
			{sx1, sy2}, {mx, sy2}, {sx2, sy2},
		}
		local path = {}
		for _, p in ipairs(pts) do
			path[#path + 1] = string.format("m %.1f %.1f l %.1f %.1f %.1f %.1f %.1f %.1f",
				p[1] - 4, p[2] - 4, p[1] + 4, p[2] - 4, p[1] + 4, p[2] + 4, p[1] - 4, p[2] + 4)
		end
		lines[#lines + 1] = "{\\an7\\pos(0,0)\\bord1\\shad0\\1c&HFFFFFF&\\3c&H000000&\\p1}"
			.. table.concat(path, " ") .. "{\\p0}"

		-- resolution in the center
		lines[#lines + 1] = string.format(
			"{\\an5\\pos(%.1f,%.1f)\\fs%d\\bord2\\shad0\\1c&HFFFFFF&\\3c&H000000&}%dx%d",
			mx, my, fs, r.w, r.h)
	end

	crop_overlay.data = table.concat(lines, "\n")
	crop_overlay.hidden = false
	crop_overlay:update()
end

local function crop_hit_test(mx, my)
	local vw, vh = get_video_size()
	if not vw then return nil end
	local r = crop_normalize(crop_sel, vw, vh)
	if not r then return nil end
	local sx1, sy1 = video_to_screen(r.x, r.y, vw, vh)
	local sx2, sy2 = video_to_screen(r.x + r.w, r.y + r.h, vw, vh)
	if not sx1 then return nil end
	local tol = HANDLE_TOL

	local in_x = mx >= sx1 - tol and mx <= sx2 + tol
	local in_y = my >= sy1 - tol and my <= sy2 + tol
	local l = in_y and math.abs(mx - sx1) <= tol
	local rr = in_y and math.abs(mx - sx2) <= tol
	if l and rr then
		if math.abs(mx - sx1) <= math.abs(mx - sx2) then rr = false else l = false end
	end
	local t = in_x and math.abs(my - sy1) <= tol
	local b = in_x and math.abs(my - sy2) <= tol
	if t and b then
		if math.abs(my - sy1) <= math.abs(my - sy2) then b = false else t = false end
	end
	if l or rr or t or b then
		return {mode = "resize", l = l, r = rr, t = t, b = b}
	end
	if mx > sx1 and mx < sx2 and my > sy1 and my < sy2 then
		return {mode = "move"}
	end
	return nil
end

local function crop_on_lmb(e)
	if not crop_active then return end
	local vw, vh = get_video_size()
	if not vw then return end
	local mouse_x, mouse_y = mp.get_mouse_pos()
	if e.event == "down" then
		if crop_drag and crop_drag.pending then
			-- finishing a drag that mpv cut short: the cursor position is the final corner
			crop_on_move()
			crop_drag = nil
			crop_poll_stop()
			if crop_sel and not crop_normalize(crop_sel, vw, vh) then crop_sel = nil end
			crop_draw()
			return
		end
		crop_poll_start()
		local vx, vy = screen_to_video(mouse_x, mouse_y, vw, vh)
		if not vx then return end
		local hit = crop_hit_test(mouse_x, mouse_y)
		if hit then
			local r = crop_normalize(crop_sel, vw, vh)
			crop_sel = {x1 = r.x, y1 = r.y, x2 = r.x + r.w, y2 = r.y + r.h}
			crop_drag = hit
			if hit.mode == "move" then
				crop_drag.ox, crop_drag.oy = vx, vy
				crop_drag.orig = {x1 = crop_sel.x1, y1 = crop_sel.y1, x2 = crop_sel.x2, y2 = crop_sel.y2}
			end
		else
			crop_sel = {x1 = vx, y1 = vy, x2 = vx, y2 = vy}
			crop_drag = {mode = "new"}
		end
		crop_drag.sx, crop_drag.sy = vx, vy
	elseif e.event == "up" then
		-- mpv sometimes reports the button release right after the press, while the
		-- mouse has only moved a few pixels. Treat that as an unfinished drag: keep
		-- following the cursor until the next click.
		if crop_drag and not crop_drag.pending and crop_drag.sx then
			local ux, uy = screen_to_video(mouse_x, mouse_y, vw, vh)
			if ux then
				local moved = math.max(math.abs(ux - crop_drag.sx), math.abs(uy - crop_drag.sy))
				if moved >= 2 and moved < 12 then
					crop_drag.pending = true
					crop_draw()
					return
				end
			end
		end
		crop_poll_stop()
		if crop_drag then
			crop_drag = nil
			if crop_sel and not crop_normalize(crop_sel, vw, vh) then crop_sel = nil end
		end
	end
	crop_draw()
end

crop_on_move = function()
	if not crop_active or not crop_drag then return end
	local vw, vh = get_video_size()
	if not vw then return end
	local mouse_x, mouse_y = mp.get_mouse_pos()
	local vx, vy = screen_to_video(mouse_x, mouse_y, vw, vh)
	if not vx then return end
	local drag = crop_drag
	if drag.mode == "new" then
		crop_sel.x2, crop_sel.y2 = vx, vy
	elseif drag.mode == "resize" then
		if drag.l then crop_sel.x1 = vx end
		if drag.r then crop_sel.x2 = vx end
		if drag.t then crop_sel.y1 = vy end
		if drag.b then crop_sel.y2 = vy end
	elseif drag.mode == "move" then
		local o = drag.orig
		local w, h = o.x2 - o.x1, o.y2 - o.y1
		local nx = math.max(0, math.min(vw - w, o.x1 + (vx - drag.ox)))
		local ny = math.max(0, math.min(vh - h, o.y1 + (vy - drag.oy)))
		crop_sel = {x1 = nx, y1 = ny, x2 = nx + w, y2 = ny + h}
	end
	crop_draw()
end

crop_poll_start = function()
	if crop_poll_timer then crop_poll_timer:kill() end
	crop_poll_timer = mp.add_periodic_timer(0.02, crop_safe(function() crop_on_move() end))
end

crop_poll_stop = function()
	if crop_poll_timer then
		crop_poll_timer:kill()
		crop_poll_timer = nil
	end
end

local function crop_on_dims()
	crop_draw()
end
local crop_on_dims_safe = crop_safe(crop_on_dims)
local crop_on_mouse_safe = crop_safe(function() crop_on_move() end)

-- uosc grabs the mouse; ask it to switch off its interactive elements while cropping.
-- Harmless if uosc isn't installed.
local function uosc_set_disabled(disabled)
	local elements = disabled
		and "timeline,controls,volume,top_bar,idle_indicator,window_border,pause_indicator,speed"
		or ""
	mp.commandv("script-message-to", "uosc", "disable-elements", mp.get_script_name(), elements)
end

local CROP_BINDINGS = {
	"crop_lmb", "crop_lmb_dbl", "crop_move",
	"crop_enter", "crop_kpenter", "crop_esc", "crop_clear_sel",
}

crop_exit = function()
	crop_active = false
	crop_drag = nil
	crop_sel = nil
	for _, name in ipairs(CROP_BINDINGS) do mp.remove_key_binding(name) end
	mp.unobserve_property(crop_on_dims_safe)
	mp.unobserve_property(crop_on_mouse_safe)
	crop_poll_stop()
	uosc_set_disabled(false)
	crop_overlay_off()
	if not crop_was_paused then mp.set_property_native("pause", false) end
end

local function crop_confirm()
	local vw, vh = get_video_size()
	local r = vw and crop_normalize(crop_sel, vw, vh)
	crop_exit()
	if not r then
		if CROP then
			CROP = nil
			print_or_update_text_overlay("CROP CLEARED")
		else
			print("NO CROP SELECTED")
		end
		return
	end
	if r.x == 0 and r.y == 0 and r.w >= vw - 1 and r.h >= vh - 1 then
		CROP = nil
		print_or_update_text_overlay("CROP: FULL FRAME (OFF)")
		return
	end
	CROP = {x = r.x, y = r.y, w = r.w, h = r.h, vw = vw, vh = vh}
	print_or_update_text_overlay(string.format("CROP %dx%d at %d,%d", r.w, r.h, r.x, r.y))
end

local function crop_cancel()
	crop_exit()
	print("CROP CANCELLED")
end

local function crop_enter()
	local vw, vh = get_video_size()
	if not vw then print("No video") return end
	crop_active = true
	crop_was_paused = mp.get_property_native("pause") and true or false
	mp.set_property_native("pause", true)

	crop_sel = nil
	crop_drag = nil
	if CROP and CROP.vw == vw and CROP.vh == vh then
		crop_sel = {x1 = CROP.x, y1 = CROP.y, x2 = CROP.x + CROP.w, y2 = CROP.y + CROP.h}
	end

	uosc_set_disabled(true)
	mp.observe_property("osd-dimensions", "native", crop_on_dims_safe)
	mp.observe_property("mouse-pos", "native", crop_on_mouse_safe)
	-- short delay so uosc drops its own mouse bindings before ours are registered
	mp.add_timeout(0.15, crop_safe(function()
		if not crop_active then return end
		mp.add_forced_key_binding("MBTN_LEFT", "crop_lmb", crop_safe(crop_on_lmb), {complex = true})
		mp.add_forced_key_binding("MBTN_LEFT_DBL", "crop_lmb_dbl", function() end)
		mp.add_forced_key_binding("ENTER", "crop_enter", crop_safe(crop_confirm))
		mp.add_forced_key_binding("KP_ENTER", "crop_kpenter", crop_safe(crop_confirm))
		mp.add_forced_key_binding("ESC", "crop_esc", crop_safe(crop_cancel))
		mp.add_forced_key_binding(KEY_CROP_RESET, "crop_clear_sel", crop_safe(function()
			crop_sel = nil
			crop_drag = nil
			crop_poll_stop()
			crop_draw()
		end))
	end))
	crop_draw()
end

local function crop_toggle()
	if crop_active then crop_confirm() else crop_enter() end
end

local function crop_reset()
	if CROP then
		CROP = nil
		print_or_update_text_overlay("CROP CLEARED")
	else
		print("NO CROP SET")
	end
end

------------------------------------------------------------------------
-- Cutting
------------------------------------------------------------------------

local function cut(start_time, end_time)
	local d = get_data()
	local t = get_times(start_time, end_time)
	for k, v in pairs(t) do d[k] = v end

	if CROP then
		local vw, vh = get_video_size()
		if vw and (CROP.vw ~= vw or CROP.vh ~= vh) then
			CROP = nil
			print("CROP RESET (video size changed)")
		else
			d.crop = CROP
		end
	end

	local action = ACTION
	if d.crop and action == "COPY" then action = "ENCODE" end

	mp.msg.info(action)
	mp.msg.info(table_to_str(d))
	ACTIONS[action](d)
end

local function put_time()
	if crop_active then return end
	local time = mp.get_property_number("time-pos")
	if not START_TIME then
		START_TIME = time
		text_overlay_on()
		return
	end
	text_overlay_off()
	if time > START_TIME then
		cut(START_TIME, time)
		START_TIME = nil
	else
		print("INVALID")
		START_TIME = nil
	end
end

local function cancel_cut()
	text_overlay_off()
	START_TIME = nil
	print("CANCELLED CUT")
end

------------------------------------------------------------------------
-- Bookmarks (chapters)
------------------------------------------------------------------------

local function get_bookmark_file_path()
	local d = get_data()
	mp.msg.info(table_to_str(d))
	local outfile = string.format("%s_%s.book", d.channel, d.infile)
	return utils.join_path(d.indir, outfile)
end

local function bookmarks_load(clear_if_missing)
	local inpath = get_bookmark_file_path()
	local file = io.open(inpath, "r")
	if not file then
		if clear_if_missing then mp.set_property_native("chapter-list", {}) end
		return
	end
	local arr = {}
	for line in file:lines() do
		if tonumber(line) then
			table.insert(arr, {
				time = tonumber(line),
				title = "chapter_" .. line
			})
		end
	end
	file:close()
	table.sort(arr, function(a, b) return a.time < b.time end)
	mp.set_property_native("chapter-list", arr)
end

-- Returns list of {time = number, line = original string}
local function bookmarks_read(path)
	local list = {}
	local file = io.open(path, "r")
	if not file then return list end
	for line in file:lines() do
		local n = tonumber(line)
		if n then table.insert(list, {time = n, line = line}) end
	end
	file:close()
	return list
end

local function bookmark_add()
	local d = get_data()
	local outpath = get_bookmark_file_path()
	local time = mp.get_property_number("time-pos")
	if not time then return end

	-- Toggle: if there is a bookmark at (nearly) this position, remove it
	local list = bookmarks_read(outpath)
	local found, best = nil, nil
	for i, b in ipairs(list) do
		local diff = math.abs(b.time - time)
		if diff <= BOOKMARK_TOLERANCE and (not best or diff < best) then
			found, best = i, diff
		end
	end
	if found then
		local removed = table.remove(list, found)
		if #list == 0 then
			os.remove(outpath)
		else
			local file = io.open(outpath, "w")
			if not file then print("Failed to open bookmark file for writing") return end
			for _, b in ipairs(list) do file:write(b.line .. "\n") end
			io.close(file)
		end
		bookmarks_load(true)
		print(string.format("Removed chapter %s, %s", removed.line, d.channel))
		return
	end

	local file = io.open(outpath, "a")
	if not file then print("Failed to open bookmark file for writing") return end
	local out_string = time .. "\n"
	local filesize = file:seek("end")
	file:write(out_string)
	local delta = file:seek("end") - filesize
	io.close(file)
	bookmarks_load()
	print(string.format("Δ %s, %s", delta, d.channel))
end

local function channel_inc()
	CHANNEL = CHANNEL + 1
	bookmarks_load()
	print_or_update_text_overlay(get_current_channel_name())
end

local function channel_dec()
	if CHANNEL >= 2 then CHANNEL = CHANNEL - 1 end
	bookmarks_load()
	print_or_update_text_overlay(get_current_channel_name())
end

mp.add_key_binding(KEY_CUT, "cut", put_time)
mp.add_key_binding(KEY_CANCEL_CUT, "cancel_cut", cancel_cut)
mp.add_key_binding(KEY_BOOKMARK_ADD, "bookmark_add", bookmark_add)
mp.add_key_binding(KEY_CHANNEL_INC, "channel_inc", channel_inc)
mp.add_key_binding(KEY_CHANNEL_DEC, "channel_dec", channel_dec)
mp.add_key_binding(KEY_CYCLE_ACTION, "cycle_action", cycle_action)
mp.add_key_binding(KEY_CROP, "crop", crop_safe(crop_toggle))
mp.add_key_binding(KEY_CROP_RESET, "crop_reset", crop_reset)

mp.register_event('file-loaded', bookmarks_load)
