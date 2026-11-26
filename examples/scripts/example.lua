local m = menu.new("Example")

local greetings = 0
m:action("Say hello", function()
  greetings = greetings + 1
  log("hello number", greetings)
end, "Writes a line to the log each time you select it.")

m:toggle("Show the clock", config.get("example.clock", false), function(on)
  config.set("example.clock", on)
end, "Kept in the settings store, so it survives reloads.")

local speed = m:number("Speed", 5, 1, function(v) log("speed is now", v) end, "Left and right change it by one.")
speed.min, speed.max, speed.format = 0, 10, function(v) return v .. " m/s" end

m:choice("Colour", { "Red", "Green", "Blue" }, 1, function(index, name)
  log("picked", index, name)
end, "Left and right step through the list.")

m:label(function() return "Engine", game.mode() end, "A label with a live value on the right.")

local locked = m:action("Call the game", function() end, "Needs the injected engine.")
locked.unavailable = function()
  if game.mode() ~= "internal" then return "needs the injected engine" end
end

local more = m:submenu("More", "A nested menu with a side panel.")
more:label("Rows here are read-only text.")
more:action("Back to the top", function() menu.stack = {} end)
more.on_open = function(sub) log("opened", sub.title) end
more.side = {
  width = 420,
  title = "Panel",
  info = function() return string.format("%.0f s", now() / 1000) end,
  draw = function(x, y, w, style)
    local h = style.row * 2
    ui.rect(x, y, w, h, style.row_bg)
    ui.text(x + style.pad, style.mid(y, h, style.size), "Drawn by more.side.draw", 0xFFFFFFFF, style.size)
  end,
}

on_frame(function()
  if not config.get("example.clock", false) then return end
  ui.text(ui.width() - 20, 20, os.date("%H:%M:%S"), 0xFFFFFFFF, 24, "bold", { align = "right" })
end)
