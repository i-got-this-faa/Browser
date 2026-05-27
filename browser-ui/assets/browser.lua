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
