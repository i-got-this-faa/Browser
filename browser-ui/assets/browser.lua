-- strip-browser default configuration
-- Location: ~/.config/strip-browser/browser.lua
-- This file defines the whole browser experience. Save to hot-reload.
--
-- Shapes:
--   browser.request { "page", "new_beside" }   -- emit a request from Lua
--   browser.request { cmd = "workspace.focus", arg = 2 }
--   browser.tabs, browser.active_page, browser.active_workspace  -- live snapshot
--
-- Prompt usage: type a URL, search words, or ":command args".
-- The palette (ctrl+p) filters every built-in and user command.

return {
  theme = {
    bg           = "#101014",
    bar          = "#0c0c10",
    bar_text     = "#9aa0b0",
    border       = "#2a2c3a",
    border_focus = "#7aa2f7",
    prompt_bg    = "#1c1d27",
    prompt_text  = "#e6e9f0",
    accent       = "#7aa2f7",
  },

  behavior = {
    page_width_fraction = 0.78,   -- how much of the viewport one page takes
    gap                 = 12,     -- px between pages on the strip
    home_page           = "https://duckduckgo.com",
    search_engine_url   = "https://duckduckgo.com/?q={}",
    smooth_scroll       = 0.18,   -- per-frame easing fraction (0.05 slow .. 1 instant)
    show_page_bar       = true,   -- top strip of page chips
    show_status_bar     = true,   -- bottom bar: workspaces + url
  },

  -- Every default binding can be replaced here. Full command list:
  --   focus.url, page.reload, page.reload_bypass_cache, page.back,
  --   page.forward, page.new, page.new_beside, page.close, focus.left,
  --   focus.right, focus.up, focus.down, page.move_left, page.move_right,
  --   page.next, page.prev, workspace.new, workspace.next, workspace.prev,
  --   workspace.focus (arg = n), page.to_workspace (arg = n),
  --   overview.toggle, layout.scroll_left, layout.scroll_right,
  --   palette.open, config.reload, app.quit
  keys = {
    { "ctrl+t",        "page.new" },
    { "ctrl+shift+t",  "page.new_beside" },
    { "ctrl+w",        "page.close" },
    { "ctrl+l",        "focus.right" },
    { "ctrl+h",        "focus.left" },
    { "ctrl+alt+l",    "focus.right" },
    { "ctrl+alt+h",    "focus.left" },
    { "ctrl+shift+l",  "page.move_right" },
    { "ctrl+shift+h",  "page.move_left" },
    { "ctrl+n",        "page.next" },
    { "ctrl+shift+n",  "page.prev" },
    { "ctrl+alt+n",    "workspace.next" },
    { "ctrl+alt+shift+n", "workspace.new" },
    { "alt+left",      "page.back" },
    { "alt+right",     "page.forward" },
    { "ctrl+r",        "page.reload" },
    { "ctrl+shift+r",  "page.reload_bypass_cache" },
    { "ctrl+k",        "focus.url" },          -- prompt: address or search
    { "ctrl+pagedown", "page.next" },
    { "ctrl+pageup",   "page.prev" },
    { "ctrl+1",        "workspace.focus", arg = "1" },
    { "ctrl+2",        "workspace.focus", arg = "2" },
    { "ctrl+3",        "workspace.focus", arg = "3" },
    { "ctrl+4",        "workspace.focus", arg = "4" },
    { "ctrl+shift+1",  "page.to_workspace", arg = "1" },
    { "ctrl+shift+2",  "page.to_workspace", arg = "2" },
    { "ctrl+o",        "overview.toggle" },
    { "ctrl+shift+o",  "overview.toggle" },
    { "ctrl+shift+p",  "palette.open" },
    { "ctrl+p",        "palette.open" },
    { "ctrl+shift+e",  "config.reload" },
    { "ctrl+q",        "app.quit" },
  },

  commands = {
    ["my.hackernews"] = {
      desc = "Open Hacker News beside this page",
      run = function()
        browser.request { "page.new_beside" }
        return { "page.navigate", "https://news.ycombinator.com" }
      end,
    },

    ["my.to-work"] = {
      desc = "Move the active page to workspace 2 (work)",
      run = function()
        return { cmd = "page.to_workspace", arg = 2 }
      end,
    },
  },

  events = {
    page_created = function(page)
      -- Send social media to workspace 3 automatically.
      if page.url:find("^https://twitter") or page.url:find("^https://x%.com") then
        return { cmd = "page.to_workspace", arg = 3 }
      end
    end,
  },
}
