scripthookvi = {}

local frames, threads, keys = {}, {}, {}
local registry, current, raw_log, get, wall = {}, nil, log, config.get, now
local clock, ticks, pad_now = 0, 0, {}

function on_frame(fn)
  frames[#frames + 1] = { fn = fn, owner = current }
  if current then current.frames = current.frames + 1 end
end

function script(fn)
  threads[#threads + 1] = { co = coroutine.create(fn), wake = 0, owner = current }
  if current then current.threads = current.threads + 1 end
end

function wait(ms) coroutine.yield(ms or 0) end

local raw_down, raw_pad = input.down, input.pad
local queue, fed, fed_pad, holding, release_at = {}, {}, nil, false, 0

function input.down(vk) return fed[vk] or raw_down(vk) end

function input.pad() return fed_pad or raw_pad() end

local function fake_pad(spec)
  local p = raw_pad()
  for name, v in pairs(p) do p[name] = type(v) == "number" and 0 or false end
  p.connected = true
  for k, v in pairs(spec or {}) do
    if math.type(k) == "integer" then p[v] = true else p[k] = v end
  end
  return p
end

function input.feeding() return holding or #queue > 0 end

function input.feed(steps)
  if not input.feeding() then queue[1] = {} end
  for _, step in ipairs(steps) do queue[#queue + 1] = step end
end

local function feed_tick()
  if holding then
    if clock < release_at then return end
    holding, fed = false, {}
    if fed_pad then fed_pad = fake_pad() end
    return
  end
  local step = table.remove(queue, 1)
  if not step then
    fed_pad = nil
    return
  end
  if type(step) == "number" then step = { step } end
  fed = {}
  for _, vk in ipairs(step) do fed[vk] = true end
  if step.pad then fed_pad = fake_pad(step.pad) end
  holding, release_at = true, clock + (step.ms or 0)
end

local function is_down(vk)
  if type(vk) == "string" then return pad_now[vk] == true end
  return input.down(vk)
end

local function track(vk)
  local k = keys[vk]
  if not k then
    local d = is_down(vk)
    k = { d, d, since = clock }
    keys[vk] = k
  end
  return k
end

function input.pressed(vk)
  local k = track(vk)
  return k[2] and not k[1]
end

function input.held(vk)
  local k = track(vk)
  return k[2] and clock - k.since
end

function input.repeated(vk, delay, rate)
  local k = track(vk)
  delay, rate = delay or 400, rate or 120
  if k.seen ~= ticks then
    k.seen, k.fired = ticks, false
    if k[2] and not k[1] then
      k.fired, k.next, k.gap = true, clock + delay, rate
    elseif k[2] and k.next and clock >= k.next then
      k.fired, k.next, k.gap = true, clock + k.gap, math.max(rate / 4, k.gap * 0.85)
    end
  end
  return k.fired
end

function mem.rva(offset) return mem.base() + offset end

function mem.deref(addr, ...)
  for _, offset in ipairs({ ... }) do
    addr = mem.read(addr, "ptr")
    if not addr or addr == 0 then return nil end
    addr = addr + offset
  end
  return addr
end

function mem.struct(addr, layout)
  return setmetatable({}, {
    __index = function(_, k)
      if k == "addr" then return addr end
      local f = layout[k]
      if not f then return nil end
      if type(f[2]) == "table" then return mem.struct(addr + f[1], f[2]) end
      return mem.read(addr + f[1], f[2])
    end,
    __newindex = function(_, k, v)
      local f = layout[k] or error("no field " .. tostring(k))
      mem.write(addr + f[1], f[2], v)
    end,
  })
end

local scratch, scratch_size, scratch_at = mem.alloc(65536), 65536, 0

function mem.cstr(s)
  if #s + 1 > scratch_size then error("mem.cstr: string longer than 65535 bytes", 2) end
  if scratch_at + #s + 1 > scratch_size then scratch_at = 0 end
  local p = scratch + scratch_at
  mem.poke(p, s .. "\0")
  scratch_at = scratch_at + #s + 1
  return p
end

scripthookvi.builds = {}
scripthookvi.offsets = {}

function game.build()
  local stamp = game.stamp()
  if not stamp then return nil end
  return scripthookvi.builds[string.format("%08x:%x", stamp, mem.size())] or "unknown"
end

function game.at(name)
  local known = scripthookvi.offsets[game.build()]
  return known and known[name] and mem.rva(known[name])
end

local thread_queue, thread_armed = {}, false

function game.on_thread(fn)
  if hook.ticking() then return fn() end
  thread_queue[#thread_queue + 1] = fn
end

on_frame(function()
  if not thread_armed and game.mode() == "internal" and mem.found() then
    local at = game.at("frame_update")
    if at then
      thread_armed = true
      if get("engine.game_thread", true) ~= false then hook.tick(at) end
    end
  end
  if not hook.ticking() then return end
  local queue = thread_queue
  thread_queue = {}
  for _, fn in ipairs(queue) do
    local ok, err = pcall(fn)
    if not ok then log("game.on_thread: " .. tostring(err)) end
  end
end)

function game.status()
  local build = game.build()
  if game.mode() == "detached" then return "no game" end
  if not build then return game.mode() .. ", title not found" end
  if build == "unknown" then return game.mode() .. ", unknown build" end
  return game.mode() .. ", build " .. build
end

function mem.sections()
  local base = mem.base()
  local list = {}
  local pe = mem.read(base + 0x3C, "u32")
  if not pe or mem.read(base + pe, "u32") ~= 0x4550 then return list end
  local count = mem.read(base + pe + 6, "u16") or 0
  local at = base + pe + 0x18 + (mem.read(base + pe + 0x14, "u16") or 0)
  for i = 0, math.min(count, 32) - 1 do
    local s = at + i * 40
    local name = (mem.bytes(s, 8) or ""):gsub("%z.*", "")
    local size, rva = mem.read(s + 8, "u32"), mem.read(s + 12, "u32")
    if size and rva then list[#list + 1] = { name = name, addr = base + rva, size = size } end
  end
  return list
end

sig = {}
local sigs, sig_names = {}, {}

local function sig_key(name)
  local stamp = game.stamp()
  return stamp and string.format("sig.%08x:%x.%s", stamp, mem.size(), name)
end

function sig.define(name, def)
  if type(name) ~= "string" or name == "" or name:find("[^%w%p]") or name:find("=") then error("sig.define: bad name " .. tostring(name), 2) end
  local pattern = type(def) == "table" and def.pattern
  local size = type(pattern) == "string" and select(2, pattern:gsub("%S+", "")) or 0
  if size == 0 or not pcall(mem.scan, pattern, 0, 0) then
    error("sig.define: " .. name .. " needs a pattern of hex bytes and ??", 2)
  end
  for _, k in ipairs({ "offset", "deref", "add" }) do
    if def[k] ~= nil and math.type(def[k]) ~= "integer" or k == "deref" and (def[k] or 0) < 0 then
      error("sig.define: " .. name .. " needs a whole number " .. k, 2)
    end
  end
  if def.section ~= nil and type(def.section) ~= "string" then error("sig.define: " .. name .. " needs a section name", 2) end
  local copy = { pattern = pattern, offset = def.offset, rip = def.rip, deref = def.deref, add = def.add, section = def.section }
  if not sigs[name] then sig_names[#sig_names + 1] = name end
  sigs[name] = { def = copy, size = size, scans = sigs[name] and sigs[name].scans or 0 }
end

local function locate(name, s)
  local base, d = mem.base(), s.def
  local start, len = base, mem.size()
  if d.section then
    local found
    for _, sec in ipairs(mem.sections()) do
      if sec.name == d.section then found = sec end
    end
    if not found then return nil, "no section " .. d.section end
    start, len = found.addr, found.size
  end
  local key = sig_key(name)
  local rva = key and config.get(key)
  if math.type(rva) == "integer" then
    local at = base + rva
    if mem.scan(d.pattern, at, s.size, 1)[1] == at then return at, nil, "cache" end
    log(string.format("sig %s: cached +%x no longer matches", name, rva))
    config.set(key, nil)
  end
  local t0 = now()
  local hits = mem.scan(d.pattern, start, len, 16)
  s.scans = s.scans + 1
  log(string.format("sig %s: %d match%s in %.0f ms", name, #hits, #hits == 1 and "" or "es", now() - t0))
  if #hits == 0 then return nil, "not found" end
  if #hits > 1 then return nil, #hits == 16 and "16 or more matches" or #hits .. " matches" end
  if key then config.set(key, hits[1] - base) end
  return hits[1], nil, "scan"
end

local function follow(at, d)
  local addr = at + (d.offset or 0)
  if d.rip then
    local disp = mem.read(addr, "i32")
    if not disp then return nil, "unreadable" end
    addr = addr + 4 + disp
  end
  for _ = 1, d.deref or 0 do
    addr = mem.read(addr, "ptr")
    if not addr then return nil, "unreadable" end
    if addr == 0 then return nil, "null pointer" end
  end
  return addr + (d.add or 0)
end

function sig.get(name)
  local s = sigs[name]
  if not s then return nil, "not defined" end
  if not mem.found() then return nil, "no title" end
  local base = mem.base()
  if s.base ~= base then
    s.base, s.addr = base, nil
    s.match, s.reason, s.from = locate(name, s)
  end
  if s.match and not s.addr then s.addr, s.reason = follow(s.match, s.def) end
  if s.addr then return s.addr end
  return nil, s.reason
end

function sig.all()
  local list, base = {}, mem.found() and mem.base()
  for i, name in ipairs(sig_names) do
    local s = sigs[name]
    local fresh = s.base == base
    list[i] = {
      name = name,
      pattern = s.def.pattern,
      state = not fresh and "pending" or s.addr and "found" or "failed",
      addr = fresh and s.addr or nil,
      rva = fresh and s.addr and s.addr - base or nil,
      reason = fresh and s.reason or nil,
      from = fresh and s.from or nil,
      scans = s.scans,
    }
  end
  return list
end

function sig.rescan()
  local found = mem.found()
  for _, name in ipairs(sig_names) do
    sigs[name].base = nil
    local key = found and sig_key(name)
    if key then config.set(key, nil) end
  end
end

menu = {
  key = 0x73, open = false, root = nil, stack = {}, x = 40, y = 40, width = 432, rows = 12,
  accent = 0xC2185B, side = "left", scale = 1, opacity = 82, keys = "both", block = true, sound = nil,
  pad = true, typing = nil,
}
local Menu = {}
Menu.__index = Menu

local numpad = { up = { 0x68 }, down = { 0x62 }, select = { 0x65 }, back = { 0x60 }, left = { 0x64 }, right = { 0x66 } }
local arrows = { up = { 0x26 }, down = { 0x28 }, select = { 0x0D }, back = { 0x08 }, left = { 0x25 }, right = { 0x27 } }
local both = {}
for name, vks in pairs(numpad) do both[name] = { vks[1], arrows[name][1] } end
menu.key_sets = { numpad = numpad, arrows = arrows, both = both }
local pad_keys = { up = "dpad_up", down = "dpad_down", select = "a", back = "b", left = "dpad_left", right = "dpad_right" }

local typed = {
  [0x20] = "  ", [0xBA] = ";:", [0xBB] = "=+", [0xBC] = ",<", [0xBD] = "-_", [0xBE] = ".>", [0xBF] = "/?",
  [0xC0] = "`~", [0xDB] = "[{", [0xDC] = "\\|", [0xDD] = "]}", [0xDE] = "'\"",
}
for i = 0, 9 do typed[0x30 + i] = i .. (")!@#$%^&*("):sub(i + 1, i + 1) end
for i = 0, 25 do typed[0x41 + i] = string.char(0x61 + i, 0x41 + i) end

function menu.new(title)
  local m = setmetatable({ title = title, items = {}, sel = 1 }, Menu)
  menu.root = menu.root or m
  return m
end

local function add(m, item)
  m.items[#m.items + 1] = item
  if current then
    item.file = current.name
    current.items = current.items + 1
  end
  return item
end

function Menu:action(label, fn, desc)
  return add(self, { kind = "action", label = label, fn = fn, desc = desc })
end

function Menu:toggle(label, value, fn, desc)
  return add(self, { kind = "toggle", label = label, value = value and true or false, fn = fn, desc = desc })
end

function Menu:number(label, value, step, fn, desc)
  return add(self, { kind = "number", label = label, value = value, step = step or 1, fn = fn, desc = desc })
end

function Menu:choice(label, options, index, fn, desc)
  return add(self, { kind = "choice", label = label, options = options, value = index or 1, fn = fn, desc = desc })
end

function Menu:text(label, value, fn, desc)
  return add(self, { kind = "text", label = label, value = value or "", fn = fn, desc = desc })
end

function Menu:label(fn, desc)
  return add(self, { kind = "label", text = fn, desc = desc })
end

function Menu:submenu(label, desc)
  local m = setmetatable({ title = label, items = {}, sel = 1 }, Menu)
  m.entry = add(self, { kind = "menu", label = label, menu = m, desc = desc })
  return m
end

local function guarded(fn, ...)
  if not fn then return end
  local ok, err = pcall(fn, ...)
  if not ok then log("menu: " .. tostring(err)) end
end

local function sound(name)
  if menu.sound then guarded(menu.sound, name) end
end

local function value_of(v)
  if type(v) ~= "function" then return v end
  local ok, a, b = pcall(v)
  if not ok then return "?" end
  return a, b
end

function menu.why(item)
  if not item or not item.unavailable then return nil end
  local ok, why = pcall(item.unavailable)
  if not ok then return "error" end
  if why == true then return "unavailable" end
  return why or nil
end

function menu.shown(item)
  if item.kind == "number" then return item.format and item.format(item.value) or tostring(item.value) end
  if item.kind == "choice" then return tostring(item.options[item.value] or "") end
  return nil
end

function menu.change(item, d)
  if item.kind == "choice" then
    local n = #item.options
    if n == 0 then return end
    item.value = (item.value - 1 + d) % n + 1
    guarded(item.fn, item.value, item.options[item.value])
    return
  end
  local v = item.value + d * item.step
  if item.min and v < item.min then v = item.wrap and item.max or item.min end
  if item.max and v > item.max then v = item.wrap and item.min or item.max end
  if v ~= item.value then
    item.value = v
    guarded(item.fn, v)
  end
end

function menu.enter(m)
  menu.stack[#menu.stack + 1] = m
  if m.on_open then guarded(m.on_open, m) end
end

local function hit(set, name, test)
  test = test or input.pressed
  for _, vk in ipairs(set[name]) do
    if test(vk) then return true end
  end
  return menu.pad and test(pad_keys[name]) or false
end

local blocked, leaving = "", false

local function block_keys()
  local want
  leaving = leaving and (input.down(0x0D) or input.down(0x1B))
  if menu.open and menu.block then
    local set = menu.key_sets[menu.keys] or both
    want = {}
    for _, name in ipairs({ "up", "down", "select", "back", "left", "right" }) do
      for _, vk in ipairs(set[name]) do want[#want + 1] = vk end
    end
    if menu.typing or leaving then
      for vk in pairs(typed) do want[#want + 1] = vk end
      for _, vk in ipairs({ 0x08, 0x0D, 0x10, 0x1B, 0xA0, 0xA1 }) do want[#want + 1] = vk end
    end
  end
  local sig = want and table.concat(want, ",") or ""
  if sig ~= blocked then
    input.block(want)
    blocked = sig
  end
end

local function type_tick()
  local t = menu.typing
  if input.pressed(0x0D) or (menu.pad and input.pressed("a")) then
    menu.typing, leaving = nil, true
    t.item.value = t.text
    sound("select")
    guarded(t.item.fn, t.text)
    return
  end
  if input.pressed(0x1B) or (menu.pad and input.pressed("b")) then
    menu.typing, leaving = nil, true
    sound("back")
    return
  end
  if input.repeated(0x08) then t.text = t.text:sub(1, (utf8.offset(t.text, -1) or 1) - 1) end
  local shift = input.down(0x10)
  for vk, chars in pairs(typed) do
    if input.repeated(vk) then t.text = t.text .. chars:sub(shift and 2 or 1, shift and 2 or 1) end
  end
end

local function navigate(set, m)
  local n = #m.items
  if n > 0 then
    if m.sel > n then m.sel = n end
    if hit(set, "up", input.repeated) then m.sel = (m.sel - 2) % n + 1; sound("move") end
    if hit(set, "down", input.repeated) then m.sel = m.sel % n + 1; sound("move") end
    local item = m.items[m.sel]
    local why = menu.why(item)
    if hit(set, "select") then
      if why then
        sound("refuse")
      elseif item.kind == "action" then
        sound("select")
        guarded(item.fn)
      elseif item.kind == "toggle" then
        sound("select")
        item.value = not item.value
        guarded(item.fn, item.value)
      elseif item.kind == "menu" then
        sound("select")
        menu.enter(item.menu)
      elseif item.kind == "text" then
        sound("select")
        menu.typing = { item = item, text = tostring(item.value) }
      end
    end
    if not why and (item.kind == "number" or item.kind == "choice") then
      local d = (hit(set, "right", input.repeated) and 1 or 0) - (hit(set, "left", input.repeated) and 1 or 0)
      if d ~= 0 then
        sound("move")
        menu.change(item, d)
      end
    end
  end
  if hit(set, "back") then
    sound("back")
    if #menu.stack > 0 then menu.stack[#menu.stack] = nil else menu.open = false end
  end
end

local function menu_tick()
  local pad_toggle = menu.pad and input.pressed("a") and pad_now.rb
  if input.pressed(menu.key) or pad_toggle then
    menu.open = not menu.open
    menu.stack = {}
    menu.typing = nil
    sound(menu.open and "open" or "close")
  end
  block_keys()
  if not menu.open or not menu.root then return end
  if menu.typing then
    type_tick()
  elseif not pad_toggle then
    navigate(menu.key_sets[menu.keys] or both, menu.stack[#menu.stack] or menu.root)
  end
  menu.draw(menu.stack[#menu.stack] or menu.root)
end

local function wrap(text, width, size)
  local lines, line = {}, ""
  for word in tostring(text):gmatch("%S+") do
    local try = line == "" and word or line .. " " .. word
    if line ~= "" and ui.text_width(try, size) > width then
      lines[#lines + 1] = line
      line = word
    else
      line = try
    end
  end
  if line ~= "" then lines[#lines + 1] = line end
  return lines
end

function menu.fit(text, width, size, font)
  text = tostring(text)
  if ui.text_width(text, size, font) <= width then return text end
  local cut = #text
  while cut > 0 and ui.text_width(text:sub(1, cut) .. "…", size, font) > width do cut = utf8.offset(text, 0, cut) - 1 end
  return text:sub(1, cut) .. "…"
end

function menu.draw(m)
  local s = ui.height() / 1080
  local t = s * math.max(0.5, math.min(2, menu.scale or 1))
  local function px(v) return math.floor(v * s + 0.5) end
  local function tx(v) return math.floor(v * t + 0.5) end
  local function mid(top, h, size, font) return top + math.floor((h - ui.line_height(size, font)) / 2 + 0.5) end
  local n = #m.items
  local rows = math.max(1, math.floor(menu.rows))
  local w = px(menu.width)
  local x = menu.side == "right" and ui.width() - px(menu.x) - w or px(menu.x)
  local y = px(menu.y)
  local pad, row, size = px(16), tx(38), tx(18)
  local head, bar, small = tx(76), tx(34), tx(14)
  local root = menu.root or m
  local accent = 0xF5000000 | (menu.accent & 0xFFFFFF)
  local alpha = math.floor(math.max(0, math.min(100, menu.opacity or 82)) * 2.55 + 0.5)
  local row_bg, label_bg = alpha << 24, math.min(255, alpha + 14) << 24
  menu.style = {
    s = s, px = px, tx = tx, mid = mid, pad = pad, row = row, size = size, small = small, bar = bar,
    accent = accent, row_bg = row_bg, label_bg = label_bg, bar_bg = 0xF5000000,
  }

  ui.rect(x, y, w, head, accent)
  ui.text(x + pad, mid(y, head, tx(34), "bold"), root.title, 0xFFFFFFFF, tx(34), "bold")
  y = y + head
  local top = y

  local name = #menu.stack > 0 and m.title or "Main menu"
  local count = string.format("%d / %d", n > 0 and m.sel or 0, n)
  ui.rect(x, y, w, bar, 0xF5000000)
  ui.text(x + pad, mid(y, bar, small, "bold"), string.upper(name), 0xFFFFFFFF, small, "bold")
  ui.text(x + w - pad - ui.text_width(count, small), mid(y, bar, small), count, 0xFFC8C8C8, small)
  y = y + bar

  local first = math.max(1, math.min(m.sel - rows + 1, n - rows + 1))
  if m.sel < first then first = m.sel end
  local last = math.min(n, first + rows - 1)
  for i = first, last do
    local item = m.items[i]
    local selected = i == m.sel
    local why = menu.why(item)
    local ink = selected and 0xFF000000 or 0xFFFFFFFF
    local soft = selected and 0xFF5A5A5A or 0xFFA8A8A8
    local dim = selected and 0xFF8C8C8C or 0xFF767676
    local ty = mid(y, row, size)
    local right = x + w - pad
    local label_w = 0
    local face = item.font
    local label_face = item.kind == "action" and face or nil
    ui.rect(x, y, w, row, selected and 0xF8FFFFFF or (item.kind == "label" and label_bg or row_bg))
    if why then
      local reason = menu.fit(why, math.floor(w * 0.45), small)
      local rw = ui.text_width(reason, small)
      local label = item.kind == "label" and (item.label or value_of(item.text)) or value_of(item.label)
      ui.text(x + pad, ty, menu.fit(label, w - 3 * pad - rw, size, label_face), dim, size, label_face)
      ui.text(right - rw, mid(y, row, small), reason, dim, small)
    elseif item.kind == "label" then
      local text, value = value_of(item.text)
      if value ~= nil then
        value = tostring(value)
        local vw = math.min(ui.text_width(value, size, face), math.floor(w * 0.62))
        ui.text(x + pad, ty, menu.fit(text, w - 3 * pad - vw, size), soft, size)
        ui.text(right - vw, ty, menu.fit(value, vw, size, face), ink, size, face)
      else
        ui.text(x + pad, ty, menu.fit(text, w - 2 * pad, size), soft, size)
      end
    else
      local label = tostring(value_of(item.label))
      if item.kind == "toggle" and item.note then
        label = menu.fit(label, w - 3 * pad - px(10) - tx(20) - ui.text_width(tostring(value_of(item.note)), small), size)
      end
      label_w = ui.text_width(label, size, label_face)
      ui.text(x + pad, ty, label, ink, size, label_face)
    end
    local control = not why and item.kind
    if control == "toggle" then
      local b, th = tx(20), math.max(1, tx(2))
      local bx, by = right - b, y + math.floor((row - b) / 2 + 0.5)
      if item.note then
        local note = tostring(value_of(item.note))
        ui.text(bx - px(10) - ui.text_width(note, small), mid(y, row, small), note, soft, small)
      end
      if item.value then
        local tick = tx(16)
        ui.rect(bx, by, b, b, ink)
        ui.text(bx + math.floor((b - ui.text_width("✓", tick, "bold")) / 2 + 0.5), mid(by, b, tick, "bold"), "✓", selected and 0xFFFFFFFF or 0xFF000000, tick, "bold")
      else
        ui.rect(bx, by, b, th, ink)
        ui.rect(bx, by + b - th, b, th, ink)
        ui.rect(bx, by + th, th, b - 2 * th, ink)
        ui.rect(bx + b - th, by + th, th, b - 2 * th, ink)
      end
    elseif control == "number" or control == "choice" then
      local gap = px(8)
      local rx = right - ui.text_width("›", size)
      local room = rx - 2 * gap - ui.text_width("‹", size) - (x + pad + label_w + pad)
      local value = menu.fit(menu.shown(item), math.max(room, ui.text_width("…", size)), size, face)
      ui.text(rx, ty, "›", selected and ink or soft, size)
      rx = rx - gap - ui.text_width(value, size, face)
      ui.text(rx, ty, value, ink, size, face)
      rx = rx - gap - ui.text_width("‹", size)
      ui.text(rx, ty, "‹", selected and ink or soft, size)
    elseif control == "menu" then
      ui.text(right - ui.text_width("›", size), ty, "›", ink, size)
    elseif control == "text" then
      local value = menu.fit(item.value, math.max(right - (x + pad + label_w + pad), ui.text_width("…", size)), size)
      ui.text(right - ui.text_width(value, size), ty, value, ink, size)
    end
    y = y + row
  end

  if n > rows then
    local foot, arrow = tx(28), tx(16)
    local ay = mid(y, foot, arrow)
    local half = ui.text_width("↑", arrow) + px(5)
    ui.rect(x, y, w, foot, 0xF5000000)
    ui.text(x + math.floor(w / 2) - half, ay, "↑", first > 1 and 0xFFFFFFFF or 0xFF505050, arrow)
    ui.text(x + math.floor(w / 2) + px(5), ay, "↓", last < n and 0xFFFFFFFF or 0xFF505050, arrow)
    y = y + foot
  end

  local item = m.items[m.sel]
  local desc = item and value_of(item.desc)
  local typing = menu.typing
  if typing then desc = "Enter accepts, Esc cancels, Backspace deletes." end
  if desc then
    local dsize = tx(15)
    local lh = ui.line_height(dsize)
    local lines = wrap(desc, w - 2 * pad, dsize)
    local inset = px(11)
    y = y + px(6)
    ui.rect(x, y, w, math.max(1, px(3)), accent)
    y = y + math.max(1, px(3))
    if typing then
      local caret, shown = math.max(1, px(2)), typing.text
      while shown ~= "" and ui.text_width(shown, size) > w - 2 * pad - 2 * caret do shown = shown:sub(utf8.offset(shown, 2) or #shown + 1) end
      ui.rect(x, y, w, row, 0xF8FFFFFF)
      ui.text(x + pad, mid(y, row, size), shown, 0xFF000000, size)
      local ch = ui.line_height(size)
      ui.rect(x + pad + math.floor(ui.text_width(shown, size) + 0.5) + caret, mid(y, row, size), caret, ch, accent)
      y = y + row
    end
    ui.rect(x, y, w, #lines * lh + 2 * inset, label_bg)
    for i, line in ipairs(lines) do
      ui.text(x + pad, y + inset + (i - 1) * lh, line, 0xFFFFFFFF, dsize)
    end
  end

  local side = m.side
  if side then
    local sw, gap = px(side.width or 560), px(12)
    local sx = menu.side == "right" and x - gap - sw or x + w + gap
    local rule = math.max(1, px(3))
    ui.rect(sx, top - rule, sw, rule, accent)
    ui.rect(sx, top, sw, bar, 0xF5000000)
    ui.text(sx + pad, mid(top, bar, small, "bold"), string.upper(tostring(value_of(side.title) or "")), 0xFFFFFFFF, small, "bold")
    local info = side.info and tostring(value_of(side.info))
    if info then ui.text(sx + sw - pad - ui.text_width(info, small), mid(top, bar, small), info, 0xFFC8C8C8, small) end
    guarded(side.draw, sx, top + bar, sw, menu.style)
  end
end

local function fail(entry, err)
  local ok, text = pcall(tostring, err)
  if not ok then text = "?" end
  entry.state, entry.error = "failed", text
  entry.line = tonumber(text:match((entry.name:gsub("%p", "%%%0")) .. ":(%d+):") or text:match(":(%d+):"))
end

local function fault(owner, kind, err)
  if owner then
    fail(owner, err)
    log(string.format("%s: %s stopped: %s", owner.name, kind, owner.error))
  else
    log(kind .. ": " .. tostring(err))
  end
end

local menu_failed

function scripthookvi.tick(now_ms)
  clock, ticks = now_ms, ticks + 1
  feed_tick()
  pad_now = input.pad()
  for vk, k in pairs(keys) do
    k[1] = k[2]
    k[2] = is_down(vk)
    if k[2] and not k[1] then k.since = clock end
    if not k[2] then k.next = nil end
  end
  for _, f in ipairs(frames) do
    if not f.parked then
      current = f.owner
      local ok, err = pcall(f.fn, now_ms)
      if not ok then
        f.parked = true
        fault(f.owner, "on_frame", err)
      end
    end
  end
  for i = #threads, 1, -1 do
    local t = threads[i]
    if now_ms >= t.wake then
      current = t.owner
      local ok, ms = coroutine.resume(t.co)
      if not ok then fault(t.owner, "script", ms) end
      if coroutine.status(t.co) == "dead" then
        table.remove(threads, i)
      else
        t.wake = now_ms + (tonumber(ms) or 0)
      end
    end
  end
  current = nil
  local ok, err = pcall(menu_tick)
  if not ok then
    menu.open = false
    if tostring(err) ~= menu_failed then log("menu: " .. tostring(err)) end
    menu_failed = tostring(err)
  end
end

function scripthookvi.scripts()
  local out = {}
  for i, entry in ipairs(registry) do
    local copy = {}
    for k, v in pairs(entry) do copy[k] = v end
    copy.enabled = entry.core or get("scripts." .. entry.name .. ".enabled", true) ~= false
    out[i] = copy
  end
  return out
end

return function(name, chunk)
  local entry = { name = name, state = "loaded", load_ms = 0, frames = 0, threads = 0, items = 0, core = name:match("^0%d_") ~= nil, user = name:sub(1, 5) == "user/" }
  registry[#registry + 1] = entry
  if not entry.core and get("scripts." .. name .. ".enabled", true) == false then
    entry.state = "disabled"
    raw_log(name .. ": disabled in the settings")
    return
  end
  if type(chunk) ~= "function" then
    fail(entry, chunk)
    raw_log(name .. ": " .. entry.error)
    return
  end
  local start = wall()
  current = entry
  local ok, err = pcall(chunk)
  current = nil
  entry.load_ms = wall() - start
  if ok then
    raw_log("loaded " .. name)
  else
    fail(entry, err)
    raw_log(name .. ": " .. entry.error)
  end
end
